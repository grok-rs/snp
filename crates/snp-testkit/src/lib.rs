//! Dev-only hermetic test environment (10 §7.3): binaries under test, the hermetic `TestEnv`, a deterministic git
//! fixture builder, oracle and binary runners and the insta filters (ARCHITECTURE §1.4.14).
//!
//! Owner: WP-18.

mod bins;
mod cmd;
mod env;
mod fixture;
mod repo;
mod snapshot;

pub use bins::Bins;
pub use cmd::{Cmd, Output};
pub use env::{Oracle, TestEnv};
pub use fixture::Fixture;
pub use repo::RepoBuilder;
