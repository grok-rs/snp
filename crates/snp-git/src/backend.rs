//! The `GitBackend` seam (S12) and its snp-proc implementation (ADR-065).
//!
//! Owner: WP-18.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-18")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-18")]

use std::ffi::OsString;
use std::fmt::Debug;
use std::io;
use std::path::Path;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_proc::spawn::{Captured, Group, PyStatus, Role, SpawnError, Stdin};

/// Seam (S12): real = snp-proc; tests = `testing::ScriptedBackend` (fail any call by argv pattern, record calls).
pub trait GitBackend: Send + Sync + Debug {
    /// `spawn::capture`.
    fn capture(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
        group: Group,
    ) -> Result<Captured, SpawnError>;
    /// `spawn::stream`.
    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError>;
    /// `spawn::inherit`.
    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError>;
}

/// `role` is used for every spawn: the run's backend uses `Role::Tool` with the run token; the stash builds its own
/// backend with `Role::Critical` and a fresh token that is never cancelled (ADR-065).
#[derive(Debug)]
pub struct ProcBackend {
    /// Cancellation of Hook/Tool spawns.
    pub cancel: CancelToken,
    /// The role of every spawn.
    pub role: Role,
}

impl GitBackend for ProcBackend {
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
