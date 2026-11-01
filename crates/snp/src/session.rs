//! What one invocation knows before and while a command runs (§10.1): the brand, the startup environment, the
//! invocation cwd, the printer, the process cancel token, the signal guard once installed, and the facts the error
//! envelope needs afterwards without calling into git again (the `git --version` line).
//!
//! Owner: WP-33.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::output::Printer;
use snp_proc::signals::SignalGuard;

/// One invocation.
#[derive(Debug)]
pub(crate) struct Session {
    /// The brand of this invocation (ADR-023).
    pub(crate) brand: Brand,
    /// The startup environment (`Env::capture_process`, the only capture).
    pub(crate) env: Env,
    /// The invocation cwd (`os.getcwd()` before any chdir); `None` when it cannot be determined (deleted).
    pub(crate) cwd: Option<PathBuf>,
    /// `argv[0]` as given (`scripts::select_exe`).
    pub(crate) arg0: Option<OsString>,
    /// The process printer.
    pub(crate) printer: Printer,
    /// The process cancel token (signals, EPIPE, panics).
    pub(crate) cancel: CancelToken,
    /// snp's own executable (`current_exe()`; resolved, never embedded as is: `select_exe` picks the stable path).
    pub(crate) self_exe: PathBuf,
    /// The signal handler, once a command installed it (dropped at process exit, after the envelope).
    pub(crate) signals: OnceLock<Arc<SignalGuard>>,
    /// `git version X.Y.Z` once a command resolved git (crash-log version block).
    pub(crate) git_version: OnceLock<String>,
}

impl Session {
    /// The invocation cwd, or pre-commit's `os.getcwd()` failure (`FileNotFoundError`, exit 3).
    pub(crate) fn cwd(&self) -> Result<&std::path::Path, snp_core::failure::Failure> {
        self.cwd.as_deref().ok_or_else(|| {
            snp_core::failure::Failure::from_io(
                &std::io::Error::from(std::io::ErrorKind::NotFound),
                None,
                None,
            )
        })
    }
}
