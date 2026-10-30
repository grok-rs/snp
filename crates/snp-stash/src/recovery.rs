//! Journal recovery of runs that never restored (§4.6, ADR-067).
//!
//! snp-only (pre-commit keeps no journal: a killed run leaves `patch<ts>-<pid>` in the store and says nothing,
//! D-DATA-13). Liveness is the worktree lock, never a pid. Nothing here applies a patch: the user decides.
//!
//! Owner: WP-23.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use snp_core::failure::Failure;
use snp_core::path::RepoPath;
use snp_git::Git;

use crate::journal::{self, JournalData, PointerData};
use crate::{lock, messages};

/// A journaled run that never restored (§4.6). Recovery hints use `git apply --3way` (valid: `--binary` patches
/// carry full index lines and their preimage blobs are in the object store via write-tree).
#[derive(Debug, Clone)]
pub struct Unrestored {
    /// The saved patch.
    pub patch: PathBuf,
    /// The `write-tree` of the index at stash time.
    pub tree: Option<String>,
    /// The worktree it belongs to.
    pub worktree: PathBuf,
    /// The pid that wrote the journal (informational; liveness is the worktree lock).
    pub pid: u32,
    /// The last journaled state.
    pub state: Box<str>,
    /// Paths staged by `stage_fixed` before the run died.
    pub staged_fixed: Vec<RepoPath>,
}

impl Unrestored {
    /// The exact stderr notice (§4.6).
    ///
    /// `snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in <patch> (index
    /// tree <tree>). Restore with: git apply --3way <patch>`, with `git reset -q <tree> -- <paths…> && ` (or
    /// `git read-tree <tree> && ` for a long list) before the apply in the `staging-fixed` state.
    pub fn hint(&self) -> String {
        let staged: &[RepoPath] = if &*self.state == "staging-fixed" {
            &self.staged_fixed
        } else {
            &[]
        };
        messages::recovery_notice(&self.patch, self.tree.as_deref(), staged)
    }
}

/// What a scan found: orphaned runs, and files that only describe finished or never-stashed runs.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    /// Runs whose patch was never restored.
    pub(crate) unrestored: Vec<Unrestored>,
    /// Journals and pointers of this worktree that protect nothing (restored, or no patch): safe to delete while
    /// holding the lock.
    pub(crate) stale: Vec<PathBuf>,
}

/// The pointers in a git dir: `snp-stash.journal` and `snp-stash-<n>.journal`.
fn pointers(git_dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(git_dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n == journal::POINTER_NAME
                || n.strip_prefix("snp-stash-")
                    .and_then(|r| r.strip_suffix(".journal"))
                    .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// The `*.snp-journal` files of a patches directory.
fn store_journals(patches_dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(patches_dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with("run-") && n.ends_with(journal::JOURNAL_SUFFIX)
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// One run as recorded by its journal and/or pointer.
#[derive(Debug, Default)]
struct Run {
    data: Option<JournalData>,
    pointer: Option<PointerData>,
    files: Vec<PathBuf>,
}

impl Run {
    fn state(&self) -> String {
        // The journal is written before the pointer: it is the newer record when both exist.
        self.data
            .as_ref()
            .map(|d| d.state.clone())
            .or_else(|| self.pointer.as_ref().map(|p| p.state.clone()))
            .unwrap_or_default()
    }

    fn patch(&self) -> Option<PathBuf> {
        self.data
            .as_ref()
            .and_then(|d| d.patch.clone())
            .or_else(|| self.pointer.as_ref().and_then(|p| p.patch.clone()))
    }

    fn tree(&self) -> Option<String> {
        self.data
            .as_ref()
            .and_then(|d| d.tree.clone())
            .or_else(|| self.pointer.as_ref().and_then(|p| p.tree.clone()))
    }

    fn staged_fixed(&self) -> Vec<RepoPath> {
        let raw: &[Vec<u8>] = match (&self.data, &self.pointer) {
            (Some(d), _) if !d.staged_fixed.is_empty() => &d.staged_fixed,
            (_, Some(p)) => &p.staged_fixed,
            (Some(d), None) => &d.staged_fixed,
            (None, None) => &[],
        };
        raw.iter().filter_map(|b| RepoPath::new(b)).collect()
    }
}

/// Same path, compared literally (both sides come from the same discovery).
fn same_path(a: &Path, b: &Path) -> bool {
    a == b || a.as_os_str() == b.as_os_str()
}

/// Reads every pointer of `git_dir` and every journal of `patches_dir` that belongs to `worktree_root`.
pub(crate) fn scan(git_dir: &Path, patches_dir: &Path, worktree_root: &Path) -> Scan {
    // Keyed by the journal path (or the pointer path for a store-less run).
    let mut runs: BTreeMap<PathBuf, Run> = BTreeMap::new();
    for p in pointers(git_dir) {
        let Some(ptr) = journal::read_small(&p).and_then(|b| PointerData::parse(&b)) else {
            continue;
        };
        if let Some(w) = &ptr.worktree
            && !same_path(w, worktree_root)
        {
            continue;
        }
        let key = ptr.journal.clone().unwrap_or_else(|| p.clone());
        let data = ptr
            .journal
            .as_deref()
            .and_then(journal::read_small)
            .and_then(|b| JournalData::parse(&b));
        let run = runs.entry(key).or_default();
        run.files.push(p);
        if let Some(j) = &ptr.journal
            && data.is_some()
        {
            run.files.push(j.clone());
        }
        if run.data.is_none() {
            run.data = data;
        }
        run.pointer = Some(ptr);
    }
    for j in store_journals(patches_dir) {
        let Some(data) = journal::read_small(&j).and_then(|b| JournalData::parse(&b)) else {
            continue;
        };
        if !same_path(&data.worktree, worktree_root) {
            continue;
        }
        let run = runs.entry(j.clone()).or_default();
        if !run.files.contains(&j) {
            run.files.push(j);
        }
        if run.data.is_none() {
            run.data = Some(data);
        }
    }
    let mut out = Scan::default();
    for run in runs.into_values() {
        let state = run.state();
        let patch = run.patch().filter(|p| p.is_file());
        match patch {
            Some(patch) if journal::is_unrestored(&state) => out.unrestored.push(Unrestored {
                patch,
                tree: run.tree(),
                worktree: worktree_root.to_path_buf(),
                pid: run.data.as_ref().map_or(0, |d| d.pid),
                staged_fixed: run.staged_fixed(),
                state: state.into_boxed_str(),
            }),
            _ => out.stale.extend(run.files),
        }
    }
    out.unrestored.sort_by(|a, b| a.patch.cmp(&b.patch));
    out
}

/// Liveness is the worktree lock, never a pid (ADR-067): call only while holding `<git_dir>/snp-stash.lock` (or
/// after a successful non-blocking probe of it). Reads the git-dir pointer `<git_dir>/snp-stash.journal` (works with
/// any store) and the journals in `patches_dir` for this worktree. Never auto-applied.
///
/// Every journal whose last state is `stashed`, `staging-fixed`, `rolling-back` or `failed` and whose patch still
/// exists is reported, ordered by patch path; unreadable or foreign files are ignored.
pub fn find_unrestored(
    git_dir: &Path,
    patches_dir: &Path,
    worktree_root: &Path,
) -> Vec<Unrestored> {
    scan(git_dir, patches_dir, worktree_root).unrestored
}

/// Non-blocking: try the worktree lock, scan, release (run/hook-impl start; skipped for nested runs and when held).
///
/// Empty when another run holds the lock (it owns the journals it is writing). The lock token is not written: the
/// probe never exempts a nested run.
pub fn recovery_scan(git_dir: &Path, patches_dir: &Path, worktree_root: &Path) -> Vec<Unrestored> {
    match lock::try_probe(git_dir) {
        Some(held) => {
            let found = find_unrestored(git_dir, patches_dir, worktree_root);
            drop(held);
            found
        }
        None => Vec::new(),
    }
}

/// `snp doctor --stash` / `SNP_STASH_VERIFY` (warn-only, 06's verify): `git apply --check --reverse <patch>` against
/// the worktree after a restore (true = the patch's changes are present).
pub fn verify_restored(git: &Git, patch: &Path) -> Result<bool, Failure> {
    let c = git.run_raw(&[
        OsStr::new("apply"),
        OsStr::new("--check"),
        OsStr::new("--reverse"),
        patch.as_os_str(),
    ])?;
    Ok(c.status.0 == 0)
}
