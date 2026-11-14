//! gc/clean maintenance of the store: `sweep`, `clean`, marker retention and the `configs-v1` reader
//! (ARCHITECTURE §5.6).
//!
//! Everything here is best effort and contained: deletions go through `snp_os::fs::{retire_dir,
//! remove_tree_within}` (rename into `trash/`, fd-relative removal), only names of snp's own layout are removed,
//! and an entry that cannot be read or removed is left alone.
//!
//! Owner: WP-22.

use std::collections::{BTreeMap, BTreeSet};
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

pub(crate) mod protect;

use protect::{CleanStep, NotUnderstood, PatchKind};

/// Scratch/trash entries and unreferenced tool versions younger than this are kept (a process may still use them).
const ORPHAN_AGE: Duration = Duration::from_hours(1);
/// Restored stash patches younger than this are kept (D-DATA-04).
const PATCH_AGE: Duration = Duration::from_hours(30 * 24);
/// [`PATCH_AGE`] in whole seconds, as the K8 kernel compares ages: the limit has no fractional part, so an age `a`
/// is at least the limit exactly when `a.as_secs()` is.
const PATCH_AGE_SECS: u64 = PATCH_AGE.as_secs();
/// [`ORPHAN_AGE`] in whole seconds (`clean`'s age limit for patches; exact as for [`PATCH_AGE_SECS`]).
const ORPHAN_AGE_SECS: u64 = ORPHAN_AGE.as_secs();
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

/// What the stash journals of `patches/` protect from removal (the K8 kernel's [`protect::Protection`]); `All`
/// names the journal (or `patches/` itself) that could not be read or understood.
type Protection = protect::Protection<PathBuf>;

/// The journals [`protection`] already decoded, keyed by path and stamped with `(len, mtime)`: `clean` re-reads the
/// journals right before each removal, and only a journal whose stamp changed is read and parsed again. A stale entry
/// is harmless: a journal only ever starts naming a patch its run has just written (younger than both age limits),
/// and later states only drop protection.
#[derive(Debug, Default)]
struct JournalCache(BTreeMap<PathBuf, (Stamp, Option<Vec<u8>>)>);

/// A journal's `(len, mtime)`.
type Stamp = (u64, SystemTime);

/// What the journals of `patches/` protect: every journal except those whose `state` is `restored` or `done` (the
/// patch's content is back in the worktree) protects the patch its `patch` field names, decoded with the journal's
/// byte-string codec (a JSON string, or `{"hex": …}` for a non-UTF-8 path). Fails closed ([`Protection::All`]) when
/// the directory cannot be listed, a journal cannot be read (a journal deleted meanwhile, by a completed restore, is
/// skipped), is bigger than [`layout::JOURNAL_CAP`], or is not understood (see [`journal_patch`]). Journals are read
/// in full: a cap must never cut the `patch` field off. An empty journal is not understood either: a non-durable
/// state (`rolling-back`, `failed`) is written by an unsynced rename, which a power loss can leave empty, so an empty
/// journal may be one that protected a patch.
fn protection(patches: &Path, cache: &mut JournalCache) -> Protection {
    protection_capped(patches, layout::JOURNAL_CAP, cache)
}

/// [`protection`] with the journal size limit `cap`.
fn protection_capped(patches: &Path, cap: u64, cache: &mut JournalCache) -> Protection {
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
        let stamp = match fs::metadata(&path) {
            Ok(m) => m.modified().ok().map(|t| (m.len(), t)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Protection::All(path),
        };
        let verdict = match (stamp, cache.0.get(&path)) {
            (Some(stamp), Some((cached, name))) if *cached == stamp => Ok(name.clone()),
            _ => {
                let bytes = match snp_os::fs::read_capped(&path, cap.saturating_add(1)) {
                    Ok(b) if u64::try_from(b.len()).is_ok_and(|n| n <= cap) => b,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Ok(_) | Err(_) => return Protection::All(path),
                };
                let verdict = journal_patch(&bytes);
                if let (Some(stamp), Ok(name)) = (stamp, &verdict) {
                    cache.0.insert(path.clone(), (stamp, name.clone()));
                }
                verdict
            }
        };
        if !protect::add_journal(&mut names, verdict) {
            return Protection::All(path);
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

/// The file name of the patch a journal protects: `Ok(None)` when it protects none (state `restored` or `done`, or
/// no patch journaled yet: `patch` absent or `null` in a state other than an unrestored one), `Err` when the journal
/// is not understood (not a JSON object with a string `state`, an unrestored state — `stashed`, `staging-fixed`,
/// `rolling-back`, `failed`, snp-stash `is_unrestored` — without a patch, a `patch` that does not decode or names no
/// file). `serde_json` keeps its default recursion limit, so hostile nesting is refused, never a stack overflow.
///
/// Unverified shell of kernel K8: `serde_json` decodes the journal and `Path::file_name` takes the patch's name;
/// what the decoded journal protects is [`protect::journal_verdict`].
fn journal_patch(bytes: &[u8]) -> Result<Option<Vec<u8>>, NotUnderstood> {
    #[derive(serde::Deserialize)]
    struct Journal<'a> {
        #[serde(borrow)]
        state: std::borrow::Cow<'a, str>,
        #[serde(default)]
        patch: serde_json::Value,
    }
    let j = serde_json::from_slice::<Journal<'_>>(bytes).map_err(|_| NotUnderstood)?;
    let (kind, payload) = match &j.patch {
        serde_json::Value::Null => (PatchKind::Null, &b""[..]),
        serde_json::Value::String(s) => (PatchKind::Str, s.as_bytes()),
        serde_json::Value::Object(m) => match m.get("hex").and_then(serde_json::Value::as_str) {
            Some(h) => (PatchKind::Hex, h.as_bytes()),
            None => (PatchKind::Other, &b""[..]),
        },
        _ => (PatchKind::Other, &b""[..]),
    };
    let Some(path) = protect::journal_verdict(j.state.as_bytes(), kind, payload)? else {
        return Ok(None);
    };
    let patch = marker::path_from_bytes(path).ok_or(NotUnderstood)?;
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
    match protection(&patches, &mut JournalCache::default()) {
        Protection::All(journal) => report.unreadable_journal = Some(journal),
        protection @ Protection::Patches(_) => {
            for e in &entries(&patches) {
                let name = e.file_name();
                let bytes = name.as_encoded_bytes();
                let path = e.path();
                let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                if protect::sweep_deletable(
                    &protection,
                    bytes,
                    is_dir,
                    age_secs(&path, now),
                    PATCH_AGE_SECS,
                ) && fs::remove_file(&path).is_ok()
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

/// The age of `path` in whole seconds (`None` when unknown).
fn age_secs(path: &Path, now: SystemTime) -> Option<u64> {
    age(path, now).map(|a| a.as_secs())
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
    // unjournaled patch may be about to be restored. The journals are re-read right before each removal (only those
    // that changed are parsed again); one that cannot be read keeps everything (fail closed).
    let patches = root.join(layout::PATCHES);
    let now = SystemTime::now();
    let mut unreadable = None;
    let mut cache = JournalCache::default();
    for e in &entries(&patches) {
        let name = e.file_name();
        let bytes = name.as_encoded_bytes();
        let path = e.path();
        let (journal, age) = (is_journal(e), age_secs(&path, now));
        if !protect::clean_candidate(journal, age, ORPHAN_AGE_SECS) {
            continue;
        }
        let p = protection(&patches, &mut cache);
        match protect::clean_step(p, bytes, journal, age, ORPHAN_AGE_SECS) {
            CleanStep::Stop(journal) => {
                unreadable = Some(journal);
                break;
            }
            CleanStep::Keep => continue,
            CleanStep::Remove => {}
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
        assert_eq!(journal_patch(br#"{"state":"some-future-state"}"#), Ok(None));
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
            (
                br#"{"state":"rolling-back","patch":"/p/patch2-2"}"#,
                b"patch2-2",
            ),
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
            b"{}",
            br#"{"state":null}"#,
            br#"{"patch":"/p/patch2-2"}"#,
            br#"{"state":null,"patch":"/p/patch2-2"}"#,
            br#"{"v":1,"state":"stashed"}"#,
            br#"{"v":1,"state":"failed","patch":null}"#,
            br#"{"state":"staging-fixed"}"#,
            br#"{"state":"rolling-back","patch":null}"#,
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
        let fresh = JournalCache::default;
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        assert_eq!(
            protection(&dir.join("missing"), &mut fresh()),
            Protection::Patches(Vec::new())
        );
        fs::write(dir.join("patch1-1"), b"").unwrap();
        fs::write(
            dir.join("run-1.snp-journal"),
            br#"{"state":"failed","patch":"/x/patch1-1"}"#,
        )
        .unwrap();
        assert_eq!(
            protection(dir, &mut fresh()),
            Protection::Patches(vec![b"patch1-1".to_vec()])
        );
        // Over the size limit: not understood, never cut short.
        let len = fs::metadata(dir.join("run-1.snp-journal")).unwrap().len();
        assert_eq!(
            protection_capped(dir, len, &mut fresh()),
            Protection::Patches(vec![b"patch1-1".to_vec()])
        );
        assert_eq!(
            protection_capped(dir, len - 1, &mut fresh()),
            Protection::All(dir.join("run-1.snp-journal"))
        );
        // An empty journal may be a protecting one a power loss emptied (an unsynced `failed` rename).
        fs::write(dir.join("run-3.snp-journal"), b"").unwrap();
        assert_eq!(
            protection(dir, &mut fresh()),
            Protection::All(dir.join("run-3.snp-journal"))
        );
        fs::remove_file(dir.join("run-3.snp-journal")).unwrap();
        fs::create_dir_all(dir.join("run-2.snp-journal")).unwrap();
        assert_eq!(
            protection(dir, &mut fresh()),
            Protection::All(dir.join("run-2.snp-journal"))
        );
    }

    #[test]
    fn the_cache_rereads_a_changed_journal() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        let journal = dir.join("run-1.snp-journal");
        fs::write(&journal, br#"{"state":"started"}"#).unwrap();
        let mut cache = JournalCache::default();
        assert_eq!(protection(dir, &mut cache), Protection::Patches(Vec::new()));
        assert_eq!(cache.0.len(), 1);
        // A changed length is a changed stamp, whatever the mtime granularity.
        fs::write(&journal, br#"{"state":"stashed","patch":"/x/patch1-1"}"#).unwrap();
        assert_eq!(
            protection(dir, &mut cache),
            Protection::Patches(vec![b"patch1-1".to_vec()])
        );
        // An unchanged journal is served from the cache.
        assert_eq!(
            protection(dir, &mut cache),
            Protection::Patches(vec![b"patch1-1".to_vec()])
        );
        fs::write(&journal, b"").unwrap();
        assert_eq!(protection(dir, &mut cache), Protection::All(journal));
    }

    /// The decisions as they were before the K8 kernel (PR #19), kept to tie the kernel to them.
    mod former {
        use super::super::{ORPHAN_AGE, PATCH_AGE};
        use std::path::PathBuf;
        use std::time::Duration;

        pub(super) fn is_patch_or_sibling(name: &[u8], patch: &[u8]) -> bool {
            name.strip_prefix(patch)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"."))
        }

        pub(super) fn covers(all: bool, names: &[Vec<u8>], name: &[u8]) -> bool {
            all || names.iter().any(|p| is_patch_or_sibling(name, p))
        }

        pub(super) fn sweep_removes(
            all: bool,
            names: &[Vec<u8>],
            name: &[u8],
            is_dir: bool,
            age: Option<Duration>,
        ) -> bool {
            !all && name.starts_with(b"patch")
                && !is_dir
                && age.is_some_and(|a| a >= PATCH_AGE)
                && !covers(all, names, name)
        }

        pub(super) fn clean_considers(journal: bool, age: Option<Duration>) -> bool {
            !(journal || age.is_none_or(|a| a < ORPHAN_AGE))
        }

        /// The former `clean` loop body after its pre-check: `None` stops, `Some(remove)` otherwise.
        pub(super) fn clean_action(
            all: bool,
            names: &[Vec<u8>],
            name: &[u8],
            journal: bool,
            age: Option<Duration>,
        ) -> Option<bool> {
            if !clean_considers(journal, age) {
                return Some(false);
            }
            if all {
                return None;
            }
            Some(!covers(all, names, name))
        }

        pub(super) fn unhex(s: &[u8]) -> Option<Vec<u8>> {
            if !s.len().is_multiple_of(2) {
                return None;
            }
            let digit = |b: u8| -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b.wrapping_sub(b'0')),
                    b'a'..=b'f' => Some(b.wrapping_sub(b'a').wrapping_add(10)),
                    b'A'..=b'F' => Some(b.wrapping_sub(b'A').wrapping_add(10)),
                    _ => None,
                }
            };
            let mut out = Vec::new();
            let mut it = s.iter();
            while let (Some(&h), Some(&l)) = (it.next(), it.next()) {
                out.push((digit(h)? << 4) | digit(l)?);
            }
            Some(out)
        }

        fn path_from_json(v: &serde_json::Value) -> Option<PathBuf> {
            match v {
                serde_json::Value::String(s) => Some(PathBuf::from(s)),
                serde_json::Value::Object(m) => {
                    let bytes = unhex(m.get("hex")?.as_str()?.as_bytes())?;
                    crate::marker::path_from_bytes(bytes)
                }
                _ => None,
            }
        }

        pub(super) fn journal_patch(bytes: &[u8]) -> Result<Option<Vec<u8>>, ()> {
            #[derive(serde::Deserialize)]
            struct Journal<'a> {
                #[serde(borrow)]
                state: std::borrow::Cow<'a, str>,
                #[serde(default)]
                patch: serde_json::Value,
            }
            let j = serde_json::from_slice::<Journal<'_>>(bytes).map_err(|_| ())?;
            match &*j.state {
                "restored" | "done" => return Ok(None),
                "stashed" | "staging-fixed" | "rolling-back" | "failed" if j.patch.is_null() => {
                    return Err(());
                }
                _ if j.patch.is_null() => return Ok(None),
                _ => {}
            }
            let patch = path_from_json(&j.patch).ok_or(())?;
            let name = patch.file_name().ok_or(())?;
            Ok(Some(name.as_encoded_bytes().to_vec()))
        }
    }

    /// `SplitMix64` (the snp-pycompat `tests/properties.rs` precedent): seeded, std-only.
    struct Mix(u64);

    impl Mix {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn bytes(&mut self, max: u64) -> Vec<u8> {
            let len = self.next() % (max + 1);
            (0..len).map(|_| self.next().to_le_bytes()[0]).collect()
        }
    }

    /// Every word over `alphabet` up to `max` bytes.
    fn words(alphabet: &[u8], max: usize) -> Vec<Vec<u8>> {
        let mut all = vec![Vec::new()];
        let mut last = vec![Vec::new()];
        for _ in 0..max {
            let next: Vec<Vec<u8>> = last
                .iter()
                .flat_map(|w| {
                    alphabet.iter().map(move |&c| {
                        let mut w = w.clone();
                        w.push(c);
                        w
                    })
                })
                .collect();
            all.extend(next.iter().cloned());
            last = next;
        }
        all
    }

    /// K8 G1: the kernel's sibling test and `covers` are the former ones, on every name and patch list over
    /// `{p, ., -, a}` (names up to 5 bytes against single patches up to 5 bytes; names up to 4 bytes against every
    /// list of at most two patches up to 3 bytes).
    #[test]
    fn protection_kernel_matches_former_code_exhaustively() {
        let alphabet = b"p.-a";
        let long = words(alphabet, 5);
        for name in &long {
            for patch in &long {
                assert_eq!(
                    protect::is_patch_or_sibling(name, patch),
                    former::is_patch_or_sibling(name, patch),
                    "{name:?} {patch:?}"
                );
            }
        }
        let short = words(alphabet, 3);
        let mut lists: Vec<Vec<Vec<u8>>> = vec![Vec::new()];
        for a in &short {
            lists.push(vec![a.clone()]);
            for b in &short {
                lists.push(vec![a.clone(), b.clone()]);
            }
        }
        let names = words(alphabet, 4);
        for list in lists {
            let p = Protection::Patches(list.clone());
            for name in &names {
                assert_eq!(p.covers(name), former::covers(false, &list, name));
            }
        }
        assert!(Protection::All(PathBuf::new()).covers(b""));
    }

    /// K8 G2: `sweep_deletable`, `clean_candidate` and `clean_step`, fed whole-second ages, decide what the former
    /// `Duration` comparisons and `clean` loop decided, at and around both thresholds, for every protection shape.
    #[test]
    fn removal_decisions_match_former_code() {
        assert_eq!(PATCH_AGE.subsec_nanos(), 0);
        assert_eq!(ORPHAN_AGE.subsec_nanos(), 0);
        let ns = Duration::from_nanos(1);
        let s = Duration::from_secs(1);
        let ages = |t: Duration| {
            [
                None,
                Some(Duration::ZERO),
                t.checked_sub(s),
                t.checked_sub(ns),
                Some(t),
                Some(t + ns),
                Some(t + s),
                Some(Duration::MAX),
            ]
        };
        let names: [&[u8]; 9] = [
            b"",
            b"patc",
            b"patch",
            b"patch1-1",
            b"patch1-1.now",
            b"patch1-1-2",
            b"xpatch1-1",
            b"run-1.snp-journal",
            b"Patch1-1",
        ];
        let lists = [
            None,
            Some(vec![]),
            Some(vec![b"patch1-1".to_vec()]),
            Some(vec![b"patch1-1-2".to_vec(), b"patch".to_vec()]),
        ];
        for list in &lists {
            let p = list
                .clone()
                .map_or_else(|| Protection::All(PathBuf::new()), Protection::Patches);
            let (all, names_of) = (list.is_none(), list.as_deref().unwrap_or_default());
            for name in names {
                for is_dir in [false, true] {
                    for age in ages(PATCH_AGE) {
                        assert_eq!(
                            protect::sweep_deletable(
                                &p,
                                name,
                                is_dir,
                                age.map(|a| a.as_secs()),
                                PATCH_AGE_SECS
                            ),
                            former::sweep_removes(all, names_of, name, is_dir, age),
                            "{name:?} {is_dir} {age:?} {list:?}"
                        );
                    }
                }
            }
        }
        for journal in [false, true] {
            for age in ages(ORPHAN_AGE) {
                let secs = age.map(|a| a.as_secs());
                assert_eq!(
                    protect::clean_candidate(journal, secs, ORPHAN_AGE_SECS),
                    former::clean_considers(journal, age),
                    "{journal} {age:?}"
                );
                for list in &lists {
                    let (all, names_of) = (list.is_none(), list.as_deref().unwrap_or_default());
                    for name in names {
                        let p = list
                            .clone()
                            .map_or_else(|| Protection::All(PathBuf::new()), Protection::Patches);
                        let step =
                            match protect::clean_step(p, name, journal, secs, ORPHAN_AGE_SECS) {
                                CleanStep::Stop(_) => None,
                                CleanStep::Keep => Some(false),
                                CleanStep::Remove => Some(true),
                            };
                        assert_eq!(
                            step,
                            former::clean_action(all, names_of, name, journal, age),
                            "{name:?} {journal} {age:?} {list:?}"
                        );
                    }
                }
            }
        }
    }

    /// K8 G4: the byte-string codec is the former `marker` decoder on every string over hex-digit boundary bytes up
    /// to 4 bytes, and decodes `keys::hex` (and its upper-case form) back to the bytes: every single byte and 2,000
    /// seeded random strings up to 64 bytes.
    #[test]
    fn unhex_matches_former_code_and_round_trips() {
        for s in words(b"09afAFgG/:@`", 4) {
            assert_eq!(protect::unhex(&s), former::unhex(&s), "{s:?}");
        }
        let mut rng = Mix(0x6b38);
        let singles = (0..=255_u8).map(|b| vec![b]);
        let random: Vec<Vec<u8>> = (0..2000).map(|_| rng.bytes(64)).collect();
        for b in singles.chain(random) {
            let lower = crate::keys::hex(&b);
            assert_eq!(protect::unhex(lower.as_bytes()), Some(b.clone()));
            assert_eq!(
                protect::unhex(lower.to_ascii_uppercase().as_bytes()),
                Some(b)
            );
        }
    }

    /// K8 G4: `journal_patch` (`serde_json`, the kernel's verdict, `Path::file_name`) returns what the former code
    /// returned, on every combination of journaled states and `patch` shapes below, malformed journals and 2,000
    /// seeded random hex paths.
    #[test]
    fn journal_patch_matches_former_code() {
        let states = [
            "\"restored\"",
            "\"done\"",
            "\"stashed\"",
            "\"staging-fixed\"",
            "\"rolling-back\"",
            "\"failed\"",
            "\"started\"",
            "\"some-future-state\"",
            "\"\"",
            "\"Stashed\"",
            "\"restored \"",
            "\"fail\"",
            "null",
            "1",
        ];
        let patches = [
            None,
            Some("null"),
            Some("\"/p/patch1-1\""),
            Some("\"patch1-1\""),
            Some("\"/p/patch1-1/\""),
            Some("\"/p/x/.\""),
            Some("\"/p/..\""),
            Some("\"/\""),
            Some("\"\""),
            Some("\".\""),
            Some("7"),
            Some("true"),
            Some("[]"),
            Some("{}"),
            Some("{\"x\":1}"),
            Some("{\"hex\":7}"),
            Some("{\"hex\":null}"),
            Some("{\"hex\":\"\"}"),
            Some("{\"hex\":\"2f702f7061746368342d34\"}"),
            Some("{\"hex\":\"2F702F7061746368342D34\"}"),
            Some("{\"hex\":\"2fff2f7061746368352d35\"}"),
            Some("{\"hex\":\"2f7\"}"),
            Some("{\"hex\":\"zz\"}"),
            Some("{\"hex\":\"2f702f7061746368342d3g\"}"),
        ];
        let mut journals: Vec<String> = Vec::new();
        for state in states {
            for patch in patches {
                journals.push(match patch {
                    None => format!("{{\"v\":1,\"state\":{state}}}"),
                    Some(p) => format!("{{\"v\":1,\"state\":{state},\"ita\":[],\"patch\":{p}}}"),
                });
            }
        }
        for patch in patches.into_iter().flatten() {
            journals.push(format!("{{\"patch\":{patch}}}"));
        }
        for broken in ["", "[]", "{", "{}", "null", "{\"state\":\"resto", "[[[["] {
            journals.push(broken.to_owned());
        }
        let mut rng = Mix(0x4b38);
        for _ in 0..2000 {
            let hex = crate::keys::hex(&rng.bytes(24));
            journals.push(format!(
                "{{\"state\":\"failed\",\"patch\":{{\"hex\":\"{hex}\"}}}}"
            ));
        }
        for j in &journals {
            assert_eq!(
                journal_patch(j.as_bytes()).map_err(|NotUnderstood| ()),
                former::journal_patch(j.as_bytes()),
                "{j}"
            );
        }
    }
}
