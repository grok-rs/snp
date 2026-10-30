//! The command-runner and tool-directory seams (S12).
//!
//! Owner: WP-16.

use std::ffi::OsString;
use std::fmt::Debug;
use std::path::{Path, PathBuf};

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_os::lock::FileLock;
use snp_proc::resolve::normalize_cmd;
use snp_proc::spawn::{self, Captured, Group, PyStatus, Role, Spec, Stdin};

use crate::LangError;

/// The only way backends run setup/probe commands (S12): real impl = `normalize_cmd` against the CHILD env +
/// `snp_proc::spawn::capture`; tests use `harness::ShimRunner`, which records argv/cwd/env and replays scripted outputs.
pub trait CmdRunner: Send + Sync + Debug {
    /// Runs `argv` (unresolved `argv[0]`) with exactly `env` in `cwd` and captures its output.
    fn run(&self, argv: &[OsString], env: &Env, cwd: &Path) -> Result<Captured, LangError>;
}

/// The real [`CmdRunner`]: spawns with `Role::Tool`, `Group::Inherit`.
///
/// `util.cmd_output_b(*argv, check=False)` (`util.py:90-113`): `argv[0]` is resolved with `normalize_cmd` against the
/// child's `PATH`/`PATHEXT` (shebang interpreters prepended, one level); stdout and stderr are captured separately;
/// stdin is `/dev/null` (snp's own tools never get a terminal, ADR-019). Resolution failures and spawn errors do not
/// raise, they become a status-1 result exactly as in pre-commit: stdout = ``Executable `x` not found`` (no newline)
/// or `str(OSError)` + `\n`, stderr empty. A cancelled token is [`LangError::Interrupted`], before or after the
/// child ran (the child itself is escalated by `snp_proc`, §10.4).
#[derive(Debug)]
pub struct ProcRunner {
    /// Cancels running setup commands.
    pub cancel: CancelToken,
}

/// A status-1 result carrying `stdout` (pre-commit's "as if the process had run" errors).
fn failed_run(stdout: Vec<u8>) -> Captured {
    Captured {
        status: PyStatus(1),
        stdout,
        stderr: Vec::new(),
        stopped_by_tty: false,
    }
}

impl CmdRunner for ProcRunner {
    fn run(&self, argv: &[OsString], env: &Env, cwd: &Path) -> Result<Captured, LangError> {
        if self.cancel.is_cancelled() {
            return Err(LangError::Interrupted);
        }
        let resolved = match normalize_cmd(argv, env, cwd) {
            Ok(r) => r,
            Err(e) => return Ok(failed_run(e.message().into_bytes())),
        };
        let spec = Spec {
            argv: &resolved,
            env,
            cwd,
            stdin: Stdin::Null,
            group: Group::Inherit,
            role: Role::Tool,
            deadline: None,
            cancel: &self.cancel,
        };
        let captured = match spawn::capture(&spec) {
            Ok(c) => c,
            Err(e) => {
                // `force_bytes(e).rstrip(b'\n') + b'\n'` (`util.py:104-105`).
                let text = e.python_text();
                failed_run(format!("{}\n", text.trim_end_matches('\n')).into_bytes())
            }
        };
        if self.cancel.is_cancelled() {
            return Err(LangError::Interrupted);
        }
        Ok(captured)
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
