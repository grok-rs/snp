//! The hermetic test environment (10 §7.3.2).
//!
//! Owner: WP-18.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-18")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-18"
)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use snp_core::env::Env;

use crate::{Bins, Cmd, Fixture, RepoBuilder};

/// Canonicalised temp root, hermetic env (10 §7.3.2), fixed `GIT_*_DATE` counter.
#[derive(Debug)]
pub struct TestEnv {
    /// W0 placeholder; WP-18 stores the temp root, the env, the bins and the date counter.
    _priv: (),
}

impl TestEnv {
    /// A fresh hermetic environment.
    pub fn new(bins: Bins) -> Self {
        todo!()
    }

    /// The canonicalised temp root.
    pub fn root(&self) -> &Path {
        todo!()
    }

    /// Child env only; never the process env.
    pub fn env(&self) -> &Env {
        todo!()
    }

    /// Derived env (same root).
    #[must_use]
    pub fn with_env(&self, k: &str, v: impl Into<OsString>) -> Self {
        todo!()
    }

    /// A deterministic repository builder under the root.
    pub fn git_repo(&self, name: &str) -> RepoBuilder {
        todo!()
    }

    /// Recreation of a pre-commit testing/resources repo.
    pub fn fixture_repo(&self, fixture: Fixture) -> PathBuf {
        todo!()
    }

    /// `bins.snp` (snp brand).
    pub fn snp(&self, cwd: &Path) -> Cmd {
        todo!()
    }

    /// Through a `pre-commit` symlink/copy (compat brand).
    pub fn snp_as_pre_commit(&self, cwd: &Path) -> Cmd {
        todo!()
    }

    /// `SNP_ORACLE_DIR`/target/oracle; never PATH.
    pub fn oracle(&self, which: Oracle, cwd: &Path) -> Option<Cmd> {
        todo!()
    }

    /// insta filters (10 §7.3.4).
    pub fn filters(&self) -> Vec<(String, String)> {
        todo!()
    }

    /// `bins.probe`.
    pub fn probe(&self) -> &Path {
        todo!()
    }
}

/// The pre-commit 4.6.2 oracle venvs built by `cargo xtask oracle install`.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Oracle {
    /// `target/oracle/pc462-py312` (CPython 3.12.3).
    Pc462Py312,
    /// `target/oracle/pc462-py313` (CPython ≥ 3.13.14).
    Pc462Py313,
}
