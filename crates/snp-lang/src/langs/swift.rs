//! The `swift` language backend (§6.2; `pre_commit/languages/swift.py`).
//!
//! `install_environment` (`swift.py:36-50`) creates `<env>` (`os.mkdir`: it must not exist) and runs `swift build
//! --package-path <prefix> -c release --build-path <env>/.build` **in the run's cwd** (no `cwd=`: the git root);
//! `additional_dependencies` are not supported. Hooks run with `<env>/.build/release` first on `PATH`.
//!
//! pre-commit marks the backend `# pragma: win32 no cover` without refusing it; snp attempts it on Windows the same way
//! (DECISIONS 04b Q11: "lua/swift on Windows → attempted as pre-commit does").
//!
//! Owner: WP-25.

use std::ffi::OsString;
use std::sync::Arc;

use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;

use super::docker::group2::{assert_support, basic_plan, default_version, mkdir_new, outcome};
use crate::common::setup_cmd;
use crate::framework::default_support;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: swift`.
#[derive(Debug)]
pub struct Swift;

/// `swift.BUILD_DIR`.
const BUILD_DIR: &str = ".build";
/// `swift.BUILD_CONFIG`.
const BUILD_CONFIG: &str = "release";

impl Language for Swift {
    fn id(&self) -> LanguageId {
        LanguageId::Swift
    }

    /// `assert_version_default('swift', …)` and `assert_no_additional_deps('swift', …)`.
    fn support(&self) -> Support {
        default_support(LanguageId::Swift)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(default_version())
    }

    /// `install_environment` (`swift.py:36-50`), see the module docs.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        assert_support(LanguageId::Swift, cx)?;
        let host = cx.cx.host;
        let env_dir = cx.cx.env_dir;
        mkdir_new(env_dir)?;
        let argv: Vec<OsString> = vec![
            "swift".into(),
            "build".into(),
            "--package-path".into(),
            cx.cx.prefix.as_os_str().to_os_string(),
            "-c".into(),
            BUILD_CONFIG.into(),
            "--build-path".into(),
            env_dir.join(BUILD_DIR).into_os_string(),
        ];
        setup_cmd(host.runner, &argv, host.base_env, host.cwd)?;
        Ok(outcome("swift"))
    }

    /// `lang_base.basic_health_check`.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `get_env_patch(env)`: `<env>/.build/release` first on `PATH`.
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(EnvPatch::default()
            .prepend_path("PATH", [cx.env_dir.join(BUILD_DIR).join(BUILD_CONFIG)]))
    }

    /// `lang_base.basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
