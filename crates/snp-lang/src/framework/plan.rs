//! What to run; the executor (snp-run) applies shuffle/partitioning/pool/pty uniformly.
//!
//! Owner: WP-16.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use snp_core::env::Env;

/// What to run; the executor (snp-run) applies shuffle/partitioning/pool/pty uniformly.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum RunPlan {
    /// `lang_base.run_xargs`.
    Xargs {
        /// The resolved command prefix.
        cmd: Arc<[OsString]>,
        /// The hook environment.
        env: Env,
        /// `lang_base._shuffled` (not for `require_serial`).
        shuffle: bool,
        /// `require_serial`.
        serial: bool,
        /// Steps before the first partition.
        pre: Vec<PreStep>,
    },
    /// In-process, no spawn.
    Fail {
        /// `entry` (the failure message).
        entry: Arc<str>,
    },
    /// In-process native, no shuffle, serial partitions.
    Pygrep {
        /// `args`.
        args: Arc<[Arc<str>]>,
        /// `entry` (the pattern).
        entry: Arc<str>,
    },
    /// C-hook rows (X13-EXE-01..04): an empty `entry` ("Executable … not found"), ….
    Error {
        /// Exit code.
        code: i32,
        /// Output bytes.
        output: Vec<u8>,
    },
}

/// e.g. docker's per-run `docker build` (skipped when `skip_if_ok` succeeds; ADR-035).
#[derive(Debug, Clone)]
pub struct PreStep {
    /// The command.
    pub argv: Vec<OsString>,
    /// Its environment.
    pub env: Env,
    /// Its cwd.
    pub cwd: PathBuf,
    /// A probe command; the step is skipped when it succeeds.
    pub skip_if_ok: Option<Vec<OsString>>,
}
