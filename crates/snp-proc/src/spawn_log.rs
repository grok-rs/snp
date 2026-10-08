//! Test instrumentation (feature `testing`, enabled only through `crates/snp`'s `testing` feature): with
//! `SNP_TEST_SPAWN_LOG=<file>` every spawn appends `<role> <group> <argv0 basename>\n` (spawn-budget tests, WP-32/WP-36).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use std::path::Path;

use snp_core::env::Env;

/// Arms the log from `SNP_TEST_SPAWN_LOG` in the captured environment.
pub fn enable_from_env(env: &Env) {
    todo!()
}

/// The logged lines of `path`.
pub fn read(path: &Path) -> Vec<String> {
    todo!()
}
