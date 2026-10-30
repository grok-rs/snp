//! The `coursier` language backend (§6.2; `pre_commit/languages/coursier.py`).
//!
//! `install_environment` (`coursier.py:23-62`) locates `cs` (else `coursier`) on the **unpatched** `PATH`, then —
//! inside the activated environment (`PATH=<env>:…`, `COURSIER_CACHE=<env>/.cs-cache`) — runs one
//! `cs fetch` + `cs install --dir <env>` pair per application descriptor of the repository's `.pre-commit-channel/`
//! directory (`--default-channels=false --channel <dir> <app>`, the app being the descriptor's stem), and one pair for
//! all `additional_dependencies`. Without a channel directory and without dependencies it is the `FatalError`
//! `expected .pre-commit-channel dir or additional_dependencies`.
//!
//! snp differences: descriptors are processed in sorted order (pre-commit: directory order, 04b §5.5), and an empty
//! channel directory without dependencies yields an empty environment instead of pre-commit's `FileNotFoundError` on
//! its state file (04b §5.10 #8).
//!
//! Owner: WP-25.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::env::{Env, EnvPatch};
use snp_core::lang::LanguageId;

use super::docker::group2::{
    assert_support, basic_plan, default_version, listdir, makedirs, outcome, push_deps, stem,
};
use crate::common::{prefix_path, setup_cmd};
use crate::framework::default_support;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: coursier`.
#[derive(Debug)]
pub struct Coursier;

/// The `AssertionError` when neither `cs` nor `coursier` is found (`coursier.py:30-34`), brand-aware.
pub fn missing_cs_message(brand: Brand) -> String {
    format!(
        "{} requires system-installed \"cs\" or \"coursier\" executables in the application search path",
        brand.prog()
    )
}

/// The `FatalError` without channel directory and dependencies (`coursier.py:52-54`).
pub const NOTHING_TO_INSTALL: &str = "expected .pre-commit-channel dir or additional_dependencies";

/// `PATH` first entry and `COURSIER_CACHE` of an environment.
fn env_patch(env_dir: &Path) -> EnvPatch {
    EnvPatch::default()
        .prepend_path("PATH", [env_dir.to_path_buf()])
        .set("COURSIER_CACHE", env_dir.join(".cs-cache").into_os_string())
}

/// `_install(*opts)`: `cs fetch <opts…>`, then `cs install --dir <env> <opts…>`, both in the repository.
fn install_pair(
    cx: &InstallCtx<'_>,
    cs: &OsStr,
    env: &Env,
    opts: &[OsString],
) -> Result<(), LangError> {
    let host = cx.cx.host;
    let mut fetch: Vec<OsString> = vec![cs.to_os_string(), "fetch".into()];
    fetch.extend(opts.iter().cloned());
    setup_cmd(host.runner, &fetch, env, cx.cx.prefix)?;
    let mut install: Vec<OsString> = vec![
        cs.to_os_string(),
        "install".into(),
        "--dir".into(),
        cx.cx.env_dir.as_os_str().to_os_string(),
    ];
    install.extend(opts.iter().cloned());
    setup_cmd(host.runner, &install, env, cx.cx.prefix)?;
    Ok(())
}

impl Language for Coursier {
    fn id(&self) -> LanguageId {
        LanguageId::Coursier
    }

    /// `assert_version_default('coursier', …)`; dependencies allowed.
    fn support(&self) -> Support {
        default_support(LanguageId::Coursier)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(default_version())
    }

    /// `install_environment` (`coursier.py:23-62`), see the module docs.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        assert_support(LanguageId::Coursier, cx)?;
        let host = cx.cx.host;
        let cs = ["cs", "coursier"]
            .iter()
            .find_map(|n| host.probe.find_exe(OsStr::new(n), host.base_env, host.cwd))
            .ok_or_else(|| LangError::Assertion(missing_cs_message(host.brand)))?;
        let env = host.base_env.patched(&env_patch(cx.cx.env_dir));
        let channel = prefix_path(cx.cx.prefix, OsStr::new(".pre-commit-channel"));
        let channel_path = Path::new(&channel);
        if channel_path.is_dir() {
            for descriptor in listdir(channel_path)? {
                let opts: Vec<OsString> = vec![
                    "--default-channels=false".into(),
                    "--channel".into(),
                    channel.clone(),
                    stem(&descriptor),
                ];
                install_pair(cx, cs.as_os_str(), &env, &opts)?;
            }
        } else if cx.deps.is_empty() {
            return Err(LangError::Fatal(NOTHING_TO_INSTALL.to_owned()));
        }
        if !cx.deps.is_empty() {
            let mut opts = Vec::new();
            push_deps(&mut opts, cx.deps);
            install_pair(cx, cs.as_os_str(), &env, &opts)?;
        }
        // An empty channel directory installs nothing; the environment still exists (04b §5.10 #8).
        makedirs(cx.cx.env_dir)?;
        let installer = cs
            .file_stem()
            .map_or_else(|| "cs".to_owned(), |s| s.to_string_lossy().into_owned());
        Ok(outcome(&installer))
    }

    /// `lang_base.basic_health_check`.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `get_env_patch(env)` (`coursier.py:65-69`): `PATH` with the environment directory itself first (not
    /// `<env>/bin`), `COURSIER_CACHE=<env>/.cs-cache` (jars stay in the environment).
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(env_patch(cx.env_dir))
    }

    /// `lang_base.basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
