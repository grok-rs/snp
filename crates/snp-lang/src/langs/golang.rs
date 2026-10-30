//! The `golang` language backend (§6.2; `pre_commit/languages/golang.py`).
//!
//! | pre-commit 4.6.2 | snp |
//! |---|---|
//! | `get_default_version`: `system` iff `exe_exists('go')` | the same (`SNP_PREFER_SYSTEM_TOOLCHAINS=1` drops the `$HOME` heuristic) |
//! | non-system: `go.dev/dl/?mode=json` `[0]` for `default`, else the version verbatim; `dl.google.com/go/go{v}.{os}-{arch}.{ext}` unverified, extracted into `<env>` and moved to `<env>/.go`; 404 ⇒ `ValueError` | the same URL and version rules, verified by the go.dev JSON sha256 (D-SEC-08), extracted once into the shared `tools-v1/go/<version>` and linked as `<env>/.go` (a private copy where links are unavailable); 404 ⇒ the same `ValueError` text |
//! | `go install ./...` then `go install <dep>` per dependency in the prefix, env = `no_git_env(environ + GOPATH=<env>)` − `GOBIN` (+ `GOTOOLCHAIN=local`, `GOROOT`, `PATH`), then `rmtree(<env>/pkg)` | the same (readonly-aware, contained `rmtree`) |
//! | `in_env`: `PATH=<env>/bin` (system) or `GOROOT`, `GOTOOLCHAIN=local`, `PATH=<env>/bin:<env>/.go/bin`; `GOPATH` not set | the same |
//!
//! Owner: WP-24.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use snp_core::env::{Env, EnvPatch};
use snp_core::lang::LanguageId;
use snp_net::NetError;
use snp_net::tools::{self, Tool};

use crate::common::{rmtree, setup_cmd};
use crate::framework::default_support;
use crate::langs::python::shared::{
    self, PATHSEP, ToolRef, basic_plan, ensure_tool, io_err, mkdirs, no_git_env, system_exe,
};
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// The texts of the golang backend (pre-commit 4.6.2's).
mod messages {
    /// `_install_go` on a 404 (`golang.py:96-103`): `ValueError`.
    pub(super) fn no_such_version(os: &str, arch: &str) -> String {
        format!("Could not find a version matching your system requirements (os={os}; arch={arch})")
    }
}

/// `language: golang`.
#[derive(Debug)]
pub struct Golang;

/// `get_env_patch(venv, version)` (`golang.py:70-85`).
fn env_patch(env_dir: &Path, version: &str) -> EnvPatch {
    let bin = env_dir.join("bin");
    if version == "system" {
        return EnvPatch::default().prepend_path("PATH", [bin]);
    }
    let goroot = env_dir.join(".go");
    EnvPatch::default()
        .set("GOROOT", goroot.as_os_str())
        .set("GOTOOLCHAIN", "local")
        .prepend_path("PATH", [bin, goroot.join("bin")])
}

/// The install environment (`golang.py:139-150`): `no_git_env(dict(os.environ, GOPATH=gopath))` without `GOBIN`;
/// for a managed toolchain also `GOTOOLCHAIN=local`, `GOROOT=<env>/.go` and `PATH=<env>/.go/bin:$PATH`.
fn install_env(base: &Env, env_dir: &Path, version: &str) -> Env {
    let env =
        no_git_env(&shared::with_var(base, "GOPATH", env_dir.as_os_str())).without(&["GOBIN"]);
    if version == "system" {
        return env;
    }
    let goroot = env_dir.join(".go");
    let mut path = goroot.join("bin").into_os_string();
    if let Some(old) = base.get("PATH") {
        path.push(PATHSEP);
        path.push(old);
    }
    let patch = EnvPatch::default()
        .set("GOTOOLCHAIN", "local")
        .set("GOROOT", goroot.as_os_str())
        .set("PATH", path);
    env.patched(&patch)
}

/// `_install_go(version, envdir)` (`golang.py:88-120`): the toolchain at `<env>/.go`; returns the shared tool
/// reference when `<env>/.go` links to `tools-v1/go/<version>`.
fn install_go(h: &Host<'_>, version: &str, env_dir: &Path) -> Result<Option<ToolRef>, LangError> {
    let target = tools::host_target();
    let tool = match ensure_tool(h, Tool::Go, "go", version, target) {
        Ok(t) => t,
        Err(LangError::Net(NetError::Http { status: 404, .. })) => {
            return Err(LangError::Value(messages::no_such_version(
                shared::os_name(target),
                shared::go_arch(target),
            )));
        }
        Err(e) => return Err(e),
    };
    mkdirs(env_dir)?;
    let goroot = env_dir.join(".go");
    if shared::symlink(&tool.dir, &goroot, true).is_ok() {
        return Ok(Some((Arc::from("go"), Arc::from(tool.version.as_str()))));
    }
    // No symbolic links (Windows without the privilege): a private copy, as pre-commit has.
    copy_tree(&tool.dir, &goroot)?;
    Ok(None)
}

/// A recursive copy of a toolchain directory (symlinks inside it are copied as links on POSIX).
fn copy_tree(from: &Path, to: &Path) -> Result<(), LangError> {
    mkdirs(to)?;
    let entries = std::fs::read_dir(from).map_err(|e| io_err("read directory", from, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_err("read directory", from, e))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let ft = entry.file_type().map_err(|e| io_err("stat", &src, e))?;
        if ft.is_dir() {
            copy_tree(&src, &dst)?;
        } else if ft.is_symlink() {
            let target = std::fs::read_link(&src).map_err(|e| io_err("read link", &src, e))?;
            shared::symlink(&target, &dst, false).map_err(|e| io_err("symlink", &dst, e))?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| io_err("copy", &dst, e))?;
        }
    }
    Ok(())
}

impl Language for Golang {
    fn id(&self) -> LanguageId {
        LanguageId::Golang
    }

    /// Any `language_version`, any `additional_dependencies`.
    fn support(&self) -> Support {
        default_support(LanguageId::Golang)
    }

    /// `get_default_version()` (`golang.py:62-67`).
    fn default_version(&self, h: &Host<'_>) -> Result<Arc<str>, LangError> {
        h.probe
            .default_version(LanguageId::Golang, h.base_env, h.cwd, &mut || {
                Ok(Arc::from(if system_exe(h, "go") {
                    "system"
                } else {
                    "default"
                }))
            })
    }

    /// `install_environment` (`golang.py:130-161`).
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        let h = cx.cx.host;
        let env_dir = cx.cx.env_dir;
        let version = cx.cx.version;
        let mut tools_used = Vec::new();
        if version != "system"
            && let Some(t) = install_go(h, version, env_dir)?
        {
            tools_used.push(t);
        }
        let env = install_env(h.base_env, env_dir, version);
        let mut cmd: Vec<OsString> = ["go", "install", "./..."]
            .into_iter()
            .map(OsString::from)
            .collect();
        setup_cmd(h.runner, &cmd, &env, cx.cx.prefix)?;
        for dep in cx.deps {
            cmd = ["go", "install"].into_iter().map(OsString::from).collect();
            cmd.push(OsString::from(&**dep));
            setup_cmd(h.runner, &cmd, &env, cx.cx.prefix)?;
        }
        let pkg = env_dir.join("pkg");
        if std::fs::symlink_metadata(&pkg).is_ok() {
            rmtree(&pkg, env_dir).map_err(|e| io_err("remove", &pkg, e))?;
        }
        Ok(InstallOutcome {
            installer: Arc::from("go"),
            fingerprint: None,
            tools: tools_used,
        })
    }

    /// `health_check = basic_health_check`: always healthy.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `get_env_patch(envdir, version)`.
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(env_patch(cx.env_dir, cx.version))
    }

    /// `basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
