//! The `r` language backend (§6.2; `pre_commit/languages/r.py`).
//!
//! The environment is an renv project: `install_environment` (`r.py:190-242`) creates `<env>`, copies the
//! repository's `renv.lock` and `renv/` into it, and runs R scripts written to a temporary `script.R` (never `-e`, to
//! avoid Windows quoting bugs) with `cwd=<env>` inside the activated environment: the bootstrap script with
//! `Rscript --vanilla` (restores the lock file, writes `<env>/activate.R`, installs the repository itself when its
//! `DESCRIPTION` has a `Package` field), then with the renv options `--no-save --no-restore --no-site-file
//! --no-environ` the script recording the R version and, with dependencies, `renv::install(commandArgs(trailingOnly =
//! TRUE))` followed by the dependencies. Install and health use `$R_HOME/bin/Rscript` when `R_HOME` is set, else
//! `Rscript`; hooks always run `Rscript` from `PATH` (parity, 04b Q6).
//!
//! The health check (`r.py:86-107`) compares the recorded R version with the current one (two Rscript runs). snp
//! records the fingerprint of the Rscript executable at install time (`InstallOutcome::fingerprint`) and skips both
//! runs while that executable is unchanged (04b §5.8; ADR decision "R health stamp → yes (invisible)"). Only a native
//! Rscript that is not a version-manager proxy gets a fingerprint (its realpath): an asdf/mise shim or a `#!` wrapper
//! can switch R versions without changing, so those always take pre-commit's two runs.
//!
//! The prefix literal of the scripts ([`r_string_literal`]) equals Python's `repr` except for non-printable non-ASCII
//! characters and undecodable bytes, which get R escapes (`\U{…}`, `\xNN`) instead of Python's. COMPATIBILITY §5 has no register ID for that literal difference yet (allocation by the integration lead is pending).
//!
//! The hook's `entry` must be `Rscript <script>` or `Rscript -e <expr>` (`_entry_validate`, texts verbatim, including
//! the missing space before `or`); the script is joined to the repository unless the hook is local.
//!
//! Owner: WP-25.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::{Env, EnvPatch};
use snp_core::lang::LanguageId;

use super::docker::group2::{
    assert_support, copy_into, copytree, default_version, entry_error, makedirs, py_join, tempdir,
    win_exe, write_text, xargs,
};
use crate::common::{prefix_path, setup_cmd};
use crate::framework::default_support;
use crate::{
    EnvCtx, Fingerprint, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError,
    Language, PlanCtx, RunPlan, Support,
};

/// `language: r`.
#[derive(Debug)]
pub struct R;

/// `_RENV_ACTIVATED_OPTS` (`r.py:23-25`).
pub const RENV_ACTIVATED_OPTS: [&str; 4] = [
    "--no-save",
    "--no-restore",
    "--no-site-file",
    "--no-environ",
];

/// `_inline_r_setup`'s prelude (`r.py:245-259`), followed by `\n` and the dedented code.
const R_SETUP: &str = "options(\n    install.packages.compile.from.source = \"never\",\n    pkgType = \"binary\"\n)\n";

/// The bootstrap script after `prefix_dir <- <literal>\n` (`r.py:200-225`, dedented).
const INSTALL_BODY: &str = concat!(
    "options(\n",
    "    repos = c(CRAN = \"https://cran.rstudio.com\"),\n",
    "    renv.consent = TRUE\n",
    ")\n",
    "source(\"renv/activate.R\")\n",
    "renv::restore()\n",
    "activate_statement <- paste0(\n",
    "  'suppressWarnings({',\n",
    "  'old <- setwd(\"', getwd(), '\"); ',\n",
    "  'source(\"renv/activate.R\"); ',\n",
    "  'setwd(old); ',\n",
    "  'renv::load(\"', getwd(), '\");})'\n",
    ")\n",
    "writeLines(activate_statement, 'activate.R')\n",
    "is_package <- tryCatch(\n",
    "  {\n",
    "      path_desc <- file.path(prefix_dir, 'DESCRIPTION')\n",
    "      suppressWarnings(desc <- read.dcf(path_desc))\n",
    "      \"Package\" %in% colnames(desc)\n",
    "  },\n",
    "  error = function(...) FALSE\n",
    ")\n",
    "if (is_package) {\n",
    "    renv::install(prefix_dir)\n",
    "}\n",
);

/// `_read_installed_version`.
const READ_INSTALLED: &str = "cat(renv::settings$r.version())";
/// `_read_executable_version`.
const READ_CURRENT: &str = "cat(as.character(getRversion()))";
/// `_write_current_r_version`.
const WRITE_VERSION: &str = "renv::settings$r.version(as.character(getRversion()))";
/// The dependency install script (`r.py:236-241`).
const INSTALL_DEPS: &str = "renv::install(commandArgs(trailingOnly = TRUE))";

/// `ValueError` texts of `_entry_validate` (`r.py:157-174`), verbatim.
pub const ENTRY_NOT_RSCRIPT: &str = "entry must start with `Rscript`.";
/// See [`ENTRY_NOT_RSCRIPT`].
pub const ENTRY_TWO_EXPRESSIONS: &str = "You can supply at most one expression.";
/// See [`ENTRY_NOT_RSCRIPT`] (no space before `or`, as in pre-commit).
pub const ENTRY_SYNTAX: &str =
    "The only valid syntax is `Rscript -e {expr}`or `Rscript path/to/hook/script`";
/// `IndexError: list index out of range` of a one-token entry (`entry[1]`).
pub const ENTRY_INDEX: &str = "list index out of range";

/// The full text written to `script.R` for `code` (`_inline_r_setup(textwrap.dedent(code))`).
pub fn script_text(code: &str) -> String {
    format!("{R_SETUP}\n{code}")
}

/// The bootstrap script of `install_environment` for the repository at `prefix`.
pub fn install_script(prefix: &Path) -> String {
    script_text(&format!(
        "prefix_dir <- {}\n{INSTALL_BODY}",
        r_string_literal(prefix.as_os_str())
    ))
}

/// The repository path as an R string literal, equal to pre-commit's Python `repr(prefix_dir)` for every path whose
/// non-printable characters are ASCII: single quotes unless the text contains `'` and no `"`, `\\`, `\t`, `\n`, `\r`
/// and `\xNN` escapes. A non-printable non-ASCII character becomes `\U{…}` (Python's `\xNN` above `0x7f` would be a raw
/// byte in R) and a byte that is not UTF-8 becomes `\xNN` (the raw byte, which is what the path holds).
pub fn r_string_literal(s: &OsStr) -> String {
    let bytes = s.as_encoded_bytes();
    let text = String::from_utf8_lossy(bytes);
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            push_escaped(&mut out, c, quote);
        }
        for b in chunk.invalid() {
            let _ignored = write!(out, "\\x{b:02x}");
        }
    }
    out.push(quote);
    out
}

/// One character of [`r_string_literal`].
fn push_escaped(out: &mut String, c: char, quote: char) {
    match c {
        '\\' => out.push_str("\\\\"),
        '\t' => out.push_str("\\t"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        c if c == quote => {
            out.push('\\');
            out.push(c);
        }
        c if c.is_ascii() && (c < ' ' || c == '\x7f') => {
            let _ignored = write!(out, "\\x{:02x}", u32::from(c));
        }
        c if !c.is_ascii() && !snp_pycompat::chars::is_printable(c) => {
            let _ignored = write!(out, "\\U{{{:08x}}}", u32::from(c));
        }
        c => out.push(c),
    }
}

/// `_rscript_exec()` (`r.py:149-154`): `$R_HOME/bin/Rscript[.exe]` when `R_HOME` is set (even empty), else `Rscript`.
pub fn rscript_exec(env: &Env) -> OsString {
    match env.get("R_HOME") {
        Some(home) => py_join(
            Path::new(home),
            &[OsStr::new("bin"), OsStr::new(&win_exe("Rscript"))],
        )
        .into_os_string(),
        None => OsString::from("Rscript"),
    }
}

/// `get_env_patch(env)` (`r.py:123-127`): `R_PROFILE_USER=<env>/activate.R`, `RENV_PROJECT`⊘.
pub fn env_patch(env_dir: &Path) -> EnvPatch {
    EnvPatch::default()
        .set(
            "R_PROFILE_USER",
            env_dir.join("activate.R").into_os_string(),
        )
        .unset("RENV_PROJECT")
}

/// `_execute_r(code, cli_opts=…, args=…, cwd=<env>)` (`r.py:28-37`): writes `script.R` into a fresh temporary
/// directory and runs `<rscript> <opts…> <script> <args…>` activated in `<env>`; stdout decoded lossily
/// (D-CRASH-13) without trailing newlines.
fn execute_r(
    host: &Host<'_>,
    env_dir: &Path,
    code: &str,
    opts: &[&str],
    args: &[Arc<str>],
) -> Result<String, LangError> {
    let env = host.base_env.patched(&env_patch(env_dir));
    let tmp = tempdir(host.base_env)?;
    let script = tmp.path().join("script.R");
    write_text(&script, code)?;
    let mut argv: Vec<OsString> = vec![rscript_exec(&env)];
    argv.extend(opts.iter().map(OsString::from));
    argv.push(script.into_os_string());
    argv.extend(args.iter().map(|a| OsString::from(&**a)));
    let stdout = setup_cmd(host.runner, &argv, &env, env_dir)?;
    Ok(String::from_utf8_lossy(&stdout)
        .trim_end_matches('\n')
        .to_owned())
}

/// The Rscript executable install and health use, resolved (realpath) for its fingerprint. `None` (no fast path:
/// pre-commit's two Rscript runs every time) for a relative path and for an Rscript whose file can stay the same while
/// the R version behind it changes: a version-manager proxy (a `shims` directory: asdf, mise, rtx, pyenv-style
/// managers; a manager binary) or a script (`#!`; on Windows anything but `.exe`/`.com`). These are the guards of the
/// framework's persisted probe facts (`ToolProbe`, ARCHITECTURE §6.1).
fn rscript_fingerprint(host: &Host<'_>, env_dir: &Path) -> Option<Fingerprint> {
    let exe = rscript_exec(host.base_env);
    let found: PathBuf = host.probe.find_exe(&exe, host.base_env, env_dir)?;
    if !found.is_absolute() {
        return None;
    }
    let real = snp_pycompat::ospath::realpath(found.as_os_str(), env_dir).ok()?;
    if is_version_proxy(&found, &real) || !is_native(&real) {
        return None;
    }
    let mut fp = host.probe.fingerprint(&real)?;
    fp.detail = Arc::from("Rscript");
    Some(fp)
}

/// A version-manager proxy: a `shims` (or Chocolatey) directory on either path, or a manager's own binary.
fn is_version_proxy(found: &Path, real: &Path) -> bool {
    const MANAGERS: [&str; 11] = [
        "rustup",
        "rustup-init",
        "mise",
        "rtx",
        "volta",
        "volta-shim",
        "proto-shim",
        "asdf",
        "pyenv",
        "rbenv",
        "nodenv",
    ];
    let has_component = |p: &Path, name: &str| {
        p.components()
            .any(|c| matches!(c, std::path::Component::Normal(n) if n.eq_ignore_ascii_case(name)))
    };
    if [found, real]
        .iter()
        .any(|p| has_component(p, "shims") || has_component(p, "chocolatey"))
    {
        return true;
    }
    let stem = real
        .file_stem()
        .map_or_default(|s| s.to_string_lossy().to_ascii_lowercase());
    MANAGERS.contains(&stem.as_str())
}

/// A native executable: not a `#!` script (Windows: `.exe`/`.com`).
fn is_native(real: &Path) -> bool {
    use std::io::Read as _;
    if cfg!(windows) {
        return real
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("com"));
    }
    let mut head = [0_u8; 2];
    std::fs::File::open(real)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| head != *b"#!")
}

/// The two-spawn health check of pre-commit (`r.py:86-107`).
fn slow_health(host: &Host<'_>, env_dir: &Path) -> Result<Health, LangError> {
    let opts = &RENV_ACTIVATED_OPTS;
    let installed = execute_r(host, env_dir, &script_text(READ_INSTALLED), opts, &[])?;
    let current = execute_r(host, env_dir, &script_text(READ_CURRENT), opts, &[])?;
    if installed == "NULL" || installed.is_empty() {
        return Ok(Health::Unhealthy(format!(
            "Hooks were installed with an unknown R version. R version for hook repo now set to {current}"
        )));
    }
    if installed != current {
        return Ok(Health::Unhealthy(format!(
            "Hooks were installed for R version {installed}, but current R executable has version {current}"
        )));
    }
    Ok(Health::Healthy)
}

/// `_cmd_from_hook` (`r.py:177-187`): `shlex.split(entry)` (no hazmat rewrite), validated, then
/// `('Rscript', *opts, *part, *args)`.
fn hook_command(
    entry: &str,
    args: &[Arc<str>],
    prefix: &Path,
    is_local: bool,
) -> Result<Result<Vec<OsString>, RunPlan>, LangError> {
    let cmd = match snp_pycompat::shlex::split(entry) {
        Ok(cmd) => cmd,
        Err(e) => {
            return Ok(Err(entry_error(&snp_proc::hookcmd::HookCmdError::Shlex(e))));
        }
    };
    let Some(first) = cmd.first() else {
        // `entry[0]` of an empty split: the empty-entry C-hook (X13-EXE-01).
        return Ok(Err(entry_error(&snp_proc::hookcmd::HookCmdError::Empty)));
    };
    if first != "Rscript" {
        return Err(LangError::Value(ENTRY_NOT_RSCRIPT.to_owned()));
    }
    let Some(second) = cmd.get(1) else {
        return Err(LangError::Value(ENTRY_INDEX.to_owned()));
    };
    if second == "-e" {
        if cmd.len() > 3 {
            return Err(LangError::Value(ENTRY_TWO_EXPRESSIONS.to_owned()));
        }
    } else if cmd.len() > 2 {
        return Err(LangError::Value(ENTRY_SYNTAX.to_owned()));
    }
    let mut argv: Vec<OsString> = vec![OsString::from(first)];
    argv.extend(RENV_ACTIVATED_OPTS.iter().map(OsString::from));
    if second == "-e" || is_local {
        argv.extend(cmd.iter().skip(1).map(OsString::from));
    } else {
        argv.push(prefix_path(prefix, OsStr::new(second)));
    }
    argv.extend(args.iter().map(|a| OsString::from(&**a)));
    Ok(Ok(argv))
}

impl Language for R {
    fn id(&self) -> LanguageId {
        LanguageId::R
    }

    /// `assert_version_default('r', …)`; dependencies allowed.
    fn support(&self) -> Support {
        default_support(LanguageId::R)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(default_version())
    }

    /// `install_environment` (`r.py:190-242`), see the module docs; the outcome carries the Rscript fingerprint of
    /// the health fast path.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        assert_support(LanguageId::R, cx)?;
        let host = cx.cx.host;
        let prefix = cx.cx.prefix;
        let env_dir = cx.cx.env_dir;
        makedirs(env_dir)?;
        copy_into(
            &PathBuf::from(prefix_path(prefix, OsStr::new("renv.lock"))),
            env_dir,
        )?;
        copytree(
            &PathBuf::from(prefix_path(prefix, OsStr::new("renv"))),
            &env_dir.join("renv"),
        )?;
        execute_r(host, env_dir, &install_script(prefix), &["--vanilla"], &[])?;
        execute_r(
            host,
            env_dir,
            &script_text(WRITE_VERSION),
            &RENV_ACTIVATED_OPTS,
            &[],
        )?;
        if !cx.deps.is_empty() {
            execute_r(
                host,
                env_dir,
                &script_text(INSTALL_DEPS),
                &RENV_ACTIVATED_OPTS,
                cx.deps,
            )?;
        }
        Ok(InstallOutcome {
            installer: Arc::from("renv"),
            fingerprint: rscript_fingerprint(host, env_dir),
            tools: Vec::new(),
        })
    }

    /// `health_check` (`r.py:86-107`) behind the fingerprint fast path: healthy without spawning while the Rscript
    /// executable recorded at install time is unchanged (path, size, mtime, inode); otherwise pre-commit's two Rscript
    /// runs and its texts (a failing Rscript is a `CalledProcessError`, not "unhealthy", as in pre-commit).
    fn health(
        &self,
        cx: &EnvCtx<'_>,
        recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        if let Some(fp) = recorded.and_then(|s| s.fingerprint.as_ref())
            && let Some(now) = rscript_fingerprint(cx.host, cx.env_dir)
            && now.exe == fp.exe
            && now.len == fp.len
            && now.mtime_ns == fp.mtime_ns
            && now.ino_or_index == fp.ino_or_index
        {
            return Ok(Health::Healthy);
        }
        slow_health(cx.host, cx.env_dir)
    }

    /// `get_env_patch(env)`: `R_PROFILE_USER=<env>/activate.R`, `RENV_PROJECT`⊘.
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(env_patch(cx.env_dir))
    }

    /// `run_hook` (`r.py:262-278`): `_cmd_from_hook` (see the module docs); validation failures are pre-commit's run-time `ValueError`s
    /// (exit 3), an entry that does not split is the C-hook of the other languages.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(
            match hook_command(cx.entry, cx.args, cx.prefix, cx.is_local)? {
                Ok(argv) => xargs(argv, cx.env),
                Err(plan) => plan,
            },
        )
    }
}
