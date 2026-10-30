//! The `julia` language backend (§6.2; `pre_commit/languages/julia.py`).
//!
//! `language_version` is accepted and ignored (only the environment directory name uses it; parity, 04b Q9).
//! `install_environment` (`julia.py:71-140`) runs inside the activated environment: it creates `<env>`, copies the
//! repository's `JuliaProject.toml` or `Project.toml` (else creates an empty `Project.toml`) and `JuliaManifest.toml` or
//! `Manifest.toml` into it, and runs `julia --startup-file=no -e <JULIA_CODE> -- <env> <deps…>` in the repository.
//! Hooks run `julia --startup-file=no <script> <entry args…> <args…>` where the script is `entry[0]` joined to the
//! repository (kept as is for `repo: local`), with `JULIA_LOAD_PATH=<env>`, `JULIA_PROJECT` removed and
//! `JULIA_DEPOT_PATH=<env>/depot<pathsep>`.
//!
//! Owner: WP-25.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;

use super::docker::group2::{
    copy_into, default_version, entry_error, io_err, is_file, makedirs, outcome, push_deps, xargs,
};
use crate::common::{hook_cmd, prefix_path, setup_cmd};
use crate::framework::default_support;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: julia`.
#[derive(Debug)]
pub struct Julia;

/// The Julia program of `install_environment` (`julia.py:110-135`), byte for byte: it starts with a newline and ends
/// with the 8 spaces that indent the closing `"""` of the Python literal (not dedented).
pub const JULIA_CODE: &str = concat!(
    "\n",
    "        @assert length(ARGS) > 0\n",
    "        hook_env = ARGS[1]\n",
    "        deps = join(ARGS[2:end], \" \")\n",
    "\n",
    "        # We prepend @stdlib here so that we can load the package manager even\n",
    "        # though `get_env_patch` limits `JULIA_LOAD_PATH` to just the hook env.\n",
    "        pushfirst!(LOAD_PATH, \"@stdlib\")\n",
    "        using Pkg\n",
    "        popfirst!(LOAD_PATH)\n",
    "\n",
    "        # Instantiate the environment shipped with the hook repo. If we have\n",
    "        # additional dependencies we disable precompilation in this step to\n",
    "        # avoid double work.\n",
    "        precompile = isempty(deps) ? \"1\" : \"0\"\n",
    "        withenv(\"JULIA_PKG_PRECOMPILE_AUTO\" => precompile) do\n",
    "            Pkg.instantiate()\n",
    "        end\n",
    "\n",
    "        # Add additional dependencies (with precompilation)\n",
    "        if !isempty(deps)\n",
    "            withenv(\"JULIA_PKG_PRECOMPILE_AUTO\" => \"1\") do\n",
    "                Pkg.REPLMode.pkgstr(\"add \" * deps)\n",
    "            end\n",
    "        end\n",
    "        ",
);

/// The interpreter option of install and run (`--startup-file=no` since 4.3.0: a user's `startup.jl` never runs).
const STARTUP: &str = "--startup-file=no";

/// `get_env_patch(env, version)` (`julia.py:49-61`).
fn env_patch(env_dir: &Path) -> EnvPatch {
    let mut depot = env_dir.join("depot").into_os_string();
    depot.push(if cfg!(windows) { ";" } else { ":" });
    EnvPatch::default()
        .set("JULIA_LOAD_PATH", env_dir.as_os_str())
        .unset("JULIA_PROJECT")
        .set("JULIA_DEPOT_PATH", depot)
}

/// Copies the first existing file of `names` from the repository into `env_dir`; whether one was found.
fn copy_first(prefix: &Path, env_dir: &Path, names: [&str; 2]) -> Result<bool, LangError> {
    for name in names {
        let file = PathBuf::from(prefix_path(prefix, OsStr::new(name)));
        if is_file(&file) {
            copy_into(&file, env_dir)?;
            return Ok(true);
        }
    }
    Ok(false)
}

impl Language for Julia {
    fn id(&self) -> LanguageId {
        LanguageId::Julia
    }

    /// `language_version` ignored; dependencies allowed.
    fn support(&self) -> Support {
        default_support(LanguageId::Julia)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(default_version())
    }

    /// `install_environment` (`julia.py:71-140`), see the module docs. No version assertion: any
    /// `language_version` is accepted.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        let host = cx.cx.host;
        let env_dir = cx.cx.env_dir;
        let env = host.base_env.patched(&env_patch(env_dir));
        makedirs(env_dir)?;
        let prefix = cx.cx.prefix;
        if !copy_first(prefix, env_dir, ["JuliaProject.toml", "Project.toml"])? {
            // `open(…, 'a').close()`: creates the file without truncating an existing one.
            let project = env_dir.join("Project.toml");
            std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&project)
                .map_err(io_err("open", &project))?;
        }
        copy_first(prefix, env_dir, ["JuliaManifest.toml", "Manifest.toml"])?;
        let mut argv: Vec<OsString> = vec![
            "julia".into(),
            STARTUP.into(),
            "-e".into(),
            JULIA_CODE.into(),
            "--".into(),
            env_dir.as_os_str().to_os_string(),
        ];
        push_deps(&mut argv, cx.deps);
        setup_cmd(host.runner, &argv, &env, prefix)?;
        Ok(outcome("julia"))
    }

    /// `lang_base.basic_health_check`.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `JULIA_LOAD_PATH=<env>`, `JULIA_PROJECT`⊘, `JULIA_DEPOT_PATH=<env>/depot<pathsep>` (the trailing separator
    /// appends Julia's default depots, 4.6.1).
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(env_patch(cx.env_dir))
    }

    /// `run_hook` (`julia.py:21-46`): `hook_cmd(entry, args)`, the script (`cmd[0]`) joined to the repository unless
    /// the hook is local, then `julia --startup-file=no <script> <cmd[1:]…>`. An entry that does not split into a
    /// command fails only this hook (C-hook, X13-EXE-01/02).
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        let cmd = match hook_cmd(cx.entry, cx.args, cx.host.self_exe) {
            Ok(cmd) => cmd,
            Err(e) => return Ok(entry_error(&e)),
        };
        let Some((script, rest)) = cmd.split_first() else {
            return Ok(entry_error(&snp_proc::hookcmd::HookCmdError::Empty));
        };
        let script = if cx.is_local {
            script.clone()
        } else {
            prefix_path(cx.prefix, script)
        };
        let mut argv: Vec<OsString> = vec!["julia".into(), STARTUP.into(), script];
        argv.extend(rest.iter().cloned());
        Ok(xargs(argv, cx.env))
    }
}
