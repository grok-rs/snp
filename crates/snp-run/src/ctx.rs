//! The run context (§3.1 step 2).
//!
//! Owner: WP-32.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::clock::Clock;
use snp_core::env::Env;
use snp_core::output::Printer;
use snp_git::{GitBackend, GitExe};
use snp_lang::{CmdRunner, ToolProbe};
use snp_net::Fetch;
use snp_proc::pool::{JobPool, Limiter};
use snp_proc::signals::SignalGuard;
use snp_proc::spawn::OutputBudget;
use snp_pyre::RegexCache;
use snp_repo::Resolve;
use snp_store::Store;

/// Constructed once by the binary (§3.1 step 2). `git_backend` is the run's Tool-role backend; the stash builds its
/// own Critical one (§1.4.18). `net`/`installs` are the Limiters of §1.4.8; `probe`/`runner` feed `snp_lang::Host`;
/// the binary builds them as `ToolProbe::new(persist_dir)` and `ProcRunner { cancel }` (snp → snp-lang edge, ADR-081).
#[derive(Debug)]
pub struct RunCtx {
    /// The startup environment.
    pub env: Env,
    /// The brand.
    pub brand: Brand,
    /// The output sink.
    pub printer: Arc<Printer>,
    /// The store.
    pub store: Store,
    /// The resolved git executable.
    pub git_exe: Arc<GitExe>,
    /// The run's Tool-role git backend.
    pub git_backend: Arc<dyn GitBackend>,
    /// Hook resolution (`snp_repo::StoreResolver`, or a fake in tests).
    pub resolver: Arc<dyn Resolve>,
    /// The run's cancel token.
    pub cancel: CancelToken,
    /// The signal handler (mutating commands).
    pub signals: Option<Arc<SignalGuard>>,
    /// The job pool.
    pub pool: Arc<JobPool>,
    /// `NetLimiter`.
    pub net: Arc<Limiter>,
    /// `InstallLimiter`.
    pub installs: Arc<Limiter>,
    /// Tool probes for the language backends.
    pub probe: Arc<ToolProbe>,
    /// Setup/probe command runner.
    pub runner: Arc<dyn CmdRunner>,
    /// The run's output budget (spill files).
    pub budget: Arc<OutputBudget>,
    /// The invocation cwd.
    pub invocation_cwd: PathBuf,
    /// `argv[0]` as given.
    pub arg0: Option<OsString>,
    /// snp's own executable.
    pub self_exe: PathBuf,
    /// Wall clock.
    pub clock: Arc<dyn Clock>,
    /// Downloads (None: offline).
    pub fetch: Option<Arc<dyn Fetch>>,
    /// The run's regex cache.
    pub regexes: Arc<RegexCache>,
}
