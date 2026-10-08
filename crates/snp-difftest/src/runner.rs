//! Runs a scenario under snp and the oracle and compares (§13.2).
//!
//! Owner: WP-35.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-35")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-35")]

use std::path::PathBuf;

use crate::Scenario;

/// `oracle_snp_prog` = the pc462 oracle launched with argparse prog patched to `snp` (ADR-058), used for snp-brand
/// runs.
#[derive(Debug)]
pub struct Runner {
    /// The snp binary under test.
    pub snp_bin: PathBuf,
    /// `target/oracle/pc462-py312/bin/pre-commit`.
    pub oracle_bin: PathBuf,
    /// The `pre-commit-snp-prog` launcher.
    pub oracle_snp_prog: PathBuf,
    /// The fixed absolute root both tools run under.
    pub root: PathBuf,
}

/// The comparison result.
#[derive(Debug)]
#[non_exhaustive]
pub enum Verdict {
    /// Identical after normalisation.
    Same,
    /// A difference; `id` = the registered divergence it matches, if any.
    Diverged {
        /// The matching register ID.
        id: Option<String>,
        /// The rendered difference.
        diff: String,
    },
    /// A listed divergence that no longer reproduces.
    StaleDivergence(String),
}

impl Runner {
    /// Materialises the scenario for both tools and compares every observed effect.
    pub fn run(&self, s: &Scenario) -> Verdict {
        todo!()
    }
}
