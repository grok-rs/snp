//! `snp doctor` (§9.3, ADR-070).
//!
//! Owner: WP-33.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-33")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-33")]

use std::fmt::Debug;

use snp_cli::DoctorArgs;
use snp_core::failure::Failure;

use crate::CmdCtx;

/// Checks that need crates snp-cmd cannot depend on are injected by the binary: the fast-path table (wrapping
/// `snp_run::doctor_fast_path`). The stash-recovery check and `--stash` verification live in snp-cmd (snp-stash
/// edge).
pub trait DoctorCheck: Debug {
    /// The check's heading.
    fn name(&self) -> &'static str;
    /// The check's result lines.
    fn run(&self, cx: &CmdCtx<'_>) -> Vec<DoctorLine>;
}

/// One doctor result line.
#[derive(Debug, Clone)]
pub struct DoctorLine {
    /// Passed.
    pub ok: bool,
    /// What was checked.
    pub text: String,
    /// What `--fix` would do.
    pub fix: Option<String>,
}

/// `snp doctor [--fix] [--fast-path] [--stash]`.
pub fn doctor(a: &DoctorArgs, cx: &CmdCtx<'_>, extra: &[&dyn DoctorCheck]) -> Result<i32, Failure> {
    todo!()
}
