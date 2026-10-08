//! snp-owned repositories: clones, local clones, autoupdate facts, the try-repo shadow (03 §7.6, §11.2, §12).
//!
//! Owner: WP-18.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-18")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-18"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::Env;
use snp_core::failure::Failure;

use crate::{Git, GitBackend, GitExe};

/// snp-owned repositories: env = `no_git_env(base)` + `GIT_CONFIG_COUNT`{core.fsmonitor=, core.useBuiltinFSMonitor=false}.
#[derive(Clone, Debug)]
pub struct OwnedRepo {
    /// W0 placeholder; WP-18 stores exe, env, dir, backend.
    _priv: (),
}

/// autoupdate facts of one repository (03 §11.2).
#[derive(Clone, Debug)]
pub struct RevInfo {
    /// The repo URL.
    pub repo: String,
    /// The new rev.
    pub rev: String,
    /// The frozen sha (`--freeze`).
    pub frozen: Option<String>,
    /// Hook ids of the manifest at `rev`.
    pub hook_ids: Vec<String>,
}

impl OwnedRepo {
    /// pre-commit's exact clone sequence (03 §7.6) + one object-format retry on `mismatched algorithms` (D-SEC-06).
    /// `CloneMode::Foreground` = pre-commit's env and prompts; `Prefetch` (background attempts only, ADR-064) adds
    /// `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=` (empty: no askpass GUI), `GCM_INTERACTIVE=never` and, for ssh/scp URLs,
    /// `GIT_SSH_COMMAND='ssh -o BatchMode=yes'`; its stderr is never surfaced.
    pub fn clone_rev(
        exe: Arc<GitExe>,
        base: &Env,
        dest: &Path,
        repo: &str,
        rev: &str,
        mode: CloneMode,
        b: Arc<dyn GitBackend>,
    ) -> Result<Self, Failure> {
        todo!()
    }

    /// `git clone --local --no-checkout <src> <dest>` + checkout `<rev>` (+ shallow submodules); falls back to
    /// `clone_rev`.
    pub fn clone_local(
        exe: Arc<GitExe>,
        base: &Env,
        src: &Path,
        dest: &Path,
        rev: &str,
        b: Arc<dyn GitBackend>,
    ) -> Result<Self, Failure> {
        todo!()
    }

    /// An existing owned checkout.
    pub fn open(exe: Arc<GitExe>, base: &Env, dir: &Path, b: Arc<dyn GitBackend>) -> Self {
        todo!()
    }

    /// `git ls-tree <rev> <dir>/`: (path, blob sha) pairs (Tier B).
    pub fn ls_tree_blobs(&self, rev: &str, dir: &str) -> Result<Vec<(String, String)>, Failure> {
        todo!()
    }

    /// `git cat-file blob <rev>:<path>` (Tier B).
    pub fn read_blob(&self, rev: &str, path: &str) -> Result<Vec<u8>, Failure> {
        todo!()
    }

    /// autoupdate 03 §11.2.
    pub fn rev_info(&self, repo: &str, tags_only: bool, freeze: bool) -> Result<RevInfo, Failure> {
        todo!()
    }

    /// try-repo 03 §12: (shadow repo path, ref).
    pub fn shadow_try_repo(&self, user: &Git, tmp: &Path) -> Result<(PathBuf, String), Failure> {
        todo!()
    }
}

/// Foreground clone (prompts allowed) or background prefetch (non-interactive).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CloneMode {
    /// pre-commit's env and prompts.
    Foreground,
    /// Background attempt, non-interactive (ADR-064).
    Prefetch,
}

/// Background prefetch is allowed for https://, file:// and local paths; for ssh/scp URLs only when none of
/// `GIT_SSH_COMMAND`, `GIT_SSH` and `core.sshCommand` is set (`BatchMode` can then be injected); never otherwise
/// (ADR-064).
pub fn prefetch_eligible(url: &str, env: &Env, core_ssh_command_set: bool) -> bool {
    todo!()
}

/// git.py:27-48 allowlist (Windows: names upper-cased, §2.5).
pub fn no_git_env(env: &Env) -> Env {
    todo!()
}
