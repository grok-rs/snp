//! gc/clean maintenance of the store: `sweep`, `clean`, marker retention and the `configs-v1` reader
//! (ARCHITECTURE §5.6).
//!
//! Everything here is best effort and contained: deletions go through `snp_os::fs::{retire_dir,
//! remove_tree_within}` (rename into `trash/`, fd-relative removal), only names of snp's own layout are removed,
//! and an entry that cannot be read or removed is left alone.
//!
//! Owner: WP-22.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use snp_core::failure::Failure;
use snp_os::lock::{FileLock, LockMode};

use crate::keys::{MarkerKey, RepoKey};
use crate::layout::{self, age};
use crate::location::readme_first_line;
use crate::store::SweepReport;
use crate::{marker, messages};

/// Scratch/trash entries and unreferenced tool versions younger than this are kept (a process may still use them).
const ORPHAN_AGE: Duration = Duration::from_hours(1);
/// Restored stash patches younger than this are kept (D-DATA-04).
const PATCH_AGE: Duration = Duration::from_hours(30 * 24);
/// Read cap of markers and config entries (journals are read in full, up to `layout::JOURNAL_CAP`).
const SMALL_CAP: u64 = 64 * 1024;

/// The entries of a directory (empty when it cannot be read).
fn entries(dir: &Path) -> Vec<fs::DirEntry> {
    fs::read_dir(dir).map_or_default(|rd| rd.filter_map(Result::ok).collect())
}

/// `(entry file, config realpath, project root)` of every well-formed `configs-v1` entry.
pub(crate) fn config_entries(dir: &Path) -> Vec<(PathBuf, PathBuf, PathBuf)> {
    entries(dir)
        .into_iter()
        .filter(|e| RepoKey::from_hex(e.file_name().as_encoded_bytes()).is_some())
        .filter_map(|e| {
            let path = e.path();
            let bytes = snp_os::fs::read_capped(&path, SMALL_CAP).ok()?;
            let (cfg, root) = marker::decode_config_entry(&bytes)?;
            Some((path, cfg, root))
        })
        .collect()
}

/// Keeps the markers `keep` accepts; removes the rest (and undecodable files). Returns the number removed.
pub(crate) fn retain_markers(dir: &Path, keep: &mut dyn FnMut(&MarkerKey) -> bool) -> usize {
    let mut removed: usize = 0;
    for e in entries(dir) {
        let path = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let decoded = snp_os::fs::read_capped(&path, SMALL_CAP)
            .ok()
            .and_then(|b| MarkerKey::decode(&b));
        let kept = match &decoded {
            Some(k) => keep(k),
            None => false,
        };
        if !kept && fs::remove_file(&path).is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    removed
}

/// What the stash journals of `patches/` protect from removal (D-DATA-04, ARCHITECTURE §5.6).
#[derive(Debug, PartialEq, Eq)]
enum Protection {
    /// The file names of the patches of unrestored journals; each protects itself and its `<name>.*` siblings.
    Patches(Vec<Vec<u8>>),
    /// A journal (or the directory listing, then `patches/` itself) could not be read or understood: what it
    /// protects is unknown, so every patch is kept (fail closed).
    All(PathBuf),
}

impl Protection {
    /// Whether the entry `name` of `patches/` is protected.
    fn covers(&self, name: &[u8]) -> bool {
        match self {
            Self::All(_) => true,
            Self::Patches(names) => names.iter().any(|p| is_patch_or_sibling(name, p)),
        }
    }
}

/// Whether `name` is the patch `patch` or one of its `<patch>.*` siblings (the `<patch>.now` backup of the K+ hint,
/// the kept `.ita` and `.paths` lists, their temporary files). `patch<s>-<pid>-<n>` is another patch, not a sibling.
fn is_patch_or_sibling(name: &[u8], patch: &[u8]) -> bool {
    name.strip_prefix(patch)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"."))
}

/// What the journals of `patches/` protect: every journal except those whose `state` is `restored` or `done` (the
/// patch's content is back in the worktree) protects the patch its `patch` field names, decoded with the journal's
/// byte-string codec (a JSON string, or `{"hex": …}` for a non-UTF-8 path). Fails closed ([`Protection::All`]) when
/// the directory cannot be listed, a journal cannot be read (a journal deleted meanwhile, by a completed restore, is
/// skipped), is bigger than [`layout::JOURNAL_CAP`], is not a JSON object with a string `state`, or names its patch
/// in a form that does not decode. Journals are read in full: a cap must never cut the `patch` field off.
fn protection(patches: &Path) -> Protection {
    protection_capped(patches, layout::JOURNAL_CAP)
}

/// [`protection`] with the journal size limit `cap`.
fn protection_capped(patches: &Path, cap: u64) -> Protection {
    let listing = match fs::read_dir(patches) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Protection::Patches(Vec::new()),
        Err(_) => return Protection::All(patches.to_path_buf()),
    };
    let mut names = Vec::new();
    for e in listing {
        let Ok(e) = e else {
            return Protection::All(patches.to_path_buf());
        };
        if !is_journal(&e) {
            continue;
        }
        let path = e.path();
        let bytes = match snp_os::fs::read_capped(&path, cap.saturating_add(1)) {
            Ok(b) if u64::try_from(b.len()).is_ok_and(|n| n <= cap) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Ok(_) | Err(_) => return Protection::All(path),
        };
        match journal_patch(&bytes) {
            Ok(Some(name)) => names.push(name),
            Ok(None) => {}
            Err(NotUnderstood) => return Protection::All(path),
        }
    }
    Protection::Patches(names)
}

/// Whether `e` is a stash journal (`*.snp-journal`).
fn is_journal(e: &fs::DirEntry) -> bool {
    e.file_name()
        .as_encoded_bytes()
        .ends_with(layout::JOURNAL_SUFFIX.as_bytes())
}

/// A journal that is not understood.
#[derive(Debug, PartialEq, Eq)]
struct NotUnderstood;

/// The file name of the patch a journal protects: `Ok(None)` when it protects none (state `restored` or `done`, or
/// no patch journaled yet: `patch` absent or `null`), `Err` when the journal is not understood (not a JSON object
/// with a string `state`, a `patch` that does not decode or names no file). `serde_json` keeps its default recursion
/// limit, so hostile nesting is refused, never a stack overflow.
fn journal_patch(bytes: &[u8]) -> Result<Option<Vec<u8>>, NotUnderstood> {
    #[derive(serde::Deserialize)]
    struct Journal<'a> {
        #[serde(borrow)]
        state: Option<std::borrow::Cow<'a, str>>,
        #[serde(default)]
        patch: serde_json::Value,
    }
    let j = serde_json::from_slice::<Journal<'_>>(bytes).map_err(|_| NotUnderstood)?;
    if j.state
        .as_deref()
        .is_some_and(|st| st == "restored" || st == "done")
    {
        return Ok(None);
    }
    if j.patch.is_null() {
        return Ok(None);
    }
    let patch = marker::path_from_json(&j.patch).ok_or(NotUnderstood)?;
    let name = patch.file_name().ok_or(NotUnderstood)?;
    Ok(Some(name.as_encoded_bytes().to_vec()))
}

/// `sweep` (see `Store::sweep`).
pub(crate) fn sweep(root: &Path, tools_root: &Path, now: SystemTime) -> SweepReport {
    let mut report = SweepReport::default();
    if !root.is_dir() {
        return report;
    }
    let trash = root.join(layout::TRASH);
    let mut count = |ok: bool| {
        if ok {
            report.removed_dirs = report.removed_dirs.saturating_add(1);
        }
    };

    // 1. Orphaned scratch and trash entries (kill -9 during a fill, an interrupted removal).
    for bucket in [layout::SCRATCH, layout::TRASH] {
        let dir = root.join(bucket);
        for e in entries(&dir) {
            let path = e.path();
            if age(&path, now).is_some_and(|a| a >= ORPHAN_AGE) {
                count(snp_os::fs::remove_tree_within(&path, &dir).is_ok());
            }
        }
    }

    // 2. Repository lock files whose repository is gone (retired by gc).
    let repos = root.join(layout::REPOS);
    for e in entries(&repos) {
        let name = e.file_name();
        let Some(stem) = name.as_encoded_bytes().strip_suffix(b".lock") else {
            continue;
        };
        if RepoKey::from_hex(stem).is_none() {
            continue;
        }
        let path = e.path();
        if fs::symlink_metadata(path.with_extension("")).is_ok() {
            continue;
        }
        if let Ok(Some(lock)) = FileLock::try_acquire(&path, LockMode::Exclusive) {
            drop(lock);
            let _removed: io::Result<()> = fs::remove_file(&path);
        }
    }

    // 3. Tool versions no environment references (only for a store that owns its tools).
    if tools_root == root {
        for removed in sweep_tools(root, &trash, now) {
            count(removed);
        }
    }

    // 4. Restored stash patches older than 30 days; journals and the patches of unrestored journals stay forever.
    // A journal that cannot be read keeps every patch (fail closed) and is reported.
    let patches = root.join(layout::PATCHES);
    match protection(&patches) {
        Protection::All(journal) => report.unreadable_journal = Some(journal),
        protection @ Protection::Patches(_) => {
            for e in &entries(&patches) {
                let name = e.file_name();
                let bytes = name.as_encoded_bytes();
                if !bytes.starts_with(b"patch") || e.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let path = e.path();
                if age(&path, now).is_some_and(|a| a >= PATCH_AGE)
                    && !protection.covers(bytes)
                    && fs::remove_file(&path).is_ok()
                {
                    report.removed_patches = report.removed_patches.saturating_add(1);
                }
            }
        }
    }

    // 5. Buckets of another format version (never parsed, 11 §A.10.3).
    for e in entries(root) {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if layout::parse_bucket(name).is_some_and(|(_, v)| v != layout::BUCKET_VERSION)
            && e.file_type().is_ok_and(|t| t.is_dir())
        {
            report.removed_dirs = report.removed_dirs.saturating_add(usize::from(
                snp_os::fs::retire_dir(&e.path(), &trash).is_ok(),
            ));
        }
    }

    // 6. snp 1.x leftovers, only behind the 1.x README signature (D-DIAG-05).
    if readme_first_line(root).as_deref() == Some(messages::SNP1_README_SIGNATURE) {
        let mut ok = true;
        for d in layout::SNP1_DIRS {
            ok &= snp_os::fs::retire_dir(&root.join(d), &trash).is_ok();
        }
        for f in snp1_db_files() {
            ok &= remove_file_if_present(&root.join(f)).is_ok();
        }
        if ok {
            ok = snp_os::fs::atomic_replace(
                &root.join(layout::README),
                messages::README.as_bytes(),
                None,
            )
            .is_ok();
        }
        report.legacy_v1_removed = ok;
    }
    report
}

/// The SQLite files of snp 1.x.
fn snp1_db_files() -> [String; 4] {
    let db = layout::SNP1_DB;
    [
        db.to_owned(),
        format!("{db}-journal"),
        format!("{db}-wal"),
        format!("{db}-shm"),
    ]
}

/// Removes tool versions that no `.snp-tools.json` references; one `bool` per attempted removal.
fn sweep_tools(root: &Path, trash: &Path, now: SystemTime) -> Vec<bool> {
    let mut referenced: BTreeSet<(String, String)> = BTreeSet::new();
    for repo in entries(&root.join(layout::REPOS)) {
        for env in entries(&repo.path().join("envs")) {
            let sidecar = env.path().join(layout::TOOLS_SIDECAR);
            let bytes = match snp_os::fs::read_capped(&sidecar, marker::META_CAP) {
                Ok(b) => b,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                // Unreadable: what it references is unknown, so nothing is removed.
                Err(_) => return Vec::new(),
            };
            let Some(tools) = marker::decode_tools(&bytes) else {
                return Vec::new();
            };
            for t in tools {
                referenced.insert((
                    layout::safe_component(&t.tool),
                    layout::safe_component(&t.version),
                ));
            }
        }
    }
    let tools_dir = root.join(layout::TOOLS);
    let mut out = Vec::new();
    for tool in entries(&tools_dir) {
        if !tool.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Some(tool_name) = tool.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let lock_path = tools_dir.join(format!("{tool_name}.lock"));
        for version in entries(&tool.path()) {
            let Some(v) = version.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = version.path();
            if referenced.contains(&(tool_name.clone(), v))
                || age(&path, now).is_none_or(|a| a < ORPHAN_AGE)
            {
                continue;
            }
            // A bootstrap in progress holds the tool lock: leave its tool alone.
            let Ok(Some(lock)) = FileLock::try_acquire(&lock_path, LockMode::Exclusive) else {
                continue;
            };
            out.push(snp_os::fs::retire_dir(&path, trash).is_ok());
            drop(lock);
        }
        let _empty_removed: io::Result<()> = fs::remove_dir(tool.path());
    }
    out
}

/// `remove_file`, `NotFound` tolerated.
fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `clean` (see `Store::clean`); returns the journal that could not be read, when every patch was kept for it.
pub(crate) fn clean(root: &Path) -> Result<Option<PathBuf>, Failure> {
    let trash = root.join(layout::TRASH);
    let io_err = |e: &io::Error, p: &Path| Failure::from_io(e, Some(p.as_os_str()), None);
    let readme = readme_first_line(root);
    let snp1 = readme.as_deref() == Some(messages::SNP1_README_SIGNATURE);
    let ours = snp1 || readme.as_deref() == messages::README.split('\n').next().map(str::as_bytes);

    let mut dirs: Vec<PathBuf> = [
        layout::REPOS,
        layout::KEYS,
        layout::CONFIGS,
        layout::TOOLS,
        layout::CACHE,
        layout::SPILL,
        layout::SCRATCH,
    ]
    .iter()
    .map(|d| root.join(d))
    .collect();
    for e in entries(root) {
        if let Some(name) = e.file_name().to_str()
            && layout::parse_bucket(name).is_some()
        {
            dirs.push(e.path());
        }
    }
    if snp1 {
        dirs.extend(layout::SNP1_DIRS.iter().map(|d| root.join(d)));
    }
    dirs.sort();
    dirs.dedup();
    for d in &dirs {
        snp_os::fs::retire_dir(d, &trash).map_err(|e| io_err(&e, d))?;
    }

    // Stash recovery data survives: every journal, the patches of unrestored journals and their siblings, and every
    // patch younger than an hour — a run stashing in another worktree creates its patch before it journals it
    // (ARCHITECTURE §4 steps 5–6) and takes the shared store lock only at its first repository use, so a fresh
    // unjournaled patch may be about to be restored. The journals are re-read right before each removal; one that
    // cannot be read keeps everything (fail closed).
    let patches = root.join(layout::PATCHES);
    let now = SystemTime::now();
    let mut unreadable = None;
    for e in &entries(&patches) {
        let name = e.file_name();
        let bytes = name.as_encoded_bytes();
        let path = e.path();
        if is_journal(e) || age(&path, now).is_none_or(|a| a < ORPHAN_AGE) {
            continue;
        }
        match protection(&patches) {
            Protection::All(journal) => {
                unreadable = Some(journal);
                break;
            }
            p if p.covers(bytes) => continue,
            Protection::Patches(_) => {}
        }
        snp_os::fs::remove_tree_within(&path, &patches).map_err(|err| io_err(&err, &path))?;
    }
    let _empty_removed: io::Result<()> = fs::remove_dir(&patches);

    let mut files: Vec<String> = vec![
        layout::STORE_FILE.to_owned(),
        "snp.log".to_owned(),
        "pre-commit.log".to_owned(),
    ];
    if ours {
        files.push(layout::README.to_owned());
    }
    if snp1 {
        files.extend(snp1_db_files());
    }
    for f in files {
        let p = root.join(f);
        remove_file_if_present(&p).map_err(|e| io_err(&e, &p))?;
    }
    snp_os::fs::remove_tree_within(&trash, root).map_err(|e| io_err(&e, &trash))?;
    Ok(unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_journals_and_unjournaled_runs_protect_nothing() {
        assert_eq!(
            journal_patch(br#"{"v":1,"state":"restored","patch":"/p/patch1-1"}"#),
            Ok(None)
        );
        assert_eq!(
            journal_patch(br#"{"state":"done","patch":{"x":1}}"#),
            Ok(None)
        );
        assert_eq!(
            journal_patch(br#"{"state":"started","patch":null}"#),
            Ok(None)
        );
        assert_eq!(journal_patch(br#"{"state":"started"}"#), Ok(None));
    }

    #[test]
    fn unrestored_journals_protect_their_patch() {
        for (journal, name) in [
            (
                &br#"{"state":"stashed","patch":"/p/patch1-1"}"#[..],
                &b"patch1-1"[..],
            ),
            (
                br#"{"state":"failed","patch":"/s/patches/patch1700000000-42-1"}"#,
                b"patch1700000000-42-1",
            ),
            (br#"{"patch":"/p/patch2-2"}"#, b"patch2-2"),
            (
                br#"{"state":"some-future-state","patch":"/p/patch3-3"}"#,
                b"patch3-3",
            ),
            // `snp-stash`'s `enc_bytes` of the UTF-8 path `/p/patch4-4`, in hex (either case decodes)
            (
                br#"{"state":"failed","patch":{"hex":"2f702f7061746368342d34"}}"#,
                b"patch4-4",
            ),
            (
                br#"{"state":"failed","patch":{"hex":"2F702F7061746368342D34"}}"#,
                b"patch4-4",
            ),
        ] {
            assert_eq!(
                journal_patch(journal),
                Ok(Some(name.to_vec())),
                "{journal:?}"
            );
        }
    }

    /// The byte-string codec of `snp-stash` (`journal.rs` `enc_bytes`): a non-UTF-8 path is `{"hex": …}`.
    #[cfg(unix)]
    #[test]
    fn a_non_utf8_patch_path_decodes() {
        // `/\xff/patch5-5`
        let j = br#"{"state":"stashed","ita":[],"patch":{"hex":"2fff2f7061746368352d35"}}"#;
        assert_eq!(journal_patch(j), Ok(Some(b"patch5-5".to_vec())));
    }

    #[test]
    fn journals_not_understood_protect_everything() {
        let deep: String = std::iter::repeat_n('[', 100_000).collect();
        for broken in [
            &br#"{"state":"resto"#[..],
            b"[[[[[[[[[[[[[[[[[[[[",
            b"",
            b"[]",
            br#"{"state":1,"patch":"/p/patch1-1"}"#,
            br#"{"state":"failed","patch":7}"#,
            br#"{"state":"failed","patch":{"hex":"2f7"}}"#,
            br#"{"state":"failed","patch":{"hex":"zz"}}"#,
            br#"{"state":"failed","patch":"/"}"#,
            br#"{"state":"failed","patch":""}"#,
            deep.as_bytes(),
        ] {
            assert_eq!(journal_patch(broken), Err(NotUnderstood), "{broken:?}");
        }
    }

    #[test]
    fn a_patch_protects_itself_and_its_siblings_only() {
        let p = Protection::Patches(vec![b"patch1700000000-42".to_vec()]);
        for kept in [
            &b"patch1700000000-42"[..],
            b"patch1700000000-42.now",
            b"patch1700000000-42.ita",
            b"patch1700000000-42.paths",
            b"patch1700000000-42.paths.tmp123",
        ] {
            assert!(p.covers(kept), "{kept:?}");
        }
        for not in [
            &b"patch1700000000-4"[..],
            b"patch1700000000-421",
            b"patch1700000000-42-1",
            b"patch1700000000-42now",
            b"patch1",
        ] {
            assert!(!p.covers(not), "{not:?}");
        }
        assert!(!Protection::Patches(Vec::new()).covers(b"patch1-1"));
        assert!(Protection::All(PathBuf::from("j")).covers(b"patch1-1"));
    }

    #[test]
    fn protection_fails_closed() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        assert_eq!(
            protection(&dir.join("missing")),
            Protection::Patches(Vec::new())
        );
        fs::write(dir.join("patch1-1"), b"").unwrap();
        fs::write(
            dir.join("run-1.snp-journal"),
            br#"{"state":"failed","patch":"/x/patch1-1"}"#,
        )
        .unwrap();
        assert_eq!(
            protection(dir),
            Protection::Patches(vec![b"patch1-1".to_vec()])
        );
        // Over the size limit: not understood, never cut short.
        let len = fs::metadata(dir.join("run-1.snp-journal")).unwrap().len();
        assert_eq!(
            protection_capped(dir, len),
            Protection::Patches(vec![b"patch1-1".to_vec()])
        );
        assert_eq!(
            protection_capped(dir, len - 1),
            Protection::All(dir.join("run-1.snp-journal"))
        );
        fs::create_dir_all(dir.join("run-2.snp-journal")).unwrap();
        assert_eq!(
            protection(dir),
            Protection::All(dir.join("run-2.snp-journal"))
        );
    }
}
