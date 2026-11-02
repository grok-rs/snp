//! The `ruby` language backend (§6.2; `pre_commit/languages/ruby.py`).
//!
//! | pre-commit 4.7.0 | snp |
//! |---|---|
//! | `get_default_version`: `system` iff `exe_exists('ruby') and exe_exists('gem')` | the same (`SNP_PREFER_SYSTEM_TOOLCHAINS=1` drops the `$HOME`/`shims` heuristic) |
//! | non-system: the vendored `rbenv.tar.gz` (rbenv 1.3.2) extracted into the prefix and moved to `<env>`; for a version also `ruby-download.tar.gz` and `ruby-build.tar.gz` into `<env>/plugins`; `rbenv init -`, `rbenv download V` (falling back to `rbenv install V`), `rbenv rehash` | the same archives, embedded with their sha256 and extracted safely straight into `<env>` (D-SEC-08); the same commands |
//! | `gem build *prefix.star('.gemspec')` and `gem install --no-document --no-format-executable --no-user-install --install-dir <env>/gems --bindir <env>/gems/bin *prefix.star('.gem') *deps` in directory order | the same arguments, the `.gemspec`/`.gem` names sorted (04b §5.5) |
//! | `in_env`: `GEM_HOME`, `GEM_PATH` unset, `BUNDLE_IGNORE_CONFIG=1`, then `PATH` (+ `RBENV_ROOT`, `RBENV_VERSION`) | the same |
//!
//! On Windows only `system` is realistic: rbenv is a bash program, so a non-system install fails at `rbenv init -`
//! exactly as in pre-commit.
//!
//! Owner: WP-24.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;
use snp_net::NetError;
use snp_net::archive::{self, Extract};
use snp_net::tools::{self, Layout, Tool};

use crate::common::setup_cmd;
use crate::framework::default_support;
use crate::langs::python::shared::{basic_plan, io_err, mkdirs, no_git_env, system_exe};
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: ruby` (`pre_commit/languages/ruby.py`).
#[derive(Debug)]
pub struct Ruby;

/// `get_env_patch(venv, language_version)` (`ruby.py:40-72`).
fn env_patch(env_dir: &Path, version: &str) -> EnvPatch {
    let gems = env_dir.join("gems");
    let mut patch = EnvPatch::default()
        .set("GEM_HOME", gems.as_os_str())
        .unset("GEM_PATH")
        .set("BUNDLE_IGNORE_CONFIG", "1");
    if version == "system" {
        patch = patch.prepend_path("PATH", [gems.join("bin")]);
    } else {
        patch = patch.set("RBENV_ROOT", env_dir.as_os_str()).prepend_path(
            "PATH",
            [gems.join("bin"), env_dir.join("shims"), env_dir.join("bin")],
        );
    }
    if version != "system" && version != "default" {
        patch = patch.set("RBENV_VERSION", version.to_owned());
    }
    patch
}

/// `prefix.star(end)` (`prefix.py:16-17`), sorted: the names in the prefix directory ending with `end`.
fn star(prefix: &Path, end: &str) -> Result<Vec<OsString>, LangError> {
    let entries = std::fs::read_dir(prefix).map_err(|e| io_err("list directory", prefix, e))?;
    let mut out: Vec<OsString> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| io_err("list directory", prefix, e))?;
        let name = entry.file_name();
        if name.as_encoded_bytes().ends_with(end.as_bytes()) {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

/// Extracts the embedded helper `tool` (verified against its embedded sha256) at `dest` (which must not exist),
/// dropping the archive's top-level directory (`rbenv/`, `ruby-build/`, `ruby-download/`).
fn extract_helper(h: &Host<'_>, tool: Tool, dest: &Path) -> Result<(), LangError> {
    let artifact = tools::resolve(tool, "default", tools::host_target(), &NoFetch)?;
    let bytes = tools::embedded(&artifact.url).ok_or_else(|| {
        LangError::Net(NetError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} is not embedded", artifact.url),
        )))
    })?;
    let snp_net::Checksum::Sha256(want) = artifact.checksum else {
        return Err(LangError::Net(NetError::Io(std::io::Error::other(
            "unexpected checksum kind",
        ))));
    };
    let got: [u8; 32] = Sha256::digest(bytes).into();
    if got != want {
        return Err(LangError::Net(NetError::Checksum {
            url: artifact.url.clone(),
            expected: hex(&want),
            actual: hex(&got),
        }));
    }
    let parent = dest.parent().unwrap_or(dest);
    mkdirs(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| io_err("create temporary file", parent, e))?;
    tmp.write_all(bytes)
        .map_err(|e| io_err("write temporary file", tmp.path(), e))?;
    let strip = match artifact.layout {
        Layout::TarGz { strip } => strip.saturating_add(1),
        _ => 1,
    };
    let opts = Extract {
        strip_components: strip,
        allow_symlinks_inside: true,
        max_total_bytes: 256 << 20,
    };
    if h.cancel.is_cancelled() {
        return Err(LangError::Interrupted);
    }
    archive::tar_gz(tmp.path(), dest, &opts)?;
    Ok(())
}

/// Lower-case hex.
fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::new(), |mut s, x| {
        let _ignored = write!(s, "{x:02x}");
        s
    })
}

/// The embedded helpers resolve without the network; any other request is refused.
#[derive(Debug)]
struct NoFetch;

impl snp_net::Fetch for NoFetch {
    fn get_text(&self, _url: &str, _max: u64) -> Result<String, NetError> {
        Err(NetError::Offline)
    }

    fn get_verified(
        &self,
        _url: &str,
        _sum: &snp_net::Checksum,
        _dest_dir: &Path,
        _file_name: &str,
        _cancel: &snp_core::cancel::CancelToken,
    ) -> Result<std::path::PathBuf, NetError> {
        Err(NetError::Offline)
    }
}

/// `_install_rbenv(prefix, version)` (`ruby.py:88-101`).
fn install_rbenv(h: &Host<'_>, env_dir: &Path, version: &str) -> Result<(), LangError> {
    extract_helper(h, Tool::Rbenv, env_dir)?;
    if version != "default" {
        let plugins = env_dir.join("plugins");
        extract_helper(h, Tool::RubyDownload, &plugins.join("ruby-download"))?;
        extract_helper(h, Tool::RubyBuild, &plugins.join("ruby-build"))?;
    }
    Ok(())
}

/// `argv` from string parts.
fn cmd(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

impl Language for Ruby {
    fn id(&self) -> LanguageId {
        LanguageId::Ruby
    }

    /// Any `language_version`, any `additional_dependencies`.
    fn support(&self) -> Support {
        default_support(LanguageId::Ruby)
    }

    /// `get_default_version()` (`ruby.py:32-37`).
    fn default_version(&self, h: &Host<'_>) -> Result<Arc<str>, LangError> {
        h.probe
            .default_version(LanguageId::Ruby, h.base_env, h.cwd, &mut || {
                let system = system_exe(h, "ruby") && system_exe(h, "gem");
                Ok(Arc::from(if system { "system" } else { "default" }))
            })
    }

    /// `install_environment` (`ruby.py:115-145`).
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        let h = cx.cx.host;
        let env_dir = cx.cx.env_dir;
        let prefix = cx.cx.prefix;
        let version = cx.cx.version;
        let env = no_git_env(h.base_env).patched(&env_patch(env_dir, version));
        if version != "system" {
            install_rbenv(h, env_dir, version)?;
            setup_cmd(h.runner, &cmd(&["rbenv", "init", "-"]), &env, prefix)?;
            if version != "default"
                && let Err(e) = setup_cmd(
                    h.runner,
                    &cmd(&["rbenv", "download", version]),
                    &env,
                    prefix,
                )
            {
                if !matches!(e, LangError::CalledProcess { .. }) {
                    return Err(e);
                }
                setup_cmd(h.runner, &cmd(&["rbenv", "install", version]), &env, prefix)?;
            }
            setup_cmd(h.runner, &cmd(&["rbenv", "rehash"]), &env, prefix)?;
        }
        let mut build = cmd(&["gem", "build"]);
        build.extend(star(prefix, ".gemspec")?);
        setup_cmd(h.runner, &build, &env, prefix)?;
        let gems = env_dir.join("gems");
        let mut install = cmd(&[
            "gem",
            "install",
            "--no-document",
            "--no-format-executable",
            "--no-user-install",
            "--install-dir",
        ]);
        install.push(gems.as_os_str().to_os_string());
        install.push(OsString::from("--bindir"));
        install.push(gems.join("bin").into_os_string());
        install.extend(star(prefix, ".gem")?);
        install.extend(cx.deps.iter().map(|d| OsString::from(&**d)));
        setup_cmd(h.runner, &install, &env, prefix)?;
        Ok(InstallOutcome {
            installer: Arc::from("gem"),
            fingerprint: None,
            tools: Vec::new(),
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
