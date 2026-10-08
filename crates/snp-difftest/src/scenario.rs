//! Scenario files (10 §7.4.3 schema + `brands`, `expect_divergence`).
//!
//! Owner: WP-35.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-35")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-35")]

use std::path::Path;

/// One differential scenario.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// W0 placeholder; WP-35 stores the 10 §7.4.3 schema: requires, repos, tree, config, steps, compare, brands,
    /// `expect_divergence`.
    _priv: (),
}

/// Loads every scenario under `dir` (`tests/differential/scenarios/**`).
pub fn load_scenarios(dir: &Path) -> Result<Vec<Scenario>, String> {
    todo!()
}
