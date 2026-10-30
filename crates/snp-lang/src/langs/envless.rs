//! Env-less languages: `fail`, `pygrep`, `unsupported`, `unsupported_script`, `docker_image` (04a §7.3, §8).
//!
//! None of them has an environment (`ENVIRONMENT_DIR = None`): `get_default_version` is `'default'`, the health
//! check passes, `install_environment` is `lang_base.no_install` (an `AssertionError`, unreachable because hook
//! resolution never installs them), `in_env` is a no-op. Only `plan` differs:
//!
//! | language | `run_hook` (pre-commit 4.6.2) | snp [`RunPlan`] |
//! |---|---|---|
//! | `fail` | `(1, f'{entry}\n\n' + '\n'.join(files) + '\n')` (`fail.py:15-27`) | `Fail { entry }` (in-process) |
//! | `pygrep` | `xargs((sys.executable, '-m', 'pre_commit.languages.pygrep', *args, entry), files)`, serial, unshuffled (`pygrep.py:90-101`) | `Pygrep { args, entry }` (in-process) |
//! | `unsupported` (`system`) | `run_xargs(hook_cmd(entry, args), …)` (`lang_base.py:181-196`) | `Xargs { cmd: hook_cmd }` |
//! | `unsupported_script` (`script`) | the same with `cmd[0] = prefix.path(cmd[0])` (`unsupported_script.py:15-32`) | `Xargs { cmd[0] prefix-joined }` |
//! | `docker_image` | `run_xargs(docker_cmd(color) + hook_cmd(entry, args), …)` (`docker_image.py:16-32`) | `Xargs { cmd: docker run … + hook_cmd }` |
//!
//! An entry that `shlex.split` rejects or that splits to nothing (with no args) is pre-commit's exit-3 crash; snp
//! fails only that hook (C-hook, X13-EXE-01/02, D-CRASH-05): [`RunPlan::Error`] with exit code 1 and the text
//! "Executable \`\` not found" or `ValueError: No closing quotation`.
//!
//! Owner: WP-16.

use std::ffi::OsString;
use std::sync::Arc;

use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;
use snp_proc::hookcmd::HookCmdError;

use crate::common::{docker, hook_cmd, prefix_path};
use crate::framework::{default_support, messages};
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `lang_base.basic_get_default_version`.
fn basic_default_version() -> Arc<str> {
    Arc::from("default")
}

/// `lang_base.no_install`.
fn no_install() -> LangError {
    LangError::Assertion(messages::NOT_INSTALLABLE.to_owned())
}

/// The C-hook plan of an `entry` that does not split into a command.
fn entry_error(e: &HookCmdError) -> RunPlan {
    let text = match e {
        HookCmdError::Shlex(err) => messages::shlex_error(err.py_type(), &err.to_string()),
        _ => messages::EMPTY_ENTRY.to_owned(),
    };
    RunPlan::Error {
        code: 1,
        output: text.into_bytes(),
    }
}

/// `run_xargs(cmd, …)`: shuffled and parallel unless the hook sets `require_serial` (decided by the executor).
fn xargs(cmd: Vec<OsString>, cx: &PlanCtx<'_>) -> RunPlan {
    RunPlan::Xargs {
        cmd: Arc::from(cmd),
        env: cx.env.clone(),
        shuffle: true,
        serial: false,
        pre: Vec::new(),
    }
}

/// Implements the members every env-less language shares with `lang_base`.
macro_rules! envless_basics {
    () => {
        fn support(&self) -> Support {
            default_support(self.id())
        }

        fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
            Ok(basic_default_version())
        }

        fn install(&self, _cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
            Err(no_install())
        }

        fn health(
            &self,
            _cx: &EnvCtx<'_>,
            _recorded: Option<&InstallState>,
        ) -> Result<Health, LangError> {
            Ok(Health::Healthy)
        }

        fn activate(&self, _cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
            Ok(EnvPatch::default())
        }
    };
}

/// `language: fail` (`pre_commit/languages/fail.py`).
#[derive(Debug)]
pub struct Fail;

impl Language for Fail {
    fn id(&self) -> LanguageId {
        LanguageId::Fail
    }

    envless_basics!();

    /// `RunPlan::Fail`: no process; the executor prints `entry`, a blank line and the files, exit code 1, with all
    /// files in one call (no partitioning, no shuffle).
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(RunPlan::Fail {
            entry: Arc::from(cx.entry),
        })
    }
}

/// `language: pygrep` (`pre_commit/languages/pygrep.py`).
#[derive(Debug)]
pub struct Pygrep;

impl Language for Pygrep {
    fn id(&self) -> LanguageId {
        LanguageId::Pygrep
    }

    envless_basics!();

    /// `RunPlan::Pygrep`: the native engine (snp-hooks) parses `[*args, entry, *files]` per serial, unshuffled
    /// partition, exactly the argv pre-commit's `python -m pre_commit.languages.pygrep` receives. No `hook_cmd`: the
    /// entry is the pattern, never split or hazmat-rewritten.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(RunPlan::Pygrep {
            args: Arc::from(cx.args),
            entry: Arc::from(cx.entry),
        })
    }
}

/// `language: unsupported (alias `system`)` (`pre_commit/languages/unsupported.py`).
#[derive(Debug)]
pub struct Unsupported;

impl Language for Unsupported {
    fn id(&self) -> LanguageId {
        LanguageId::Unsupported
    }

    envless_basics!();

    /// `basic_run_hook`: `hook_cmd(entry, args)`; the executor resolves `cmd[0]` on the hook's `PATH`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(match hook_cmd(cx.entry, cx.args, cx.host.self_exe) {
            Ok(cmd) => xargs(cmd, cx),
            Err(e) => entry_error(&e),
        })
    }
}

/// `language: unsupported_script (alias `script`)` (`pre_commit/languages/unsupported_script.py`).
#[derive(Debug)]
pub struct UnsupportedScript;

impl Language for UnsupportedScript {
    fn id(&self) -> LanguageId {
        LanguageId::UnsupportedScript
    }

    envless_basics!();

    /// `hook_cmd(entry, args)` with `cmd[0]` replaced by `prefix.path(cmd[0])` — the repo root for local hooks, the
    /// clone for remote ones (`is_local` is ignored, parity); an absolute `cmd[0]` (incl. the hazmat rewrite's
    /// `self_exe`) stays as is.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        let mut cmd = match hook_cmd(cx.entry, cx.args, cx.host.self_exe) {
            Ok(cmd) => cmd,
            Err(e) => return Ok(entry_error(&e)),
        };
        if let Some(first) = cmd.first_mut() {
            *first = prefix_path(cx.prefix, first);
        }
        Ok(xargs(cmd, cx))
    }
}

/// `language: docker_image` (`pre_commit/languages/docker_image.py`).
#[derive(Debug)]
pub struct DockerImage;

impl Language for DockerImage {
    fn id(&self) -> LanguageId {
        LanguageId::DockerImage
    }

    envless_basics!();

    /// `docker run --rm [--tty] [-u uid:gid] -v <cwd>:/src:rw,Z --workdir /src` + `hook_cmd(entry, args)`: the entry
    /// supplies `[docker run options…] <image> [cmd…]`. The rootless probe (`docker system info`) runs through the
    /// host's runner once per process.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        let mut cmd = docker::docker_cmd(cx.host, cx.color)?;
        match hook_cmd(cx.entry, cx.args, cx.host.self_exe) {
            Ok(rest) => cmd.extend(rest),
            Err(e) => return Ok(entry_error(&e)),
        }
        Ok(xargs(cmd, cx))
    }
}
