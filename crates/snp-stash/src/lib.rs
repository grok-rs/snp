//! The staged-files stash guard (journaled state machine, worktree lock, isolated restore, recovery) and the
//! `DiffOracle` modification detector (ARCHITECTURE §1.4.18, §4, §3.7; pre-commit's `staged_files_only.py`).
//!
//! Owner: WP-23.

pub mod diff_oracle;
mod guard;
mod journal;
mod lock;
mod messages;
mod recovery;

pub use guard::{StashCtx, StashGuard, Stashed, enter};
pub use recovery::{
    Dismissed, Unrestored, dismiss_unrestored, find_unrestored, recovery_report, recovery_scan,
    verify_restored,
};
