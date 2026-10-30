//! The `lua` language backend (§6.2; `pre_commit/languages/lua.py`).
//!
//! The activation (`get_env_patch`, `lua.py:28-43`) needs the Lua version luarocks is configured for:
//! `luarocks config --lua-ver` (stripped), run with the unpatched environment in the run's cwd; it sets `PATH`
//! (`<env>/bin` first), `LUA_PATH=<env>/share/lua/<V>/?.lua;<env>/share/lua/<V>/?/init.lua;;` and
//! `LUA_CPATH=<env>/lib/lua/<V>/?.so;;` (`?.dll;;` on Windows), both replacing the user's values (`;;` appends Lua's
//! defaults). `install_environment` (`lua.py:53-75`) runs inside that environment: it creates `<env>`, runs
//! `luarocks --tree <env> make <rockspec>` for every `*.rockspec` of the repository, then one
//! `luarocks --tree <env> install <dep>` per dependency (luarocks cannot install several at once).
//!
//! snp differences: pre-commit spawns `luarocks config --lua-ver` on every activation (once per install and once per
//! hook run); snp spawns it **once per run** ([`lua_version`], memoised in the run's `ToolProbe`) — the output cannot
//! change while snp runs. Non-UTF-8 output is decoded lossily instead of crashing (D-CRASH-13). Rockspecs are sorted.
//! luarocks remains required at run time (parity, 04b §5.10 #5): a missing luarocks fails the hook run with
//! pre-commit's `CalledProcessError`.
//!
//! Owner: WP-25.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;

use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;

use super::docker::group2::{
    assert_support, basic_plan, default_version, makedirs, outcome, py_join, star,
};
use crate::common::setup_cmd;
use crate::framework::default_support;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: lua`.
#[derive(Debug)]
pub struct Lua;

/// `_get_lua_version()`'s argv (`lua.py:22-25`).
pub const LUA_VERSION_PROBE: [&str; 3] = ["luarocks", "config", "--lua-ver"];

/// `_get_lua_version()`: `luarocks config --lua-ver` through the host's runner with the run's base environment in
/// the run's cwd, stdout decoded lossily and stripped (Python `str.strip()`); a non-zero exit is pre-commit's
/// format-A `CalledProcessError`.
///
/// Memoised in the run's [`ToolProbe`](crate::ToolProbe) (no process-global state, ARCHITECTURE S6): the Lua slot of
/// `ToolProbe::default_version`, which `Lua::default_version` never uses (Lua's `language_version` is always
/// `default`, `lang_base.basic_get_default_version`). Concurrent callers wait for the one running probe; a failure is
/// not memoised.
pub fn lua_version(host: &Host<'_>) -> Result<Arc<str>, LangError> {
    host.probe
        .default_version(LanguageId::Lua, host.base_env, host.cwd, &mut || {
            let argv: Vec<OsString> = LUA_VERSION_PROBE.iter().map(OsString::from).collect();
            let stdout = setup_cmd(host.runner, &argv, host.base_env, host.cwd)?;
            let text = String::from_utf8_lossy(&stdout);
            Ok(Arc::from(snp_pycompat::text::strip(&text)))
        })
}

/// `get_env_patch(d)` for Lua version `version` (`lua.py:28-43`).
pub fn env_patch(env_dir: &Path, version: &str) -> EnvPatch {
    let v = OsStr::new(version);
    let share = |parts: &[&OsStr]| -> OsString {
        let mut all: Vec<&OsStr> = vec![OsStr::new("share"), OsStr::new("lua"), v];
        all.extend_from_slice(parts);
        py_join(env_dir, &all).into_os_string()
    };
    let mut lua_path = share(&[OsStr::new("?.lua;")]);
    lua_path.push(share(&[OsStr::new("?"), OsStr::new("init.lua;;")]));
    let so = if cfg!(windows) { "?.dll;;" } else { "?.so;;" };
    let lua_cpath = py_join(
        env_dir,
        &[OsStr::new("lib"), OsStr::new("lua"), v, OsStr::new(so)],
    );
    EnvPatch::default()
        .prepend_path("PATH", [env_dir.join("bin")])
        .set("LUA_PATH", lua_path)
        .set("LUA_CPATH", lua_cpath.into_os_string())
}

impl Language for Lua {
    fn id(&self) -> LanguageId {
        LanguageId::Lua
    }

    /// `assert_version_default('lua', …)`; dependencies allowed.
    fn support(&self) -> Support {
        default_support(LanguageId::Lua)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(default_version())
    }

    /// `install_environment` (`lua.py:53-75`), see the module docs.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        assert_support(LanguageId::Lua, cx)?;
        let host = cx.cx.host;
        let env_dir = cx.cx.env_dir;
        let env = host.base_env.patched(&self.activate(&cx.cx)?);
        makedirs(env_dir)?;
        for rockspec in star(cx.cx.prefix, ".rockspec")? {
            let argv: Vec<OsString> = vec![
                "luarocks".into(),
                "--tree".into(),
                env_dir.as_os_str().to_os_string(),
                "make".into(),
                rockspec,
            ];
            setup_cmd(host.runner, &argv, &env, cx.cx.prefix)?;
        }
        for dep in cx.deps {
            let argv: Vec<OsString> = vec![
                "luarocks".into(),
                "--tree".into(),
                env_dir.as_os_str().to_os_string(),
                "install".into(),
                OsString::from(&**dep),
            ];
            setup_cmd(host.runner, &argv, &env, cx.cx.prefix)?;
        }
        Ok(outcome("luarocks"))
    }

    /// `lang_base.basic_health_check`.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `get_env_patch(env)` with the memoised [`lua_version`].
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        let version = lua_version(cx.host)?;
        Ok(env_patch(cx.env_dir, &version))
    }

    /// `lang_base.basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
