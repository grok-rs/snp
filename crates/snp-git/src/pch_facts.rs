//! Per-run cache of the git facts pre-commit-hooks compute, with their exact upstream argv (non `-z`, quotePath and
//! renames honoured: bug-compat) and the hook env (09 §3, §9.6). Used by snp-hooks natives.
//!
//! Owner: WP-18.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-18")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-18"
)]

use std::ffi::OsStr;
use std::sync::Arc;

use snp_core::failure::Failure;
use snp_proc::spawn::Captured;

use crate::Git;

/// Per-run cache of pre-commit-hooks git facts.
#[derive(Debug)]
pub struct PchFacts {
    /// W0 placeholder; WP-18 stores the `Git` and `OnceLock`s.
    _priv: (),
}

impl PchFacts {
    /// A cache over `git`.
    pub fn new(git: Git) -> Self {
        todo!()
    }

    /// `diff --staged --name-only --diff-filter=A`.
    pub fn added_files(&self) -> Result<Arc<[Vec<u8>]>, Failure> {
        todo!()
    }

    /// `ls-files` (quoted, non `-z`).
    pub fn tracked_files(&self) -> Result<Arc<[Vec<u8>]>, Failure> {
        todo!()
    }

    /// `ls-files --stage`.
    #[expect(clippy::type_complexity, reason = "frozen §1.4.13 signature")]
    pub fn index_modes(&self) -> Result<Arc<[(u32, Vec<u8>)]>, Failure> {
        todo!()
    }

    /// `check-attr filter` (09 §4.E).
    pub fn lfs_files(&self, paths: &[&[u8]]) -> Result<Vec<Vec<u8>>, Failure> {
        todo!()
    }

    /// `symbolic-ref HEAD`.
    pub fn current_branch(&self) -> Result<Option<String>, Failure> {
        todo!()
    }

    /// Any other catalogued call.
    pub fn raw(&self, args: &[&OsStr]) -> Result<Captured, Failure> {
        todo!()
    }
}
