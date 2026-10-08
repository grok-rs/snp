//! 04b §5.12 shim harness: golden argv/cwd/env logs per language (feature `testing`).
//!
//! Owner: WP-16.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-16")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-16")]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use snp_core::env::Env;
use snp_proc::spawn::Captured;

use crate::{CmdRunner, LangError};

/// Scripted responses keyed by `argv[0]` basename; records calls.
#[derive(Debug, Default)]
pub struct ShimRunner {
    /// W0 placeholder; WP-16 stores the scripted responses and the call log.
    _priv: (),
}

impl CmdRunner for ShimRunner {
    fn run(&self, argv: &[OsString], env: &Env, cwd: &Path) -> Result<Captured, LangError> {
        todo!()
    }
}

/// One recorded call: argv, cwd and the environment delta against the host env.
#[derive(Debug, Clone)]
pub struct Recorded {
    /// The argv as passed to the runner.
    pub argv: Vec<OsString>,
    /// The cwd.
    pub cwd: PathBuf,
    /// (name, Some(value) | None for removed) relative to the base env.
    pub env_delta: Vec<(OsString, Option<OsString>)>,
}
