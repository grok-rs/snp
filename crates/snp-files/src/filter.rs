//! A hook's compiled filter (`files`, `exclude`, `types`, `types_or`, `exclude_types`, prefix).
//!
//! Owner: WP-22.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-22")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-22"
)]

use std::sync::Arc;

use file_identify::TagSet;
use snp_core::path::RepoPath;
use snp_pyre::{PyReError, PyRegex, RegexCache};

use crate::Needs;

/// `Classifier.filenames_for_hook`'s inputs, compiled.
#[derive(Debug, Clone)]
pub struct HookFilter {
    /// `files` (`re.search`).
    pub files: Arc<PyRegex>,
    /// `exclude` (`re.search`).
    pub exclude: Arc<PyRegex>,
    /// `types`.
    pub types: TagSet,
    /// `types_or`.
    pub types_or: TagSet,
    /// `exclude_types`.
    pub exclude_types: TagSet,
    /// The identify tiers the type filters need.
    pub needs: Needs,
    /// Boundary-aware path prefix (the `root` extension; the v1.1 workspace project).
    pub prefix: Option<RepoPath>,
}

impl HookFilter {
    /// Five filter fields as plain arguments (snp-files does not depend on snp-config) plus the boundary-aware path
    /// `prefix` (the `root` extension in v1; the workspace project prefix in v1.1): files outside it never match.
    pub fn compile(
        files: &str,
        exclude: &str,
        types: TagSet,
        types_or: TagSet,
        exclude_types: TagSet,
        prefix: Option<RepoPath>,
        cache: &RegexCache,
    ) -> Result<Self, PyReError> {
        todo!()
    }
}
