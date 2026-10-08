//! Deterministic repository builder.
//!
//! Owner: WP-18.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-18")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-18")]

use std::path::PathBuf;

use snp_git as _;

/// Deterministic repository builder (identical SHAs across runs: fixed dates, author, umask, core.autocrlf=false).
#[derive(Debug)]
pub struct RepoBuilder {
    /// W0 placeholder; WP-18 stores the repository path, the env and the pending operations.
    _priv: (),
}

impl RepoBuilder {
    /// Writes a regular file.
    #[must_use]
    pub fn file(self, path: &str, bytes: &[u8]) -> Self {
        todo!()
    }

    /// Writes an executable file (mode 100755 in the index on every OS).
    #[must_use]
    pub fn exec_file(self, path: &str, bytes: &[u8]) -> Self {
        todo!()
    }

    /// Creates a symlink.
    #[must_use]
    pub fn symlink(self, path: &str, target: &str) -> Self {
        todo!()
    }

    /// `git add -- <path>`.
    #[must_use]
    pub fn stage(self, path: &str) -> Self {
        todo!()
    }

    /// `git add -A`.
    #[must_use]
    pub fn stage_all(self) -> Self {
        todo!()
    }

    /// `git commit -m <msg>` with the next fixed date.
    #[must_use]
    pub fn commit(self, msg: &str) -> Self {
        todo!()
    }

    /// `git tag <name>`.
    #[must_use]
    pub fn tag(self, name: &str) -> Self {
        todo!()
    }

    /// `git config <key> <value>`.
    #[must_use]
    pub fn config(self, key: &str, value: &str) -> Self {
        todo!()
    }

    /// Runs the pending operations; returns the repository path.
    pub fn build(self) -> PathBuf {
        todo!()
    }
}
