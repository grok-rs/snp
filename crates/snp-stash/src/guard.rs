//! The stash guard: `staged_files_only` as a journaled state machine (§4.1–§4.5).
//!
//! Owner: WP-23.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-23")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-23"
)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use snp_core::cancel::CancelToken;
use snp_core::clock::Clock;
use snp_core::failure::Failure;
use snp_core::output::Printer;
use snp_core::path::RepoPath;
use snp_git::{Git, RepoInfo};
use snp_os as _;
use snp_proc::signals::SignalGuard;
use snp_pycompat as _;

/// Owns cheap handles (`Git` and `Printer` are `Arc`-backed) so the guard has no lifetime and can live inside
/// `RunState`. `git` must be built with `Git::with_backend(ProcBackend { cancel: CancelToken::new(), role:
/// Role::Critical })`: a token that is never cancelled, so restore commands are never cut short (ADR-065). `cancel`
/// is the run token (observed only by the lock wait). `verify` = `SNP_STASH_VERIFY=1` (warn-only post-restore check,
/// §4.2 step 10e).
#[derive(Debug)]
pub struct StashCtx {
    /// The Critical-role git handle.
    pub git: Git,
    /// The repository layout.
    pub info: RepoInfo,
    /// `<store>/patches`.
    pub patches_dir: PathBuf,
    /// The output sink.
    pub printer: Arc<Printer>,
    /// The run token (lock wait only).
    pub cancel: CancelToken,
    /// Patch timestamps.
    pub clock: Arc<dyn Clock>,
    /// The signal handler (restore shield).
    pub signals: Option<Arc<SignalGuard>>,
    /// `SNP_STASH_LOCK_TIMEOUT`, default 60 s, ≤ 1 h.
    pub lock_timeout: Duration,
    /// `SNP_STASH_VERIFY=1`.
    pub verify: bool,
    /// `caps.version < 2.19`: refuse to stash (D-DATA-17).
    pub old_git: bool,
}

/// What the stash saved.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stashed {
    /// No unstaged changes.
    Nothing,
    /// The unstaged changes, saved as this patch.
    Patch(PathBuf),
}

/// The state machine; restores on drop (panic path only, via `RunState::drop`).
#[derive(Debug)]
pub struct StashGuard {
    /// W0 placeholder; WP-23 stores the state machine, the journal and the worktree lock.
    _priv: (),
}

/// `staged_files_only` entry: lock, journal, `write-tree`, `diff-index`, checkout (§4.2 steps 1–9).
pub fn enter(cx: StashCtx) -> Result<StashGuard, Failure> {
    todo!()
}

impl StashGuard {
    /// What was stashed.
    pub fn stashed(&self) -> &Stashed {
        todo!()
    }

    /// `diff-index` rc 0 ⇒ baseline digest known empty.
    pub fn diff_index_clean(&self) -> bool {
        todo!()
    }

    /// `("_SNP_STASH_LOCK", token)` for nested runs.
    pub fn child_env_token(&self) -> Option<(&'static str, OsString)> {
        todo!()
    }

    /// `stage_fixed` (§8.3): apply `--check`; journal `staging-fixed {paths}` durably; `git --literal-pathspecs add
    /// -- <paths>`; `Ok(false)` = would not apply (nothing staged).
    pub fn stage_fixed(&mut self, paths: &[RepoPath]) -> Result<bool, Failure> {
        todo!()
    }

    /// The explicit restore (§4.2 step 10).
    pub fn restore(self) -> Result<(), Failure> {
        todo!()
    }
}
