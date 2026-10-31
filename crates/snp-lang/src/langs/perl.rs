//! The `perl` language backend (§6.2; `pre_commit/languages/perl.py`).
//!
//! `install_environment` (`perl.py:42-50`) runs `cpan -T . <deps…>` in the repository (the local template ships a
//! `Makefile.PL`) inside the activated environment, whose `PERL_MB_OPT`/`PERL_MM_OPT` send the installation to the
//! environment directory. The activation (`perl.py:21-32`) puts `<env>/bin` first on `PATH` and **replaces**
//! `PERL5LIB=<env>/lib/perl5`, `PERL_MB_OPT=--install_base <q(env)>` and
//! `PERL_MM_OPT=INSTALL_BASE=<q(env)> INSTALLSITEMAN1DIR=none INSTALLSITEMAN3DIR=none`, `q` being Python's
//! `shlex.quote`.
//!
//! snp difference: the environment directory exists after a successful install even when cpan installed nothing
//! (pre-commit then fails writing its state file, 04b §2.8). COMPATIBILITY §5 has no register ID for this crash fix yet (allocation by the integration lead is pending).
//!
//! Owner: WP-25.

use std::borrow::Cow;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;

use super::docker::group2::{
    assert_support, basic_plan, default_version, makedirs, outcome, push_deps,
};
use crate::common::setup_cmd;
use crate::framework::default_support;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: perl`.
#[derive(Debug)]
pub struct Perl;

/// `shlex.quote(path)` over the path's text (bytes on Unix).
fn quote(p: &Path) -> OsString {
    if let Some(s) = p.to_str() {
        return OsString::from(snp_pycompat::shlex::quote(s).into_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let q: Cow<'_, [u8]> = snp_pycompat::shlex::quote_bytes(p.as_os_str().as_bytes());
        OsString::from_vec(q.into_owned())
    }
    #[cfg(not(unix))]
    {
        OsString::from(snp_pycompat::shlex::quote(&p.to_string_lossy()).into_owned())
    }
}

/// `get_env_patch(env)` (`perl.py:21-32`).
pub fn env_patch(env_dir: &Path) -> EnvPatch {
    let q = quote(env_dir);
    let mut mb = OsString::from("--install_base ");
    mb.push(&q);
    let mut mm = OsString::from("INSTALL_BASE=");
    mm.push(&q);
    mm.push(" INSTALLSITEMAN1DIR=none INSTALLSITEMAN3DIR=none");
    EnvPatch::default()
        .prepend_path("PATH", [env_dir.join("bin")])
        .set(
            "PERL5LIB",
            env_dir.join("lib").join("perl5").into_os_string(),
        )
        .set("PERL_MB_OPT", mb)
        .set("PERL_MM_OPT", mm)
}

impl Language for Perl {
    fn id(&self) -> LanguageId {
        LanguageId::Perl
    }

    /// `assert_version_default('perl', …)`; dependencies allowed.
    fn support(&self) -> Support {
        default_support(LanguageId::Perl)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(default_version())
    }

    /// `install_environment` (`perl.py:42-50`): `cpan -T . <deps…>` in the repository, activated.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        assert_support(LanguageId::Perl, cx)?;
        let host = cx.cx.host;
        let env = host.base_env.patched(&env_patch(cx.cx.env_dir));
        let mut argv: Vec<OsString> = vec!["cpan".into(), "-T".into(), ".".into()];
        push_deps(&mut argv, cx.deps);
        setup_cmd(host.runner, &argv, &env, cx.cx.prefix)?;
        makedirs(cx.cx.env_dir)?;
        Ok(outcome("cpan"))
    }

    /// `lang_base.basic_health_check`.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `get_env_patch(env)`, see the module docs.
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(env_patch(cx.env_dir))
    }

    /// `lang_base.basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
