//! `repo: builtin` ⇄ `pre-commit/pre-commit-hooks` conversion (09 §7.1).
//!
//! Owner: WP-34.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-34")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-34")]

use snp_core::failure::Failure;

/// `snp migrate builtin`: pre-commit-hooks entries → `repo: builtin` (09 §7.1).
pub fn to_builtin(config_bytes: &[u8]) -> Result<Vec<u8>, Failure> {
    todo!()
}

/// `snp migrate upstream`: `repo: builtin` → pre-commit-hooks entries.
pub fn to_upstream(config_bytes: &[u8]) -> Result<Vec<u8>, Failure> {
    todo!()
}
