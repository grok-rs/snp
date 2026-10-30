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
//! hook run); snp spawns it **once per process** for a given environment and cwd ([`lua_version`]) — the output cannot
//! change while snp runs. Non-UTF-8 output is decoded lossily instead of crashing (D-CRASH-13). Rockspecs are sorted.
//! luarocks remains required at run time (parity, 04b §5.10 #5): a missing luarocks fails the hook run with
//! pre-commit's `CalledProcessError`.
//!
//! Owner: WP-25.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use snp_core::env::{Env, EnvPatch};
use snp_core::lang::LanguageId;
use snp_core::sync::lock;

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

/// One memoised probe result: keyed by a digest of the environment and the cwd it ran with.
type Slot = Arc<Mutex<Option<Arc<str>>>>;

/// The memo: (key, slot) pairs.
type Memo = Mutex<Vec<([u8; 32], Slot)>>;

/// The process-wide memo of [`lua_version`].
fn memo() -> &'static Memo {
    static MEMO: OnceLock<Memo> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(Vec::new()))
}

/// blake3 over the sorted environment and the cwd: what steers `luarocks config` (its `PATH`, `LUAROCKS_CONFIG`,
/// `HOME`, the project's `.luarocks/` …).
fn memo_key(env: &Env, cwd: &Path) -> [u8; 32] {
    let mut pairs: Vec<(&OsStr, &OsStr)> = env.iter().collect();
    pairs.sort_by(|a, b| a.0.as_encoded_bytes().cmp(b.0.as_encoded_bytes()));
    let mut h = blake3::Hasher::new();
    for (k, v) in pairs {
        h.update(k.as_encoded_bytes());
        h.update(b"=");
        h.update(v.as_encoded_bytes());
        h.update(b"\0");
    }
    h.update(b"cwd=");
    h.update(cwd.as_os_str().as_encoded_bytes());
    *h.finalize().as_bytes()
}

/// `_get_lua_version()`: `luarocks config --lua-ver` through the host's runner with the run's base environment in
/// the run's cwd, stdout decoded lossily and stripped (Python `str.strip()`); a non-zero exit is pre-commit's
/// format-A `CalledProcessError`.
///
/// Memoised per process (keyed by the environment and the cwd); concurrent callers wait for the one running probe; a
/// failure is not memoised.
pub fn lua_version(host: &Host<'_>) -> Result<Arc<str>, LangError> {
    let key = memo_key(host.base_env, host.cwd);
    let slot = {
        let mut slots = lock(memo());
        if let Some((_, s)) = slots.iter().find(|(k, _)| *k == key) {
            Arc::clone(s)
        } else {
            let s: Slot = Arc::new(Mutex::new(None));
            slots.push((key, Arc::clone(&s)));
            s
        }
    };
    let mut guard = lock(&slot);
    if let Some(v) = guard.as_ref() {
        return Ok(Arc::clone(v));
    }
    let argv: Vec<OsString> = LUA_VERSION_PROBE.iter().map(OsString::from).collect();
    let stdout = setup_cmd(host.runner, &argv, host.base_env, host.cwd)?;
    let text = String::from_utf8_lossy(&stdout);
    let v: Arc<str> = Arc::from(snp_pycompat::text::strip(&text));
    *guard = Some(Arc::clone(&v));
    Ok(v)
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
