//! The `node` language backend (§6.2; `pre_commit/languages/node.py`).
//!
//! | pre-commit 4.6.2 | snp |
//! |---|---|
//! | `get_default_version`: `default` on Windows, `system` iff `exe_exists('node') and exe_exists('npm')` (the `$HOME`/`shims` heuristic) | the same (`SNP_PREFER_SYSTEM_TOOLCHAINS=1` drops the heuristic) |
//! | `python -mnodeenv --prebuilt --clean-src <env> [-n V]` (cwd = the user's repository; `.node-version` and `[nodeenv] node=` of `tox.ini`/`setup.cfg`/`~/.nodeenvrc` steer `default`) | nodeenv reimplemented: the same version choice (`default` = newest release, `lts`, `X.Y.Z`, the nodeenv files quirk, 04a Q9), the archive verified against `SHASUMS256.txt` (TLS; musl and riscv64 from the unofficial builds, arm64 → x64 retry on macOS) into the shared `tools-v1/node/<version>` and linked into the environment; `system` gets the directory layout without nodeenv's bash shim |
//! | Windows `system`: nodeenv refuses, exit 3 | a shim-less system environment (D-CRASH-20) |
//! | `npm install --allow-git=root --install-links -g <pkgs>` in the prefix under `in_env` (`git+file://<prefix>` when the clone has `package.json`; none ⇒ `AssertionError`) | the same, with the installer environment (`no_git_env`, D-SEC-05) |
//! | `health_check`: `node --version` per environment and run | fingerprint fast path: no spawn while the `node` the environment resolves is unchanged |
//!
//! Owner: WP-24.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::{Env, EnvPatch};
use snp_core::lang::LanguageId;
use snp_net::NetError;
use snp_net::tools::{self, Tool};

use crate::common::{bin_dir, setup_cmd};
use crate::framework::default_support;
use crate::langs::python::shared::{
    self, SharedTool, basic_plan, ensure_tool, io_err, mkdirs, no_git_env, read_ini, system_exe,
};
use crate::{
    EnvCtx, Fingerprint, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError,
    Language, PlanCtx, RunPlan, Support,
};

/// The texts of the node backend (pre-commit 4.6.2's).
mod messages {
    /// `install_environment` without anything to install (`node.py:84-88`).
    pub(super) const NOTHING_TO_INSTALL: &str =
        "`language: node` must have package.json or additional_dependencies";

    /// `health_check` (`node.py:67-73`).
    pub(super) fn version_failed(rc: i64) -> String {
        format!("`node --version` returned {rc}")
    }
}

/// `language: node`.
#[derive(Debug)]
pub struct Node;

/// `(install_prefix, lib_dir)` of `get_env_patch` (`node.py:38-57`): `<env>`/`lib` on POSIX, `<env>\Scripts`/`Scripts`
/// on Windows.
fn layout(env_dir: &Path) -> (PathBuf, &'static str) {
    if cfg!(windows) {
        (bin_dir(env_dir), "Scripts")
    } else {
        (env_dir.to_path_buf(), "lib")
    }
}

/// `get_env_patch(venv)` (`node.py:38-57`).
fn env_patch(env_dir: &Path) -> EnvPatch {
    let (install_prefix, lib_dir) = layout(env_dir);
    EnvPatch::default()
        .set("NODE_VIRTUAL_ENV", env_dir.as_os_str())
        .set("NPM_CONFIG_PREFIX", install_prefix.as_os_str())
        .set("npm_config_prefix", install_prefix.as_os_str())
        .unset("NPM_CONFIG_USERCONFIG")
        .unset("npm_config_userconfig")
        .set(
            "NODE_PATH",
            env_dir.join(lib_dir).join("node_modules").into_os_string(),
        )
        .prepend_path("PATH", [bin_dir(env_dir)])
}

/// nodeenv's version choice for `default` (no `-n`): `Config._load` reads `[nodeenv] node=` from `~/.nodeenvrc`,
/// `./setup.cfg`, `./tox.ini` (later files win) and then the first line of `./.node-version`, all relative to the cwd
/// pre-commit runs nodeenv in — the user's repository (nodeenv 1.x, 04a §3.4, Q9). `None`: nodeenv's own default
/// (`latest`).
pub(crate) fn nodeenv_default(h: &Host<'_>) -> Option<String> {
    let home = PathBuf::from(crate::common::home_dir(h.base_env));
    let mut node: Option<String> = None;
    for file in [
        home.join(".nodeenvrc"),
        h.cwd.join("setup.cfg"),
        h.cwd.join("tox.ini"),
    ] {
        let Some(ini) = read_ini(&file) else {
            continue;
        };
        if let Some(v) = shared::ini_section(&ini, "nodeenv")
            .and_then(|s| s.iter().find(|(k, _)| k == "node"))
            .map(|(_, v)| v.clone())
        {
            node = Some(v);
        }
    }
    if let Ok(bytes) = snp_os::fs::read_capped(&h.cwd.join(".node-version"), shared::MAX_INI) {
        let text = String::from_utf8_lossy(&bytes);
        if let Some(first) = snp_pycompat::text::splitlines(&text).next() {
            node = Some(snp_pycompat::text::strip(first).to_owned());
        }
    }
    node.filter(|v| !v.is_empty())
}

/// The node version to download for `language_version` (`None`: a system environment).
fn requested_node(h: &Host<'_>, version: &str) -> Option<String> {
    let v = if version == "default" {
        nodeenv_default(h).unwrap_or_else(|| "latest".to_owned())
    } else {
        version.to_owned()
    };
    match v.to_ascii_lowercase().as_str() {
        "system" => None,
        "latest" => Some("latest".to_owned()),
        "lts" => Some("lts".to_owned()),
        _ => Some(v),
    }
}

/// The node toolchain for `requested` from the shared tools, retrying an arm64 macOS build that does not exist with
/// the x64 one (nodeenv: an `HTTPError` on an `arm64` URL retries `x64`, for old releases under Rosetta).
fn node_toolchain(h: &Host<'_>, requested: &str) -> Result<(SharedTool, &'static str), LangError> {
    let target = tools::host_target();
    match ensure_tool(h, Tool::Node, "node", requested, target) {
        Err(LangError::Net(NetError::Http { status: 404, .. }))
            if target == "aarch64-apple-darwin" =>
        {
            let t = ensure_tool(h, Tool::Node, "node-x64", requested, "x86_64-apple-darwin")?;
            Ok((t, "node-x64"))
        }
        other => other.map(|t| (t, "node")),
    }
}

/// Links (POSIX) or wraps (Windows) the shared toolchain's executables into the environment.
fn link_toolchain(tool: &Path, env_dir: &Path) -> Result<(), LangError> {
    let bin = bin_dir(env_dir);
    if cfg!(windows) {
        let node = tool.join("node.exe");
        let target = bin.join("node.exe");
        if std::fs::hard_link(&node, &target).is_err() {
            std::fs::copy(&node, &target).map_err(|e| io_err("copy node", &target, e))?;
        }
        for (name, script) in [
            ("npm.cmd", "node_modules\\npm\\bin\\npm-cli.js"),
            ("npx.cmd", "node_modules\\npm\\bin\\npx-cli.js"),
        ] {
            let body = format!(
                "@ECHO off\r\n\"%~dp0node.exe\" \"{}\" %*\r\n",
                tool.join(script).display()
            );
            let path = bin.join(name);
            std::fs::write(&path, body).map_err(|e| io_err("write", &path, e))?;
        }
        return Ok(());
    }
    let tool_bin = tool.join("bin");
    for name in ["node", "npm", "npx", "corepack"] {
        let src = tool_bin.join(name);
        if std::fs::symlink_metadata(&src).is_ok() {
            let link = bin.join(name);
            shared::symlink(&src, &link, false).map_err(|e| io_err("symlink", &link, e))?;
        }
    }
    let include = tool.join("include");
    if include.is_dir() {
        let link = env_dir.join("include");
        shared::symlink(&include, &link, true).map_err(|e| io_err("symlink", &link, e))?;
    }
    Ok(())
}

/// The `node` the activated environment resolves, as a realpath.
fn resolved_node(h: &Host<'_>, env: &Env) -> Option<PathBuf> {
    // Uncached: the memo of `ToolProbe::find_exe` would keep a lookup made before the install.
    let found = snp_proc::resolve::find_executable(OsStr::new("node"), env, h.cwd)?;
    let abs = if found.is_absolute() {
        found
    } else {
        h.cwd.join(found)
    };
    snp_pycompat::ospath::realpath(abs.as_os_str(), h.cwd).ok()
}

/// `install_environment` (`node.py:76-106`).
fn install_env(cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
    let h = cx.cx.host;
    let prefix = cx.cx.prefix;
    let env_dir = cx.cx.env_dir;
    let mut pkgs: Vec<OsString> = Vec::new();
    if prefix.join(".git").exists() && prefix.join("package.json").exists() {
        let mut spec = OsString::from("git+file://");
        spec.push(prefix.as_os_str());
        pkgs.push(spec);
    }
    pkgs.extend(cx.deps.iter().map(|d| OsString::from(&**d)));
    if pkgs.is_empty() {
        return Err(LangError::Assertion(
            messages::NOTHING_TO_INSTALL.to_owned(),
        ));
    }
    let (_, lib_dir) = layout(env_dir);
    mkdirs(&bin_dir(env_dir))?;
    mkdirs(&env_dir.join(lib_dir).join("node_modules"))?;
    let mut tools_used: Vec<(Arc<str>, Arc<str>)> = Vec::new();
    if let Some(requested) = requested_node(h, cx.cx.version) {
        let (tool, name) = node_toolchain(h, &requested)?;
        link_toolchain(&tool.dir, env_dir)?;
        tools_used.push((Arc::from(name), Arc::from(tool.version.as_str())));
    }
    let activated = no_git_env(h.base_env).patched(&env_patch(env_dir));
    let mut cmd: Vec<OsString> = [
        "npm",
        "install",
        "--allow-git=root",
        "--install-links",
        "-g",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    cmd.extend(pkgs);
    setup_cmd(h.runner, &cmd, &activated, prefix)?;
    let run_env = h.base_env.patched(&env_patch(env_dir));
    let fingerprint = resolved_node(h, &run_env).and_then(|p| h.probe.fingerprint(&p));
    Ok(InstallOutcome {
        installer: Arc::from("npm"),
        fingerprint,
        tools: tools_used,
    })
}

/// The fast path: the activated environment still resolves `node` to the fingerprinted executable, unchanged.
fn fast_healthy(cx: &EnvCtx<'_>, env: &Env, fp: &Fingerprint) -> bool {
    let Some(real) = resolved_node(cx.host, env) else {
        return false;
    };
    let Some(now) = cx.host.probe.fingerprint(&real) else {
        return false;
    };
    real == fp.exe
        && now.len == fp.len
        && now.mtime_ns == fp.mtime_ns
        && now.ino_or_index == fp.ino_or_index
}

impl Language for Node {
    fn id(&self) -> LanguageId {
        LanguageId::Node
    }

    /// Any `language_version`, any `additional_dependencies`.
    fn support(&self) -> Support {
        default_support(LanguageId::Node)
    }

    /// `get_default_version()` (`node.py:25-35`): `default` on Windows; `system` when `node` and `npm` exist outside
    /// `$HOME` and shims; else `default`.
    fn default_version(&self, h: &Host<'_>) -> Result<Arc<str>, LangError> {
        h.probe
            .default_version(LanguageId::Node, h.base_env, h.cwd, &mut || {
                let system = !cfg!(windows) && system_exe(h, "node") && system_exe(h, "npm");
                Ok(Arc::from(if system { "system" } else { "default" }))
            })
    }

    /// `install_environment` (see the module docs).
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        install_env(cx)
    }

    /// The fingerprint fast path, else `node --version` under the activated environment (`node.py:67-73`).
    fn health(
        &self,
        cx: &EnvCtx<'_>,
        recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        let env = cx.host.base_env.patched(&env_patch(cx.env_dir));
        if let Some(fp) = recorded.and_then(|s| s.fingerprint.as_ref())
            && fast_healthy(cx, &env, fp)
        {
            return Ok(Health::Healthy);
        }
        let argv = [OsString::from("node"), OsString::from("--version")];
        let out = cx.host.runner.run(&argv, &env, cx.host.cwd)?;
        Ok(if out.status.0 == 0 {
            Health::Healthy
        } else {
            Health::Unhealthy(messages::version_failed(out.status.0))
        })
    }

    /// `get_env_patch(envdir)`.
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(env_patch(cx.env_dir))
    }

    /// `basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
