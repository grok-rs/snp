//! autoupdate's `_original_lines` + `_write_new_config` (`commands/autoupdate.py`; 03 §11.3, 15 §A4.6).
//!
//! Owner: WP-21.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-21")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-21")]

use std::sync::Arc;

use crate::ConfigError;

/// One repo's new `rev` (and `# frozen: …` comment).
#[derive(Debug, Clone)]
pub struct RevUpdate {
    /// Index into `repos`.
    pub repo_index: usize,
    /// The new rev.
    pub new_rev: Arc<str>,
    /// `# frozen: <tag>` when `--freeze`.
    pub frozen_comment: Option<Arc<str>>,
}

/// The rewritten file.
#[derive(Debug)]
#[non_exhaustive]
pub enum Rewrite {
    /// Line-preserving rewrite of the `rev:` lines.
    Lines(Vec<u8>),
    /// The PyYAML re-dump path (the rev lines could not be located).
    FullReformat(Vec<u8>),
}

/// Rewrites the `rev:` lines of `original` (EOL-preserving, idempotent).
pub fn rewrite(original: &[u8], updates: &[RevUpdate]) -> Result<Rewrite, ConfigError> {
    todo!()
}
