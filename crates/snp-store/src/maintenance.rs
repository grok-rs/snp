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
/// Read cap of journals, markers and config entries.
const SMALL_CAP: u64 = 64 * 1024;

/// The entries of a directory (empty when it cannot be read).
fn entries(dir: &Path) -> Vec<fs::DirEntry> {
    fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).collect())
        .unwrap_or_default()
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

/// The journals of `patches/` that still protect their patch: every journal except those whose `state` is
/// `restored` or `done` (the patch's content is back in the worktree; ARCHITECTURE §5.6 protects only
/// journaled-unrestored patches). A journal that cannot be parsed protects (it may be a half-written recovery record).
fn protecting_journals(patches: &Path) -> Vec<Vec<u8>> {
    entries(patches)
        .iter()
        .filter(|e| is_journal(e))
        .filter_map(|e| snp_os::fs::read_capped(&e.path(), marker::META_CAP).ok())
        .filter(|bytes| !journal_restored(bytes))
        .collect()
}

/// Whether `e` is a stash journal (`*.snp-journal`).
fn is_journal(e: &fs::DirEntry) -> bool {
    e.file_name()
        .as_encoded_bytes()
        .ends_with(layout::JOURNAL_SUFFIX.as_bytes())
}

/// Whether a journal records a completed restore (`"state": "restored" | "done"`). Bounded: the bytes are capped by
/// the caller and `serde_json` keeps its default recursion limit.
fn journal_restored(bytes: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct State<'a> {
        #[serde(borrow)]
        state: Option<std::borrow::Cow<'a, str>>,
    }
    serde_json::from_slice::<State<'_>>(bytes)
        .ok()
        .and_then(|j| j.state)
        .is_some_and(|st| st == "restored" || st == "done")
}

/// Whether a journal mentions the patch `name` as a whole path component.
fn mentioned(name: &[u8], journals: &[Vec<u8>]) -> bool {
    journals.iter().any(|j| {
        memchr::memmem::find_iter(j, name).any(|at| {
            let next = at.checked_add(name.len()).and_then(|i| j.get(i));
            !next.is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
    })
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
    let patches = root.join(layout::PATCHES);
    let all = entries(&patches);
    let journals = protecting_journals(&patches);
    for e in &all {
        let name = e.file_name();
        let bytes = name.as_encoded_bytes();
        if !bytes.starts_with(b"patch") || e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let path = e.path();
        if age(&path, now).is_some_and(|a| a >= PATCH_AGE)
            && !mentioned(bytes, &journals)
            && fs::remove_file(&path).is_ok()
        {
            report.removed_patches = report.removed_patches.saturating_add(1);
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

/// `clean` (see `Store::clean`).
pub(crate) fn clean(root: &Path) -> Result<(), Failure> {
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

    // Stash recovery data survives: every journal, the patches of unrestored journals, and every patch younger than
    // an hour — a run stashing in another worktree creates its patch before it journals it (ARCHITECTURE §4 steps
    // 5–6) and takes the shared store lock only at its first repository use, so a fresh unjournaled patch may be
    // about to be restored. The journals are re-read right before each removal.
    let patches = root.join(layout::PATCHES);
    let now = SystemTime::now();
    for e in &entries(&patches) {
        let name = e.file_name();
        let bytes = name.as_encoded_bytes();
        let path = e.path();
        if is_journal(e)
            || age(&path, now).is_none_or(|a| a < ORPHAN_AGE)
            || mentioned(bytes, &protecting_journals(&patches))
        {
            continue;
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_journals_do_not_protect() {
        assert!(journal_restored(
            br#"{"v":1,"state":"restored","patch":"/p/patch1-1"}"#
        ));
        assert!(journal_restored(br#"{"state":"done"}"#));
        for protecting in [
            &br#"{"state":"stashed","patch":"/p/patch1-1"}"#[..],
            br#"{"state":"failed"}"#,
            br#"{"patch":"/p/patch1-1"}"#,
            br#"{"state":"resto"#,
            b"[[[[[[[[[[[[[[[[[[[[",
            b"",
        ] {
            assert!(!journal_restored(protecting), "{protecting:?}");
        }
        let deep: String = std::iter::repeat_n('[', 100_000).collect();
        assert!(!journal_restored(deep.as_bytes()));
    }

    #[test]
    fn journal_mentions_are_whole_names() {
        let journals =
            vec![br#"{"patch":"/s/patches/patch1700000000-42","state":"failed"}"#.to_vec()];
        assert!(mentioned(b"patch1700000000-42", &journals));
        assert!(!mentioned(b"patch1700000000-4", &journals));
        assert!(!mentioned(b"patch1700000000-421", &journals));
        assert!(!mentioned(b"patch1", &[]));
    }
}
