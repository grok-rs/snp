//! Dev-only hermetic test environment (10 §7.3): binaries under test, the hermetic `TestEnv`, a deterministic git
//! fixture builder, oracle and binary runners and the insta filters (ARCHITECTURE §1.4.14).
//!
//! Every child gets a cleared environment plus the 10 §7.3.2 allow-list (no `SKIP`, `PRE_COMMIT_*`, `GIT_*`,
//! `VIRTUAL_ENV` leak in), a canonicalised temp root holding `HOME`, both stores and an empty git template dir, pinned
//! git identity/config and fixed commit dates, so the same builder script yields the same SHAs on every run. The real
//! `~/.cache/pre-commit` and `~/.cache/snp` are never touched (the guard test in `tests/guard.rs`).
//!
//! Owner: WP-18.

mod bins;
mod cmd;
mod env;
mod fixture;
mod harness;
mod repo;
mod snapshot;

pub use bins::Bins;
pub use cmd::{Cmd, Output};
pub use env::{Oracle, TestEnv};
pub use fixture::Fixture;
pub use repo::RepoBuilder;
#[doc(hidden)]
pub use snapshot::__render;

/// insta, re-exported for [`snp_snapshot!`] (callers need no insta dependency of their own).
#[doc(hidden)]
pub use insta as __insta;

// The DAG edge snp-testkit → snp-git (ARCHITECTURE §1.2) is reserved for git-aware helpers; anchor it for machete.
use snp_git as _;
