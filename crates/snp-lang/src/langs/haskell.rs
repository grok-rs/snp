//! The `haskell` language backend (§6.2; `pre_commit/languages/haskell.py`).
//!
//! `install_environment` (`haskell.py:33-56`): the packages are the repository's `*.cabal` names followed by the
//! `additional_dependencies` (none at all is the `FatalError` `Expected .cabal files or additional_dependencies`);
//! `<env>/bin` is created, then `cabal update` and `cabal install --install-method copy --installdir <env>/bin
//! <pkgs…>` run in the repository. Hooks run with `<env>/bin` first on `PATH`.
//!
//! snp difference: the `*.cabal` names are sorted (pre-commit: directory order, 04b §5.5).
//!
//! Owner: WP-25.

use std::ffi::OsString;
use std::sync::Arc;

use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;

use super::docker::group2::{
    assert_support, basic_plan, default_version, makedirs, outcome, push_deps, star,
};
use crate::common::setup_cmd;
use crate::framework::default_support;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: haskell`.
#[derive(Debug)]
pub struct Haskell;

/// The `FatalError` without `*.cabal` file and dependencies (`haskell.py:41-42`).
pub const NOTHING_TO_INSTALL: &str = "Expected .cabal files or additional_dependencies";

impl Language for Haskell {
    fn id(&self) -> LanguageId {
        LanguageId::Haskell
    }

    /// `assert_version_default('haskell', …)`; dependencies allowed.
    fn support(&self) -> Support {
        default_support(LanguageId::Haskell)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(default_version())
    }

    /// `install_environment` (`haskell.py:33-56`), see the module docs.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        assert_support(LanguageId::Haskell, cx)?;
        let host = cx.cx.host;
        let mut pkgs = star(cx.cx.prefix, ".cabal")?;
        push_deps(&mut pkgs, cx.deps);
        if pkgs.is_empty() {
            return Err(LangError::Fatal(NOTHING_TO_INSTALL.to_owned()));
        }
        let bin_dir = cx.cx.env_dir.join("bin");
        makedirs(&bin_dir)?;
        let update: Vec<OsString> = vec!["cabal".into(), "update".into()];
        setup_cmd(host.runner, &update, host.base_env, cx.cx.prefix)?;
        let mut install: Vec<OsString> = vec![
            "cabal".into(),
            "install".into(),
            "--install-method".into(),
            "copy".into(),
            "--installdir".into(),
            bin_dir.into_os_string(),
        ];
        install.extend(pkgs);
        setup_cmd(host.runner, &install, host.base_env, cx.cx.prefix)?;
        Ok(outcome("cabal"))
    }

    /// `lang_base.basic_health_check`.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `get_env_patch(env)`: `<env>/bin` first on `PATH`.
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(EnvPatch::default().prepend_path("PATH", [cx.env_dir.join("bin")]))
    }

    /// `lang_base.basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
