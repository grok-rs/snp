//! Every git invocation with pre-commit's exact argv, the `GitBackend` seam (+ `ScriptedBackend`), discovery,
//! version policy, pre-push and `MERGE_MSG` parsing, owned repositories (clone, local clone, autoupdate, try-repo
//! shadow), pre-commit-hooks git facts and the hook-script classifier (ARCHITECTURE §1.4.13, 02 G.0–G.11).
//!
//! Owner: WP-18.

mod backend;
mod exe;
mod git;
pub mod hookscripts;
mod owned;
pub mod pch_facts;
#[cfg(feature = "testing")]
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
