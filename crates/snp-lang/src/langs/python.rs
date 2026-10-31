//! The `python` language backend (§6.2; `pre_commit/languages/python.py`).
//!
//! | pre-commit 4.6.2 | snp |
//! |---|---|
//! | `get_default_version`: `python{X.Y}`/`python{X}` of pre-commit's own interpreter on `PATH`, its `sys.executable` names, the `py` launcher, else `default` (`python.py:108-131`) | the same rules relative to a reference interpreter (`python3`, `python`; Windows `py -3`, `python`), memoised by [`crate::ToolProbe`] (persisted only for a native, non-proxy interpreter: a pyenv `#!` shim is probed in every process) |
//! | `virtualenv <env> [-p norm_version(v)]` with cwd `/`, then `python -mpip install . *deps` in the prefix under `in_env` | installer per environment, `SNP_PYTHON_INSTALLER = auto` (default) \| `uv` \| `pip` (ADR-073): **uv** `uv venv <env> --python <abs interp> --seed --no-project --no-config --no-python-downloads` and `uv pip install --python <env py> --no-config [translated pip config] [.] *deps`; **pip** `<interp> -m venv <env>` and pre-commit's `python -mpip install . *deps` (D-PERF-01) |
//! | an explicitly requested interpreter that is not installed: virtualenv fails, exit 3 | uv-managed CPython download into `tools-v1/python` (`SNP_PYTHON_DOWNLOADS=never` restores the failure; D-CRASH-19) |
//! | local hook without deps: `pip install .` of the placeholder package | skipped (D-PERF-02) |
//! | `health_check`: two interpreter spawns per environment and run | fingerprint fast path (zero spawns while the base interpreter is unchanged), else pre-commit's check accepting uv's `pyvenv.cfg` format |
//!
//! Activation (`get_env_patch`, `python.py:55-61`) and the run (`basic_run_hook`) are pre-commit's. The `UV_*` strip
//! list of 04a §11.5 applies to snp's own uv invocations only; the hook's environment keeps the user's variables, as
//! in pre-commit (the recorded run goldens compare the delta exactly).
//!
//! Owner: WP-24.

mod interp;
mod messages;
mod pipconf;
mod pyvenv;
pub(crate) mod shared;
mod uv;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::{Env, EnvPatch};
use snp_core::lang::LanguageId;

use self::interp::RefInterp;
use self::shared::{basic_plan, no_git_env, stdout_text};
use self::uv::Uv;
use crate::common::{bin_dir, setup_cmd};
use crate::framework::default_support;
use crate::{
    EnvCtx, Fingerprint, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError,
    Language, PlanCtx, RunPlan, Support,
};

/// `language: python` (`pre_commit/languages/python.py`).
#[derive(Debug)]
pub struct Python;

/// `SNP_PYTHON_INSTALLER` (ADR-073).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Mode {
    /// uv unless a fallback trigger fires (the default).
    Auto,
    /// uv, failing instead of falling back.
    Uv,
    /// pre-commit's installer.
    Pip,
}

impl Mode {
    /// The variable's value (unset or empty: `auto`).
    fn from_env(env: &Env) -> Result<Self, LangError> {
        let Some(v) = env.get_nonempty("SNP_PYTHON_INSTALLER") else {
            return Ok(Self::Auto);
        };
        match v.to_string_lossy().trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "uv" => Ok(Self::Uv),
            "pip" => Ok(Self::Pip),
            _ => Err(LangError::Fatal(messages::bad_installer(
                &v.to_string_lossy(),
            ))),
        }
    }

    /// The value as written.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Uv => "uv",
            Self::Pip => "pip",
        }
    }

    /// Whether an environment recorded with `installer` satisfies this mode (a different choice reinstalls).
    fn accepts(self, installer: &str) -> bool {
        match self {
            Self::Auto => installer == INSTALLER_UV || installer == INSTALLER_PIP_FALLBACK,
            Self::Uv => installer == INSTALLER_UV,
            Self::Pip => installer == INSTALLER_PIP,
        }
    }
}

/// `InstallState.installer` of a uv-built environment.
const INSTALLER_UV: &str = "uv";
/// … of an environment built with pip because `SNP_PYTHON_INSTALLER=pip`.
const INSTALLER_PIP: &str = "pip";
/// … of an environment built with pip because a fallback trigger fired under `auto`.
const INSTALLER_PIP_FALLBACK: &str = "pip:fallback";

/// The variables stripped from every uv invocation (04a §11.5): snp passes explicit flags, and these would change
/// which interpreter, project or resolution uv uses.
const UV_STRIP: [&str; 18] = [
    "UV_PYTHON",
    "UV_SYSTEM_PYTHON",
    "UV_PROJECT_ENVIRONMENT",
    "UV_PROJECT",
    "UV_MANAGED_PYTHON",
    "UV_NO_MANAGED_PYTHON",
    "UV_PYTHON_PREFERENCE",
    "UV_PYTHON_DOWNLOADS",
    "UV_CONSTRAINT",
    "UV_OVERRIDE",
    "UV_BUILD_CONSTRAINT",
    "UV_RESOLUTION",
    "UV_PRERELEASE",
    "UV_TORCH_BACKEND",
    "UV_VENV_SEED",
    "UV_VENV_CLEAR",
    "UV_CONFIG_FILE",
    "UV_NO_CONFIG",
];

/// The variables removed from `uv venv` and `uv python find/install` besides [`UV_STRIP`] (04a §11.4 step 4): an
/// active environment and the macOS framework launcher variables would change which interpreter uv picks or what
/// it reports.
const VENV_STRIP: [&str; 4] = [
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
    "__PYVENV_LAUNCHER__",
    "PYTHONEXECUTABLE",
];

/// `win_exe('python')`.
const fn python_exe() -> &'static str {
    if cfg!(windows) {
        "python.exe"
    } else {
        "python"
    }
}

/// `prefix.path(bin_dir(envdir), win_exe('python'))`.
fn env_python(env_dir: &Path) -> PathBuf {
    bin_dir(env_dir).join(python_exe())
}

/// `get_env_patch(venv)` (`python.py:55-61`).
fn env_patch(env_dir: &Path) -> EnvPatch {
    EnvPatch::default()
        .set("PIP_DISABLE_PIP_VERSION_CHECK", "1")
        .unset("PYTHONHOME")
        .set("VIRTUAL_ENV", env_dir.as_os_str())
        .prepend_path("PATH", [bin_dir(env_dir)])
}

/// The environment of every uv invocation: the installer env minus [`UV_STRIP`], with the isolated uv cache
/// (`cache-v1/uv`) unless the user chose one (`UV_CACHE_DIR`).
fn uv_env(h: &Host<'_>, base: &Env) -> Env {
    let mut env = base.without(&UV_STRIP);
    if env.get("UV_CACHE_DIR").is_none() {
        env = shared::with_var(
            &env,
            "UV_CACHE_DIR",
            h.tools.cache_dir("uv").into_os_string(),
        );
    }
    env
}

/// The root directory, the cwd of environment creation (`cmd_output_b(*venv_cmd, cwd='/')`).
fn root_dir() -> &'static Path {
    Path::new(if cfg!(windows) { "\\" } else { "/" })
}

/// `argv` as an owned vector.
fn argv<I, S>(items: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    items.into_iter().map(Into::into).collect()
}

/// How the environment gets built.
#[derive(Debug)]
enum Backend {
    /// uv.
    Uv(Uv),
    /// pip; the recorded installer name.
    Pip(&'static str),
}

/// The interpreter of a new environment.
#[derive(Debug)]
struct Interp {
    /// An absolute interpreter path, or `None` when uv chooses (discovery).
    path: Option<PathBuf>,
    /// The uv-managed interpreter directory snp downloaded it into (a shared toolchain reference).
    managed: bool,
}

/// Chooses the installer (04a §11.4 step 1, ADR-073).
fn choose_backend(
    cx: &InstallCtx<'_>,
    mode: Mode,
) -> Result<(Backend, Option<pipconf::Translation>), LangError> {
    let h = cx.cx.host;
    if mode == Mode::Pip {
        return Ok((Backend::Pip(INSTALLER_PIP), None));
    }
    let fallback = |reason: String| -> Result<(Backend, Option<pipconf::Translation>), LangError> {
        if mode == Mode::Uv {
            Err(LangError::Fatal(messages::uv_required(&reason)))
        } else {
            Ok((Backend::Pip(INSTALLER_PIP_FALLBACK), None))
        }
    };
    let translation = match pipconf::translate(&pipconf::collect(h.base_env), h.base_env) {
        Ok(t) => t,
        Err(origin) => return fallback(messages::untranslatable(&origin)),
    };
    if let Some(dep) = pipconf::unsupported_dep(cx.deps) {
        return fallback(messages::dash_dependency(&dep));
    }
    match uv::find(h, mode == Mode::Uv) {
        Ok(Some(uv)) => Ok((Backend::Uv(uv), Some(translation))),
        Ok(None) => fallback(messages::NO_UV.to_owned()),
        Err(LangError::Interrupted) => Err(LangError::Interrupted),
        Err(e) => fallback(format!("{}: {e}", messages::NO_UV)),
    }
}

/// Maps `language_version` to an interpreter (04a §11.4 step 2; pre-commit's `norm_version`, `python.py:148-165`).
fn resolve_interpreter(
    cx: &InstallCtx<'_>,
    reference: Option<&RefInterp>,
    uv: Option<&Uv>,
) -> Result<Interp, LangError> {
    let h = cx.cx.host;
    let version = cx.cx.version;
    let found = |p: PathBuf| Interp {
        path: Some(interp::absolute(&p, h.cwd)),
        managed: false,
    };
    // `default` and versions matching the reference interpreter: virtualenv's default, pre-commit's own
    // interpreter.
    if let Some(r) = reference
        && (version == "default" || interp::sys_executable_matches(version, &r.version_info))
    {
        return Ok(found(r.executable.clone()));
    }
    if version != "default" {
        if cfg!(windows)
            && let Some(p) = interp::py_launcher(h, version)?
        {
            return Ok(found(p));
        }
        if let Some(p) = h.probe.find_exe(OsStr::new(version), h.base_env, h.cwd) {
            return Ok(found(p));
        }
        let numeric = version
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        if numeric
            && let Some(p) =
                h.probe
                    .find_exe(OsStr::new(&format!("python{version}")), h.base_env, h.cwd)
        {
            return Ok(found(p));
        }
        let expanded = interp::expanduser(version, h.base_env);
        if interp::absolute(&expanded, h.cwd).is_file() {
            return Ok(found(expanded));
        }
    }
    let Some(uv) = uv else {
        return Err(LangError::Value(messages::no_interpreter(version)));
    };
    uv_find_python(cx, uv, version)
}

/// uv discovery, then (for an explicitly requested version) a uv-managed download (D-CRASH-19).
fn uv_find_python(cx: &InstallCtx<'_>, uv: &Uv, version: &str) -> Result<Interp, LangError> {
    let h = cx.cx.host;
    let env = uv_env(h, &no_git_env(h.base_env)).without(&VENV_STRIP);
    let request = (version != "default").then(|| interp::uv_request(version));
    let find_argv = |managed: bool| {
        let mut a = argv([
            uv.exe.as_os_str(),
            OsStr::new("python"),
            OsStr::new("find"),
            OsStr::new("--no-project"),
            OsStr::new("--no-config"),
            OsStr::new("--no-python-downloads"),
        ]);
        if managed {
            a.push(OsString::from("--managed-python"));
        }
        if let Some(r) = &request {
            a.push(OsString::from(r));
        }
        a
    };
    let out = h.runner.run(&find_argv(false), &env, root_dir())?;
    if out.status.0 == 0 {
        let p = stdout_text(&out.stdout);
        if !p.is_empty() {
            return Ok(Interp {
                path: Some(PathBuf::from(p)),
                managed: false,
            });
        }
    }
    let downloads_allowed = h
        .base_env
        .get("SNP_PYTHON_DOWNLOADS")
        .is_none_or(|v| !v.eq_ignore_ascii_case("never"));
    let Some(request) = request.as_ref().filter(|_| downloads_allowed) else {
        let cmd = find_argv(false);
        return Err(LangError::CalledProcess {
            cmd,
            rc: if out.status.0 == 0 { 1 } else { out.status.0 },
            stdout: out.stdout,
            stderr: out.stderr,
        });
    };
    let install_dir = h.tools.tool_dir("python", "uv");
    let env = shared::with_var(&env, "UV_PYTHON_INSTALL_DIR", install_dir.as_os_str());
    // A CPython snp downloaded earlier: found without the lock or a download.
    if install_dir.is_dir()
        && let Some(p) = managed_find(h, &find_argv(true), &env)?
    {
        return Ok(Interp {
            path: Some(p),
            managed: true,
        });
    }
    let lock_path = install_dir.parent().unwrap_or(&install_dir).to_path_buf();
    let _lock = h.tools.lock_tool("python", h.cancel).map_err(|f| match f {
        snp_core::failure::Failure::Interrupted(_) => LangError::Interrupted,
        other => shared::io_err(
            "lock tool directory",
            &lock_path,
            std::io::Error::other(format!("{other:?}")),
        ),
    })?;
    // Re-checked under the lock: another process may have installed it meanwhile.
    if install_dir.is_dir()
        && let Some(p) = managed_find(h, &find_argv(true), &env)?
    {
        return Ok(Interp {
            path: Some(p),
            managed: true,
        });
    }
    setup_cmd(
        h.runner,
        &argv([
            uv.exe.as_os_str(),
            OsStr::new("python"),
            OsStr::new("install"),
            OsStr::new("--no-config"),
            OsStr::new("--no-bin"),
            OsStr::new("--install-dir"),
            install_dir.as_os_str(),
            OsStr::new(request),
        ]),
        &env,
        root_dir(),
    )?;
    let p = stdout_text(&setup_cmd(h.runner, &find_argv(true), &env, root_dir())?);
    Ok(Interp {
        path: Some(PathBuf::from(p)),
        managed: true,
    })
}

/// `uv python find --managed-python …` in snp's `tools-v1/python/uv`: the interpreter when uv finds one.
fn managed_find(h: &Host<'_>, argv: &[OsString], env: &Env) -> Result<Option<PathBuf>, LangError> {
    let out = h.runner.run(argv, env, root_dir())?;
    let p = stdout_text(&out.stdout);
    Ok((out.status.0 == 0 && !p.is_empty()).then(|| PathBuf::from(p)))
}

/// Creates the environment and installs the hook repository and its dependencies.
fn install_env(cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
    let h = cx.cx.host;
    let env_dir = cx.cx.env_dir;
    let prefix = cx.cx.prefix;
    let mode = Mode::from_env(h.base_env)?;
    let (backend, translation) = choose_backend(cx, mode)?;
    let reference = interp::reference(h)?;
    let uv = match &backend {
        Backend::Uv(uv) => Some(uv),
        Backend::Pip(_) => None,
    };
    let chosen = resolve_interpreter(cx, reference.as_ref(), uv)?;
    let installer_env = no_git_env(h.base_env);
    let mut tools: Vec<(Arc<str>, Arc<str>)> = Vec::new();
    match &backend {
        Backend::Uv(uv) => {
            if let Some(t) = &uv.tool {
                tools.push(t.clone());
            }
            let mut venv = argv([uv.exe.as_os_str(), OsStr::new("venv"), env_dir.as_os_str()]);
            if let Some(p) = &chosen.path {
                venv.push(OsString::from("--python"));
                venv.push(p.as_os_str().to_os_string());
            }
            venv.extend(argv([
                "--seed",
                "--no-project",
                "--no-config",
                "--no-python-downloads",
            ]));
            let venv_env = uv_env(h, &installer_env).without(&VENV_STRIP);
            setup_cmd(h.runner, &venv, &venv_env, root_dir())?;
        }
        Backend::Pip(_) => {
            let Some(p) = &chosen.path else {
                return Err(LangError::Value(messages::no_interpreter(cx.cx.version)));
            };
            let venv = argv([
                p.as_os_str(),
                OsStr::new("-m"),
                OsStr::new("venv"),
                env_dir.as_os_str(),
            ]);
            setup_cmd(h.runner, &venv, &installer_env, root_dir())?;
        }
    }
    if chosen.managed {
        tools.push((Arc::from("python"), Arc::from("uv")));
    }
    // D-PERF-02: a local hook without dependencies would only install the placeholder package.
    if !(cx.is_local && cx.deps.is_empty()) {
        let activated = installer_env.patched(&env_patch(env_dir));
        match &backend {
            Backend::Uv(uv) => {
                let mut cmd = argv([
                    uv.exe.as_os_str(),
                    OsStr::new("pip"),
                    OsStr::new("install"),
                    OsStr::new("--python"),
                    env_python(env_dir).as_os_str(),
                    OsStr::new("--no-config"),
                ]);
                let mut env = uv_env(h, &activated);
                if let Some(t) = &translation {
                    cmd.extend(t.flags.iter().cloned());
                    for (k, v) in &t.env {
                        env = shared::with_var(&env, k, v.clone());
                    }
                }
                if !cx.is_local {
                    cmd.push(OsString::from("."));
                }
                cmd.extend(cx.deps.iter().map(|d| OsString::from(&**d)));
                setup_cmd(h.runner, &cmd, &env, prefix)?;
            }
            Backend::Pip(_) => {
                let mut cmd = argv(["python", "-mpip", "install", "."]);
                cmd.extend(cx.deps.iter().map(|d| OsString::from(&**d)));
                setup_cmd(h.runner, &cmd, &activated, prefix)?;
            }
        }
    }
    let installer: Arc<str> = Arc::from(match &backend {
        Backend::Uv(_) => INSTALLER_UV,
        Backend::Pip(name) => *name,
    });
    Ok(InstallOutcome {
        installer,
        fingerprint: install_fingerprint(h, env_dir, chosen.path.as_deref()),
        tools,
    })
}

/// The fingerprint of the base interpreter (the realpath of the chosen interpreter, else of the environment's
/// `python`, which is a symlink to it on POSIX), with the recorded `version_info` as detail.
fn install_fingerprint(h: &Host<'_>, env_dir: &Path, chosen: Option<&Path>) -> Option<Fingerprint> {
    let target = chosen.map_or_else(|| env_python(env_dir), Path::to_path_buf);
    let real = snp_pycompat::ospath::realpath(target.as_os_str(), h.cwd).unwrap_or(target);
    let mut fp = h.probe.fingerprint(&real)?;
    let cfg = pyvenv::read(&env_dir.join("pyvenv.cfg")).unwrap_or_default();
    fp.detail = Arc::from(pyvenv::recorded_version(&cfg).unwrap_or_default());
    Some(fp)
}

/// `_version_info(exe)` (`python.py:27-33`): `exe -S -c '…'` with the run's environment; the error text when it
/// fails (a missing executable fails without a spawn).
fn version_info(h: &Host<'_>, exe: &str) -> Result<String, LangError> {
    let cmd = argv([
        OsString::from(exe),
        OsString::from("-S"),
        OsString::from("-c"),
        OsString::from(interp::VERSION_INFO_PROG),
    ]);
    let out = h.runner.run(&cmd, h.base_env, h.cwd)?;
    Ok(if out.status.0 == 0 {
        stdout_text(&out.stdout)
    } else {
        messages::version_error(exe)
    })
}

/// pre-commit's `health_check` (`python.py:175-211`), accepting uv's and venv's `pyvenv.cfg` forms.
fn slow_health(cx: &EnvCtx<'_>) -> Result<Health, LangError> {
    let h = cx.host;
    let Some(cfg) = pyvenv::read(&cx.env_dir.join("pyvenv.cfg")) else {
        return Ok(Health::Unhealthy(messages::PYVENV_MISSING.to_owned()));
    };
    let Some(expected) = pyvenv::recorded_version(&cfg) else {
        return Ok(Health::Unhealthy(messages::VERSION_INFO_MISSING.to_owned()));
    };
    let py_exe = snp_pycompat::ospath::normpath(env_python(cx.env_dir).as_os_str());
    let py_exe = py_exe.to_string_lossy();
    let actual = version_info(h, &py_exe)?;
    if !pyvenv::versions_match(&actual, expected) {
        return Ok(Health::Unhealthy(messages::venv_mismatch(
            &actual, expected,
        )));
    }
    let Some(base) = pyvenv::get(&cfg, "base-executable") else {
        return Ok(Health::Healthy);
    };
    let base_version = version_info(h, base)?;
    if pyvenv::versions_match(&base_version, expected) {
        Ok(Health::Healthy)
    } else {
        Ok(Health::Unhealthy(messages::base_mismatch(
            &base_version,
            expected,
        )))
    }
}

/// The spawn-free fast path (04a §14.9): the recorded base interpreter is unchanged (same length, mtime and
/// inode/file index) and the environment's `pyvenv.cfg` and `python` exist.
fn fast_healthy(cx: &EnvCtx<'_>, fp: &Fingerprint) -> bool {
    let Some(now) = cx.host.probe.fingerprint(&fp.exe) else {
        return false;
    };
    now.len == fp.len
        && now.mtime_ns == fp.mtime_ns
        && now.ino_or_index == fp.ino_or_index
        && cx.env_dir.join("pyvenv.cfg").is_file()
        && std::fs::metadata(env_python(cx.env_dir)).is_ok()
}

impl Language for Python {
    fn id(&self) -> LanguageId {
        LanguageId::Python
    }

    /// Any `language_version`, any `additional_dependencies`.
    fn support(&self) -> Support {
        default_support(LanguageId::Python)
    }

    /// `get_default_version()` (`python.py:108-131`) relative to the reference interpreter; `default` when there is
    /// none.
    fn default_version(&self, h: &Host<'_>) -> Result<Arc<str>, LangError> {
        h.probe
            .default_version(LanguageId::Python, h.base_env, h.cwd, &mut || {
                let Some(r) = interp::reference(h)? else {
                    return Ok(Arc::from("default"));
                };
                let real = snp_pycompat::ospath::realpath(r.executable.as_os_str(), h.cwd)
                    .map_or_else(
                        |_| r.executable.clone().into_os_string(),
                        PathBuf::into_os_string,
                    );
                let mut find = |name: &str| {
                    h.probe
                        .find_exe(OsStr::new(name), h.base_env, h.cwd)
                        .is_some()
                };
                let mut launcher =
                    |name: &str| interp::py_launcher(h, name).ok().flatten().is_some();
                Ok(Arc::from(interp::default_from(
                    &r,
                    &real,
                    &mut find,
                    &mut launcher,
                )))
            })
    }

    /// `install_environment` (`python.py:214-228`) with the installer of ADR-073 (see the module docs).
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        install_env(cx)
    }

    /// The installer check (a different `SNP_PYTHON_INSTALLER` choice reinstalls), the fingerprint fast path, else
    /// pre-commit's `health_check`.
    fn health(
        &self,
        cx: &EnvCtx<'_>,
        recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        let mode = Mode::from_env(cx.host.base_env)?;
        if let Some(state) = recorded {
            if !mode.accepts(&state.installer) {
                return Ok(Health::Unhealthy(messages::installer_changed(
                    &state.installer,
                    mode.as_str(),
                )));
            }
            if let Some(fp) = &state.fingerprint
                && fast_healthy(cx, fp)
            {
                return Ok(Health::Healthy);
            }
        }
        slow_health(cx)
    }

    /// `get_env_patch(envdir)`: `PIP_DISABLE_PIP_VERSION_CHECK=1`, `PYTHONHOME` unset, `VIRTUAL_ENV`, `PATH`.
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(env_patch(cx.env_dir))
    }

    /// `basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
