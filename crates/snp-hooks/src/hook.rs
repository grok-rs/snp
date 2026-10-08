//! The native-hook contract (frozen in §1.4.19; ADR-071, ADR-072).
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::path::RepoPath;
use snp_git::Git;
use snp_git::pch_facts::PchFacts;
use snp_hooks_data::Epoch;

/// What a native hook sees of the run.
#[derive(Debug)]
pub struct HookCx<'a> {
    /// The work-tree root (the hook's cwd).
    pub root: &'a Path,
    /// The hook's environment.
    pub env: &'a Env,
    /// The user repository.
    pub git: &'a Git,
    /// The colour decision.
    pub color: bool,
    /// The pre-commit-hooks epoch the behaviour follows.
    pub epoch: Epoch,
    /// Windows text-mode semantics.
    pub windows: bool,
    /// argparse's column count ([`crate::columns`]).
    pub columns: usize,
    /// Memoised pre-commit-hooks git facts.
    pub facts: &'a PchFacts,
    /// Checked between files; on cancellation a native returns early and the runner reports `Interrupted`.
    pub cancel: &'a CancelToken,
}

/// The result of one partition (what the console script's process would have produced).
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// The exit status.
    pub code: i32,
    /// stdout + stderr in Python's flush order.
    pub output: Vec<u8>,
    /// Files the hook modified (exact, §3.7).
    pub modified: Vec<RepoPath>,
    /// A Python traceback ended the partition.
    pub crashed: bool,
    /// The hook never writes files.
    pub reads_only: bool,
}

/// A pre-commit-hooks console script (or builtin variant) implemented natively.
pub trait NativeHook: Send + Sync + 'static {
    /// The manifest id.
    fn id(&self) -> &'static str;
    /// argparse `prog` = console-script name.
    fn prog(&self) -> &'static str;
    /// Static pre-check of `args` (unsupported regex etc.) → `Err(reason)` ⇒ caller DELEGATES (09 §6.4 G5).
    fn preflight(&self, epoch: Epoch, args: &[Arc<str>]) -> Result<(), String>;
    /// One xargs partition. `argv` is exactly the console script's `sys.argv[1:]` — `args` followed by the
    /// partition's files in shuffled partition order — and is parsed by argparse emulation
    /// (`framework::parse_args`), so a file named `-x.py` or `-i` behaves exactly as under Python (unrecognized
    /// argument / flag; ADR-072). Effects committed in input order; the first crash stops the partition (09 §2.6).
    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome;
    /// Whether `Outcome::modified` is exact (otherwise the runner diffs).
    fn reports_modifications_exactly(&self) -> bool {
        true
    }
}
