//! The command context.
//!
//! Owner: WP-33.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::output::Printer;
use snp_git::{GitBackend, GitExe};
use snp_proc::pool::JobPool;
use snp_store::Store;

/// What every non-run command receives from the binary.
#[derive(Debug)]
pub struct CmdCtx<'a> {
    /// The startup environment.
    pub env: &'a Env,
    /// The invocation cwd.
    pub cwd: &'a Path,
    /// The brand.
    pub brand: Brand,
    /// The output sink.
    pub printer: &'a Printer,
    /// The store.
    pub store: &'a Store,
    /// The git executable, when one was found (validate-*, sample-config need none).
    pub git_exe: Option<&'a Arc<GitExe>>,
    /// The Tool-role git backend.
    pub git_backend: Arc<dyn GitBackend>,
    /// The cancel token.
    pub cancel: &'a CancelToken,
    /// The job pool (autoupdate `-j`).
    pub pool: &'a JobPool,
    /// snp's own executable.
    pub self_exe: &'a Path,
    /// `argv[0]` as given (`scripts::select_exe`).
    pub arg0: Option<&'a OsStr>,
}
