//! `migrate-config` (`commands/migrate_config.py`; 01 §10, D-DATA-09/14/15).
//!
//! Owner: WP-21.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-21")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-21")]

use std::ffi::OsStr;

use crate::ConfigError;

/// What `migrate_config` did.
#[derive(Debug)]
#[non_exhaustive]
pub enum Outcome {
    /// The new file bytes (`Configuration has been migrated.`).
    Migrated(Vec<u8>),
    /// Nothing to do (`Configuration is already migrated.`).
    AlreadyMigrated,
}

/// `migrate_config(config_file)`: top-level list → `repos:`, `sha:` → `rev:`, legacy stage names, `python_venv`.
pub fn migrate_config(bytes: &[u8], display: &OsStr) -> Result<Outcome, ConfigError> {
    todo!()
}
