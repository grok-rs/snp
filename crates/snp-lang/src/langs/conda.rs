//! The `conda` language backend (§6.2; `pre_commit/languages/conda.py`).
//!
//! The environment is `<prefix>/conda-<version>` in pre-commit, created by `<exe> env create -p <env> --file
//! environment.yml` in the repository (the local template ships an `environment.yml`), followed by one
//! `<exe> install -p <env> <deps…>` when there are `additional_dependencies` (no `--yes`: stdin is at EOF). `<exe>` is
//! `micromamba` when `PRE_COMMIT_USE_MICROMAMBA` is non-empty, else `mamba` when `PRE_COMMIT_USE_MAMBA` is non-empty,
//! else `conda` (`conda.py:50-56`; `"0"` counts as set). Installs run with the unpatched environment; hooks run with
//! `PYTHONHOME`/`VIRTUAL_ENV` removed, `CONDA_PREFIX=<env>` and the env's binary directories first on `PATH`.
//!
//! Owner: WP-25.

use std::ffi::OsString;
use std::sync::Arc;

use snp_core::env::{Env, EnvPatch};
use snp_core::lang::LanguageId;

use super::docker::group2::{assert_support, basic_plan, default_version, outcome, push_deps};
use crate::common::setup_cmd;
use crate::framework::default_support;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: conda`.
#[derive(Debug)]
pub struct Conda;

/// `_conda_exe()` (`conda.py:50-56`): `micromamba`, `mamba` or `conda`, by non-empty `PRE_COMMIT_USE_MICROMAMBA` /
/// `PRE_COMMIT_USE_MAMBA` in `env` (micromamba wins).
pub fn conda_exe(env: &Env) -> &'static str {
    if env.get_nonempty("PRE_COMMIT_USE_MICROMAMBA").is_some() {
        "micromamba"
    } else if env.get_nonempty("PRE_COMMIT_USE_MAMBA").is_some() {
        "mamba"
    } else {
        "conda"
    }
}

impl Language for Conda {
    fn id(&self) -> LanguageId {
        LanguageId::Conda
    }

    /// `assert_version_default('conda', …)`; dependencies allowed.
    fn support(&self) -> Support {
        default_support(LanguageId::Conda)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(default_version())
    }

    /// `install_environment` (`conda.py:59-77`): `<exe> env create -p <env> --file environment.yml`, then
    /// `<exe> install -p <env> <deps…>` (one call, only with deps), both in the repository with the run's base
    /// environment (`in_env` is not active during the install).
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        assert_support(LanguageId::Conda, cx)?;
        let host = cx.cx.host;
        let exe = conda_exe(host.base_env);
        let env_dir = cx.cx.env_dir.as_os_str();
        let create: Vec<OsString> = vec![
            exe.into(),
            "env".into(),
            "create".into(),
            "-p".into(),
            env_dir.to_os_string(),
            "--file".into(),
            "environment.yml".into(),
        ];
        setup_cmd(host.runner, &create, host.base_env, cx.cx.prefix)?;
        if !cx.deps.is_empty() {
            let mut install: Vec<OsString> = vec![
                exe.into(),
                "install".into(),
                "-p".into(),
                env_dir.to_os_string(),
            ];
            push_deps(&mut install, cx.deps);
            setup_cmd(host.runner, &install, host.base_env, cx.cx.prefix)?;
        }
        Ok(outcome(exe))
    }

    /// `lang_base.basic_health_check`.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `get_env_patch(env)` (`conda.py:24-40`): `PYTHONHOME`⊘, `VIRTUAL_ENV`⊘, `CONDA_PREFIX=<env>`, `PATH` with
    /// `<env>/bin` first — on Windows `<env>\Library\bin;<env>\Scripts;<env>;<env>\bin` in that order.
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        let env = cx.env_dir;
        let dirs = if cfg!(windows) {
            vec![
                env.join("Library").join("bin"),
                env.join("Scripts"),
                env.to_path_buf(),
                env.join("bin"),
            ]
        } else {
            vec![env.join("bin")]
        };
        Ok(EnvPatch::default()
            .unset("PYTHONHOME")
            .unset("VIRTUAL_ENV")
            .set("CONDA_PREFIX", env.as_os_str())
            .prepend_path("PATH", dirs))
    }

    /// `lang_base.basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
