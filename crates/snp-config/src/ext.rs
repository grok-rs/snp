//! snp's lefthook-inspired extension keys (§8, COMPATIBILITY §7).
//!
//! Owner: WP-21.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-21")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-21")]

use std::sync::Arc;
use std::time::Duration;

use snp_core::path::RepoPath;

/// Hook-level extension keys (§8.2).
#[derive(Debug, Clone, Default)]
pub struct HookExt {
    /// `priority`.
    pub priority: Option<Priority>,
    /// `env` (literal values, applied after language activation).
    pub env: Vec<(Arc<str>, Arc<str>)>,
    /// `groups`.
    pub groups: Vec<Arc<str>>,
    /// `stage_fixed` (§8.3).
    pub stage_fixed: Option<StageFixed>,
    /// `skip` conditions (list items OR).
    pub skip: Vec<Condition>,
    /// `only` conditions (list items OR).
    pub only: Vec<Condition>,
    /// `timeout` (Go duration).
    pub timeout: Option<Duration>,
    /// `fail_text`.
    pub fail_text: Option<Arc<str>>,
    /// `root` (relative directory).
    pub root: Option<RepoPath>,
    /// `use_stdin`.
    pub use_stdin: bool,
    /// `fail_on_changes`.
    pub fail_on_changes: Option<FailOnChanges>,
}

/// Root-level extension keys (§8.2).
#[derive(Debug, Clone)]
pub struct RootExt {
    /// `minimum_snp_version`.
    pub minimum_snp_version: Option<Arc<str>>,
    /// `priorities` (alias → value).
    pub priorities: indexmap::IndexMap<Arc<str>, u32>,
    /// `fail_on_changes`.
    pub fail_on_changes: FailOnChanges,
    /// `skip` conditions.
    pub skip: Vec<Condition>,
    /// `only` conditions.
    pub only: Vec<Condition>,
    /// `allow_disable` (`false` ⇒ `SNP=0` is ignored).
    pub allow_disable: bool,
    /// `fast_path`.
    pub fast_path: FastPath,
    /// `fast_path_exclude` (hook ids).
    pub fast_path_exclude: Vec<Arc<str>>,
    /// `fast_path_coalesce`.
    pub fast_path_coalesce: bool,
    /// `output`.
    pub output: OutputMode,
}

impl Default for RootExt {
    /// `allow_disable: true`, `fast_path: Auto`, `fail_on_changes: Always`, `output: Default`, everything else empty.
    fn default() -> Self {
        Self {
            minimum_snp_version: None,
            priorities: indexmap::IndexMap::new(),
            fail_on_changes: FailOnChanges::Always,
            skip: Vec::new(),
            only: Vec::new(),
            allow_disable: true,
            fast_path: FastPath::Auto,
            fast_path_exclude: Vec::new(),
            fast_path_coalesce: false,
            output: OutputMode::Default,
        }
    }
}

/// `priority`: a number or an alias from `priorities`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Priority {
    /// A literal priority (lower runs first).
    Value(u32),
    /// A name from the root `priorities` map.
    Alias(Arc<str>),
}

/// `stage_fixed` (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StageFixed {
    /// `true` / `verify`: re-run once before staging a failed fixer's changes.
    Verify,
    /// `trust`: stage without the re-run.
    Trust,
}

/// `fail_on_changes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FailOnChanges {
    /// pre-commit's behaviour.
    Always,
    /// Modifications never fail the hook.
    Never,
    /// Fail only when `CI` is truthy.
    Ci,
    /// Fail only when `CI` is not truthy.
    NonCi,
}

/// `fast_path` (§7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FastPath {
    /// Never run pre-commit-hooks natively.
    Off,
    /// NATIVE hooks only.
    Exact,
    /// NATIVE plus gated NATIVE-WITH-CAVEATS hooks (default).
    Auto,
}

/// `output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputMode {
    /// pre-commit's output.
    Default,
    /// Footer with wall time, durations and counts.
    Summary,
    /// Failures only.
    Quiet,
}

/// One `skip` / `only` condition; keys of one map AND (`All`), list items OR.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Condition {
    /// A merge is in progress.
    Merge,
    /// A rebase is in progress.
    Rebase,
    /// The commit being created is a merge commit.
    MergeCommit,
    /// A cherry-pick is in progress.
    CherryPick,
    /// A revert is in progress.
    Revert,
    /// A bisect is in progress.
    Bisect,
    /// `{ref: glob}` against the current branch.
    Ref(Arc<str>),
    /// `{env: NAME}`: the variable is non-empty.
    Env(Arc<str>),
    /// `{ci: bool}`.
    Ci(bool),
    /// `{run: cmd}`: `sh -c` exits 0 (30 s, cached per run).
    Run(Arc<str>),
    /// Every condition holds (keys of one map).
    All(Vec<Self>),
}

/// Go's `time.ParseDuration` (lefthook `timeout` values, 05 §18).
pub fn parse_go_duration(s: &str) -> Result<Duration, String> {
    todo!()
}
