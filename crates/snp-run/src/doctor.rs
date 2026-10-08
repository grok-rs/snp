//! `snp doctor --fast-path` rows (ADR-070).
//!
//! Owner: WP-32.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-32")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-32")]

use std::ffi::OsStr;

use snp_core::failure::Failure;

use crate::RunCtx;

/// `snp doctor --fast-path`: loads the config and returns `snp_repo::decision_table` rows as (ok, text); the binary
/// wraps it in a `snp_cmd::DoctorCheck` (no snp → snp-config/snp-lang edge needed; ADR-070).
pub fn doctor_fast_path(config: &OsStr, cx: &RunCtx) -> Result<Vec<(bool, String)>, Failure> {
    todo!()
}
