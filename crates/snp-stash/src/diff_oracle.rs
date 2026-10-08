//! Modification detection (§3.7, ADR-027, ADR-078).
//!
//! Owner: WP-23.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-23")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-23"
)]

use std::path::PathBuf;
use std::sync::Arc;

use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_git::{DiffFingerprint, Git, IndexEntries, RepoInfo};

/// How modifications are detected.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiffMode {
    /// stat snapshots, a real `git diff` only when something changed.
    Stat,
    /// One `git diff` per hook (pre-commit's way).
    Git,
}

/// pre-commit's `_get_diff` comparison, made cheap.
#[derive(Debug)]
pub struct DiffOracle {
    /// W0 placeholder; WP-23 stores the mode, the last `DiffFingerprint` and the snapshot (stats + diff-input
    /// sources + racy hashes).
    _priv: (),
}

/// Whether a hook changed the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Change {
    /// Same diff as before the hook.
    Unchanged,
    /// The diff changed (the hook "modified files").
    Changed,
}

impl DiffOracle {
    /// Stat mode only on Unix, `stat_oracle_safe` filesystems, ≤ 16 384 index entries, no `attr.tree` /
    /// `GIT_ATTR_SOURCE`, `SNP_DIFF_MODE != "git"`, and `tracked` + `origins` supplied by the step-6 fan-out (no
    /// extra spawn). `None` for either ⇒ Git mode.
    pub fn new(
        git: &Git,
        info: &RepoInfo,
        env: &Env,
        baseline: Option<DiffFingerprint>,
        tracked: Option<Arc<IndexEntries>>,
        origins: Option<Arc<[PathBuf]>>,
    ) -> Result<Self, Failure> {
        todo!()
    }

    /// The chosen mode.
    pub fn mode(&self) -> DiffMode {
        todo!()
    }

    /// Before a hook that may modify files (stat snapshot, or nothing in Git mode).
    pub fn before_hook(&mut self) -> Result<(), Failure> {
        todo!()
    }

    /// After it: `Unchanged` without spawning when provably identical, else one real `git diff` and digest compare.
    pub fn after_hook(&mut self) -> Result<Change, Failure> {
        todo!()
    }

    /// Native fixer reported exact modified paths (09 §6.7 caveat rules evaluated by the caller).
    pub fn after_native(
        &mut self,
        modified_tracked: bool,
        needs_real_diff: bool,
    ) -> Result<Change, Failure> {
        todo!()
    }

    /// Show-diff gate (last digest).
    pub fn non_empty(&mut self) -> Result<bool, Failure> {
        todo!()
    }
}
