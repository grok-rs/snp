//! `ScriptedBackend` test seam (feature `testing`).
//!
//! Owner: WP-18.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-18")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-18"
)]

use std::ffi::OsString;
use std::io;
use std::path::Path;

use snp_core::env::Env;
use snp_proc::spawn::{Captured, Group, PyStatus, SpawnError, Stdin};

use crate::GitBackend;

/// Rules (argv glob → [`Response`]) and a call log.
#[derive(Debug, Default)]
pub struct ScriptedBackend {
    /// W0 placeholder; WP-18 stores the rules and the call log.
    _priv: (),
}

/// What a scripted call does.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Response {
    /// Run the real git.
    Real,
    /// Exit with this status and output.
    Exit {
        /// Exit status.
        code: i64,
        /// stdout.
        stdout: Vec<u8>,
        /// stderr.
        stderr: Vec<u8>,
    },
    /// Fail to spawn with this errno.
    SpawnError(i32),
    /// Panic inside the backend.
    Panic,
}

impl ScriptedBackend {
    /// No rules: every call is `Real`.
    pub fn new() -> Self {
        todo!()
    }

    /// Adds a rule; later rules win.
    pub fn on(&self, argv_glob: &str, r: Response) -> &Self {
        todo!()
    }

    /// Every argv seen, in order.
    pub fn calls(&self) -> Vec<Vec<OsString>> {
        todo!()
    }
}

impl GitBackend for ScriptedBackend {
    fn capture(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
        group: Group,
    ) -> Result<Captured, SpawnError> {
        todo!()
    }

    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        todo!()
    }

    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError> {
        todo!()
    }
}
