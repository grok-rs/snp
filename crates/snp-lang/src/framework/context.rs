//! What a backend sees: the host services and the per-environment contexts.
//!
//! Owner: WP-16.

use std::path::Path;
use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::output::Printer;
use snp_net::Fetch;

use crate::{CmdRunner, ToolDirs, ToolProbe};

/// Process-wide services of one run.
#[derive(Debug)]
pub struct Host<'a> {
    /// The run's base environment.
    pub base_env: &'a Env,
    /// The run's cwd.
    pub cwd: &'a Path,
    /// snp's own executable.
    pub self_exe: &'a Path,
    /// Default-version and exe-lookup memoisation.
    pub probe: &'a ToolProbe,
    /// Downloads (None: offline).
    pub fetch: Option<&'a dyn Fetch>,
    /// Shared toolchain directories.
    pub tools: &'a dyn ToolDirs,
    /// Setup/probe command runner.
    pub runner: &'a dyn CmdRunner,
    /// The output sink.
    pub printer: &'a Printer,
    /// The run's cancel token.
    pub cancel: &'a CancelToken,
    /// The brand (message texts).
    pub brand: Brand,
}

/// One environment of one language version.
#[derive(Debug)]
pub struct EnvCtx<'a> {
    /// Host services.
    pub host: &'a Host<'a>,
    /// The hook repository checkout (pre-commit's `prefix`).
    pub prefix: &'a Path,
    /// `language_version`.
    pub version: &'a str,
    /// The environment directory.
    pub env_dir: &'a Path,
}

/// An install request.
#[derive(Debug)]
pub struct InstallCtx<'a> {
    /// The environment.
    pub cx: EnvCtx<'a>,
    /// `additional_dependencies`.
    pub deps: &'a [Arc<str>],
    /// `repo: local`.
    pub is_local: bool,
}

/// A run-plan request for one hook.
#[derive(Debug)]
pub struct PlanCtx<'a> {
    /// Host services.
    pub host: &'a Host<'a>,
    /// The hook repository checkout.
    pub prefix: &'a Path,
    /// `language_version`.
    pub version: &'a str,
    /// The environment directory (None for env-less languages).
    pub env_dir: Option<&'a Path>,
    /// The hook's environment (activation applied).
    pub env: &'a Env,
    /// `entry`.
    pub entry: &'a str,
    /// `args`.
    pub args: &'a [Arc<str>],
    /// `repo: local`.
    pub is_local: bool,
    /// The colour decision.
    pub color: bool,
    /// docker image tag (ADR-035).
    pub docker_tag: Option<&'a str>,
}
