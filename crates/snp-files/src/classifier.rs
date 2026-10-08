//! `Classifier` (`commands/run.py`): top-level include/exclude, then per-hook filtering.
//!
//! Owner: WP-22.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-22")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-22"
)]

use std::sync::Arc;

use snp_core::failure::Failure;
use snp_proc::pool::JobPool;
use snp_pyre::PyRegex;

use crate::{FileId, FileTable, HookFilter};

/// `Classifier.from_config`: top-level include/exclude (`re.search` on the `/`-path), then lexists.
#[derive(Debug)]
pub struct Classifier {
    /// The run's file table.
    pub table: Arc<FileTable>,
    /// Files kept by the top-level filters, in table order.
    pub kept: Vec<FileId>,
}

impl Classifier {
    /// Nested pool use runs inline (§1.4.8).
    pub fn new(
        table: Arc<FileTable>,
        include: &PyRegex,
        exclude: &PyRegex,
        pool: &JobPool,
    ) -> Result<Self, Failure> {
        todo!()
    }

    /// `filenames_for_hook`: order & duplicates kept; Vanished → dropped (X13-CLS-01); Unreadable → Err
    /// (X13-CLS-02, exit 3); a regex backtrack-limit hit → `FatalError` naming pattern and path (§3.3).
    pub fn for_hook(&self, f: &HookFilter, pool: &JobPool) -> Result<Vec<FileId>, Failure> {
        todo!()
    }

    /// check-useless-excludes.
    pub fn by_types(&self, ids: &[FileId], f: &HookFilter) -> Result<Vec<FileId>, Failure> {
        todo!()
    }
}
