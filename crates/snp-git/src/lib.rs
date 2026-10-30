//! Every git invocation with pre-commit's exact argv, the `GitBackend` seam (+ `ScriptedBackend`), discovery,
//! version policy, pre-push and `MERGE_MSG` parsing, owned repositories (clone, local clone, autoupdate, try-repo
//! shadow), pre-commit-hooks git facts and the hook-script classifier (ARCHITECTURE §1.4.13, 02 G.0–G.11).
//!
//! Two kinds of call, never mixed (02 G.10): [`Git`] works on the user's repository with the run environment and
//! pre-commit's argv (plus only `--no-renames` on the intent-to-add and staged-file queries and `GIT_DIFF_OPTS`
//! removed from the stash's `diff-index`, 02 G.7 rows 1–3); [`OwnedRepo`] works on snp's own clones and temporary
//! repositories with `no_git_env` and the fsmonitor injection (02 G.7 rows 4–7).
//!
//! Owner: WP-18.

mod backend;
mod exe;
mod git;
pub mod hookscripts;
mod messages;
mod owned;
pub mod pch_facts;
mod run;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod types;
mod zlist;

pub use backend::{GitBackend, ProcBackend};
pub use exe::{GitCaps, GitExe, GitVersion};
pub use git::Git;
pub use owned::{CloneMode, OwnedRepo, RevInfo, no_git_env, prefetch_eligible};
pub use types::{
    DiffFingerprint, DiffIndex, IndexEntries, MergeState, ObjectId, PrePushNs, RepoInfo,
};
pub use zlist::ZList;
