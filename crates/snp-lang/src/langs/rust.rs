//! The `rust` language backend (§6.2; `pre_commit/languages/rust.py`).
//!
//! | pre-commit 4.6.2 | snp |
//! |---|---|
//! | `get_default_version`: `system` iff `cargo --version` (cwd `/`) exits 0 | the same, memoised by [`crate::ToolProbe`] (a rustup proxy is never persisted) |
//! | non-system: `rustup` from `PATH` or `sh.rustup.rs`/`win.rustup.rs` (unverified script) installed into `<env>` with a throw-away `RUSTUP_HOME`, `rustup toolchain install`, then a **second** throw-away `RUSTUP_HOME` (the toolchain is downloaded twice and never kept) | `rustup` from `PATH`, else the rustup-init binary verified by its `.sha256`; one persistent `RUSTUP_HOME` in `tools-v1/rustup` shared by every environment, toolchains installed with `--profile minimal` (04a Q13) |
//! | `cli:` and library dependencies split into sets (arbitrary order) | config order (first occurrence) |
//! | `cargo add name@spec…` in the prefix (the per-deps clone), `cargo install --bins --root <env> <args>` per package | the same commands; the prefix is per dependency variant (ADR-025), so `cargo add` never leaks into another variant |
//! | `in_env`: `PATH=<env>/bin`, `RUSTUP_TOOLCHAIN` unless `system` | the same |
//!
//! Owner: WP-24.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::{Env, EnvPatch};
use snp_core::lang::LanguageId;
use snp_net::tools::{self, Tool};

use crate::common::setup_cmd;
use crate::framework::default_support;
use crate::langs::python::shared::{basic_plan, ensure_tool, mkdirs, no_git_env, with_var};
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: rust` (`pre_commit/languages/rust.py`).
#[derive(Debug)]
pub struct Rust;

/// The shared rustup tool directory: `tools-v1/rustup/<ROOT>/{cargo,rustup}` (`CARGO_HOME` holding the rustup
/// binary and its proxies, `RUSTUP_HOME` holding the toolchains). The layout version, not the rustup version: rustup
/// updates itself in place.
const RUSTUP_ROOT: &str = "v1";

/// `_rust_toolchain(language_version)` (`rust.py:43-48`): `default` → `stable`, else verbatim.
pub(crate) fn rust_toolchain(version: &str) -> &str {
    if version == "default" {
        "stable"
    } else {
        version
    }
}

/// `get_env_patch(target_dir, version)` (`rust.py:51-66`).
fn env_patch(env_dir: &Path, version: &str) -> EnvPatch {
    let patch = EnvPatch::default().prepend_path("PATH", [env_dir.join("bin")]);
    if version == "system" {
        patch
    } else {
        patch.set("RUSTUP_TOOLCHAIN", rust_toolchain(version).to_owned())
    }
}

/// The packages to `cargo install` (`rust.py:131-141`): `--path .` first, then each `cli:` dependency
/// (`cli:name[:version]`) in config order, without duplicates.
pub(crate) fn packages(deps: &[Arc<str>]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = vec![vec!["--path".to_owned(), ".".to_owned()]];
    for dep in deps {
        let Some(rest) = dep.strip_prefix("cli:") else {
            continue;
        };
        let (package, _, version) = snp_pycompat::text::partition(rest, ":");
        let args = if version.is_empty() {
            vec![package.to_owned()]
        } else {
            vec![
                package.to_owned(),
                "--version".to_owned(),
                version.to_owned(),
            ]
        };
        if !out.contains(&args) {
            out.push(args);
        }
    }
    out
}

/// The `cargo add` arguments of the library dependencies (`rust.py:69-80`): `name@spec`, `name@*` without a spec,
/// in config order without duplicates.
pub(crate) fn lib_crates(deps: &[Arc<str>]) -> Vec<String> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for dep in deps {
        if dep.starts_with("cli:") || seen.contains(&&**dep) {
            continue;
        }
        seen.push(dep);
        let (name, _, spec) = snp_pycompat::text::partition(dep, ":");
        out.push(format!(
            "{name}@{}",
            if spec.is_empty() { "*" } else { spec }
        ));
    }
    out
}

/// The persistent rustup: `(CARGO_HOME, RUSTUP_HOME)` of `tools-v1/rustup/v1`, rustup bootstrapped from `PATH` or the
/// verified rustup-init binary, and `toolchain` installed (`--profile minimal`) under the tool lock.
fn rustup_toolchain(
    h: &Host<'_>,
    env: &Env,
    toolchain: &str,
) -> Result<(PathBuf, PathBuf), LangError> {
    let root = h.tools.tool_dir("rustup", RUSTUP_ROOT);
    let cargo_home = root.join("cargo");
    let rustup_home = root.join("rustup");
    let lock_path = root.parent().unwrap_or(&root).to_path_buf();
    let _lock = h.tools.lock_tool("rustup", h.cancel).map_err(|f| match f {
        snp_core::failure::Failure::Interrupted(_) => LangError::Interrupted,
        other => crate::langs::python::shared::io_err(
            "lock tool directory",
            &lock_path,
            std::io::Error::other(format!("{other:?}")),
        ),
    })?;
    mkdirs(&rustup_home)?;
    let env = with_var(
        &with_var(env, "CARGO_HOME", cargo_home.as_os_str()),
        "RUSTUP_HOME",
        rustup_home.as_os_str(),
    );
    let own = cargo_home.join("bin").join(if cfg!(windows) {
        "rustup.exe"
    } else {
        "rustup"
    });
    let rustup: OsString = if own.is_file() {
        own.into_os_string()
    } else if let Some(found) = h.probe.find_exe(OsStr::new("rustup"), &env, h.cwd) {
        found.into_os_string()
    } else {
        let init = ensure_tool(
            h,
            Tool::Rustup,
            "rustup-init",
            "default",
            tools::host_target(),
        )?;
        let exe = init.dir.join(if cfg!(windows) {
            "rustup-init.exe"
        } else {
            "rustup-init"
        });
        let cmd: Vec<OsString> = [
            exe.as_os_str(),
            OsStr::new("-y"),
            OsStr::new("--quiet"),
            OsStr::new("--no-modify-path"),
            OsStr::new("--default-toolchain"),
            OsStr::new("none"),
            OsStr::new("--profile"),
            OsStr::new("minimal"),
        ]
        .into_iter()
        .map(OsStr::to_os_string)
        .collect();
        setup_cmd(h.runner, &cmd, &env, h.cwd)?;
        own.into_os_string()
    };
    let cmd: Vec<OsString> = [
        rustup.as_os_str(),
        OsStr::new("toolchain"),
        OsStr::new("install"),
        OsStr::new("--no-self-update"),
        OsStr::new("--profile"),
        OsStr::new("minimal"),
        OsStr::new(toolchain),
    ]
    .into_iter()
    .map(OsStr::to_os_string)
    .collect();
    setup_cmd(h.runner, &cmd, &env, h.cwd)?;
    Ok((cargo_home, rustup_home))
}

impl Language for Rust {
    fn id(&self) -> LanguageId {
        LanguageId::Rust
    }

    /// Any `language_version`, any `additional_dependencies`.
    fn support(&self) -> Support {
        default_support(LanguageId::Rust)
    }

    /// `get_default_version()` (`rust.py:29-40`): `system` iff `cargo --version` exits 0 with cwd `/` (a bogus
    /// `rust-toolchain.toml` in the repository cannot break the probe; a rustup proxy without a toolchain fails it).
    fn default_version(&self, h: &Host<'_>) -> Result<Arc<str>, LangError> {
        h.probe
            .default_version(LanguageId::Rust, h.base_env, h.cwd, &mut || {
                let argv = [OsString::from("cargo"), OsString::from("--version")];
                let root = Path::new(if cfg!(windows) { "\\" } else { "/" });
                let out = h.runner.run(&argv, h.base_env, root)?;
                Ok(Arc::from(if out.status.0 == 0 {
                    "system"
                } else {
                    "default"
                }))
            })
    }

    /// `install_environment` (`rust.py:113-160`).
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        let h = cx.cx.host;
        let env_dir = cx.cx.env_dir;
        let version = cx.cx.version;
        let prefix = cx.cx.prefix;
        let mut env = no_git_env(h.base_env).patched(&env_patch(env_dir, version));
        let mut tools_used = Vec::new();
        if version != "system" {
            let (cargo_home, rustup_home) = rustup_toolchain(h, &env, rust_toolchain(version))?;
            // `cargo` resolves to the rustup proxy of the shared home, which finds the toolchain there.
            let patch = EnvPatch::default()
                .set("RUSTUP_HOME", rustup_home.as_os_str())
                .prepend_path("PATH", [env_dir.join("bin"), cargo_home.join("bin")]);
            env = no_git_env(h.base_env)
                .patched(
                    &EnvPatch::default()
                        .set("RUSTUP_TOOLCHAIN", rust_toolchain(version).to_owned()),
                )
                .patched(&patch);
            tools_used.push((Arc::from("rustup"), Arc::from(RUSTUP_ROOT)));
        }
        let crates = lib_crates(cx.deps);
        if !crates.is_empty() {
            let mut cmd: Vec<OsString> = vec![OsString::from("cargo"), OsString::from("add")];
            cmd.extend(crates.into_iter().map(OsString::from));
            setup_cmd(h.runner, &cmd, &env, prefix)?;
        }
        for args in packages(cx.deps) {
            let mut cmd: Vec<OsString> = ["cargo", "install", "--bins", "--root"]
                .into_iter()
                .map(OsString::from)
                .collect();
            cmd.push(env_dir.as_os_str().to_os_string());
            cmd.extend(args.into_iter().map(OsString::from));
            setup_cmd(h.runner, &cmd, &env, prefix)?;
        }
        Ok(InstallOutcome {
            installer: Arc::from("cargo"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn deps(v: &[&str]) -> Vec<Arc<str>> {
        v.iter().map(|s| Arc::from(*s)).collect()
    }

    #[test]
    fn toolchains() {
        assert_eq!(rust_toolchain("default"), "stable");
        assert_eq!(rust_toolchain("1.79.0"), "1.79.0");
        assert_eq!(rust_toolchain("nightly-2024-01-01"), "nightly-2024-01-01");
    }

    #[test]
    fn dependency_split_keeps_config_order() {
        let d = deps(&[
            "serde:1.0",
            "cli:shellharden:4.2.0",
            "rand",
            "cli:ripgrep",
            "serde:1.0",
            "cli:ripgrep",
        ]);
        assert_eq!(
            packages(&d),
            vec![
                vec!["--path".to_owned(), ".".to_owned()],
                vec![
                    "shellharden".to_owned(),
                    "--version".to_owned(),
                    "4.2.0".to_owned()
                ],
                vec!["ripgrep".to_owned()],
            ]
        );
        assert_eq!(
            lib_crates(&d),
            vec!["serde@1.0".to_owned(), "rand@*".to_owned()]
        );
        assert_eq!(lib_crates(&deps(&["cli:x"])).len(), 0);
    }
}
