//! The command-runner and tool-directory seams (S12).
//!
//! Owner: WP-16.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-16")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-16")]

use std::ffi::OsString;
use std::fmt::Debug;
use std::path::{Path, PathBuf};

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_os::lock::FileLock;
use snp_proc::spawn::Captured;

use crate::LangError;

/// The only way backends run setup/probe commands (S12): real impl = `normalize_cmd` against the CHILD env +
/// `snp_proc::spawn::capture`; tests use `harness::ShimRunner`, which records argv/cwd/env and replays scripted outputs.
pub trait CmdRunner: Send + Sync + Debug {
    /// Runs `argv` (unresolved `argv[0]`) with exactly `env` in `cwd` and captures its output.
    fn run(&self, argv: &[OsString], env: &Env, cwd: &Path) -> Result<Captured, LangError>;
}

/// The real [`CmdRunner`]: spawns with `Role::Tool`, `Group::Inherit`.
#[derive(Debug)]
pub struct ProcRunner {
    /// Cancels running setup commands.
    pub cancel: CancelToken,
}

impl CmdRunner for ProcRunner {
    fn run(&self, argv: &[OsString], env: &Env, cwd: &Path) -> Result<Captured, LangError> {
        todo!()
    }
}

/// Implemented over the store by `snp_repo::StoreTools` (WP-31); tests use a temp-dir impl.
pub trait ToolDirs: Send + Sync + Debug {
    /// The shared toolchain directory of `tool` at `version`.
    fn tool_dir(&self, tool: &str, version: &str) -> PathBuf;
    /// `tools-v1/<tool>.lock`; `on_wait` prints the brand's `Locking … directory` line once per process.
    fn lock_tool(&self, tool: &str, c: &CancelToken) -> Result<FileLock, Failure>;
    /// Isolated tool caches (uv, go, npm, …).
    fn cache_dir(&self, name: &str) -> PathBuf;
}
