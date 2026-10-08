//! Journal recovery of runs that never restored (§4.6, ADR-067).
//!
//! Owner: WP-23.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-23")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-23")]

use std::path::{Path, PathBuf};

use snp_core::failure::Failure;
use snp_core::path::RepoPath;
use snp_git::Git;

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
    pub fn hint(&self) -> String {
        todo!()
    }
}

/// Liveness is the worktree lock, never a pid (ADR-067): call only while holding `<git_dir>/snp-stash.lock` (or
/// after a successful non-blocking probe of it). Reads the git-dir pointer `<git_dir>/snp-stash.journal` (works with
/// any store) and the journals in `patches_dir` for this worktree. Never auto-applied.
pub fn find_unrestored(
    git_dir: &Path,
    patches_dir: &Path,
    worktree_root: &Path,
) -> Vec<Unrestored> {
    todo!()
}

/// Non-blocking: try the worktree lock, scan, release (run/hook-impl start; skipped for nested runs and when held).
pub fn recovery_scan(git_dir: &Path, patches_dir: &Path, worktree_root: &Path) -> Vec<Unrestored> {
    todo!()
}

/// `snp doctor --stash` / `SNP_STASH_VERIFY` (warn-only, 06's verify): `git apply --check --reverse <patch>` against
/// the worktree after a restore (true = the patch's changes are present).
pub fn verify_restored(git: &Git, patch: &Path) -> Result<bool, Failure> {
    todo!()
}
