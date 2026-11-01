//! `snp self alias install|uninstall|status` and `snp self check` (11 §A.9).
//!
//! Owner: WP-33.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use snp_cli::SelfArgs;
use snp_core::brand::Brand;
use snp_core::failure::Failure;
use snp_core::profile;
use snp_core::stage::HookType;
use snp_git::hookscripts::{Owner, classify};

use crate::scripts::{self, snp_version_of, version_probe};
use crate::{CmdCtx, frame, messages};

/// `pre-commit[.exe]`.
fn alias_name() -> String {
    format!("pre-commit{}", std::env::consts::EXE_SUFFIX)
}

/// `os.path.lexists`.
fn lexists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// `<alias>.snp-backup`.
fn backup_of(alias: &Path) -> PathBuf {
    let mut s = alias.as_os_str().to_os_string();
    s.push(".snp-backup");
    PathBuf::from(s)
}

/// What a `pre-commit` file is (11 §A.9.2 `{kind}`).
fn kind_of(file: &Path, current: &Path) -> String {
    if snp_os::same_file(file, current).unwrap_or(false) {
        return "snp alias (this snp)".to_owned();
    }
    match version_probe(file) {
        Some(line) => match snp_version_of(&line) {
            Some(v) => format!("snp alias (snp {v})"),
            None => match line.strip_prefix("pre-commit ") {
                Some(v) if !v.is_empty() && !v.contains(' ') => format!("Python pre-commit {v}"),
                _ => "unknown".to_owned(),
            },
        },
        None => "unknown".to_owned(),
    }
}

/// The first `pre-commit[.exe]` on PATH that is a regular executable file (`command -v` emulation, 11 §A.19.5).
fn resolve_on_path(cx: &CmdCtx<'_>, name: &str) -> Option<PathBuf> {
    let path = cx.env.get("PATH")?;
    std::env::split_paths(path)
        .map(|d| {
            if d.as_os_str().is_empty() {
                cx.cwd.join(name)
            } else {
                d.join(name)
            }
        })
        .find(|c| is_executable(c))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_file())
}

/// The default alias directory: the parent of the stable snp path (11 §A.19.3).
fn default_dir(cx: &CmdCtx<'_>) -> PathBuf {
    let stable = scripts::select_exe(Brand::Snp, cx.arg0, cx.env, cx.self_exe);
    stable
        .parent()
        .map_or_else(|| cx.cwd.to_path_buf(), Path::to_path_buf)
}

/// How the alias was made.
#[derive(Debug, Clone, Copy)]
enum Made {
    #[cfg_attr(
        not(unix),
        expect(dead_code, reason = "the symlink is the Unix alias form")
    )]
    Symlink,
    #[cfg_attr(
        unix,
        expect(
            dead_code,
            reason = "hard links and copies are the Windows alias forms"
        )
    )]
    HardLink,
    #[cfg_attr(
        unix,
        expect(
            dead_code,
            reason = "hard links and copies are the Windows alias forms"
        )
    )]
    Copy,
}

/// Creates `tmp` as a link to (or copy of) `target`.
fn make_link(target: &Path, tmp: &Path) -> io::Result<Made> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, tmp).map(|()| Made::Symlink)
    }
    #[cfg(not(unix))]
    {
        match fs::hard_link(target, tmp) {
            Ok(()) => Ok(Made::HardLink),
            Err(_) => fs::copy(target, tmp).map(|_| Made::Copy),
        }
    }
}

fn io_fail(e: &io::Error, p: &Path) -> Failure {
    Failure::from_io(e, Some(p.as_os_str()), None)
}

/// `snp self alias install [--dir DIR] [--force]` (11 §A.9.2, §A.19.7): Unix symlink `DIR/pre-commit` → the
/// absolute stable snp path; Windows hard link `DIR\pre-commit.exe` → `snp.exe` (a copy across volumes). An
/// existing `pre-commit` that is not snp is refused (stderr, exit 1) unless `--force`, which keeps it as
/// `<alias>.snp-backup` (refused when that exists). Created atomically (temp name + rename). Afterwards the
/// `command -v pre-commit` resolution is checked and a shadowing earlier PATH entry is warned about (stderr).
fn alias_install(cx: &CmdCtx<'_>, dir: Option<&OsStr>, force: bool) -> Result<i32, Failure> {
    let target = scripts::select_exe(Brand::Snp, cx.arg0, cx.env, cx.self_exe);
    let target = std::path::absolute(&target).unwrap_or(target);
    let dir = dir.map_or_else(|| default_dir(cx), |d| cx.cwd.join(d));
    let alias = dir.join(alias_name());
    if lexists(&alias) && !scripts::is_snp(&alias, cx.self_exe) {
        if !force {
            let kind = kind_of(&alias, cx.self_exe);
            let mut line = messages::alias_refused(alias.as_os_str(), &kind);
            line.push(b'\n');
            cx.printer.write_stderr(&line);
            return Ok(1);
        }
        let backup = backup_of(&alias);
        if lexists(&backup) {
            let mut line = messages::alias_backup_exists(backup.as_os_str());
            line.push(b'\n');
            cx.printer.write_stderr(&line);
            return Ok(1);
        }
        snp_os::fs::rename_with_retry(&alias, &backup).map_err(|e| io_fail(&e, &alias))?;
    }
    fs::create_dir_all(&dir).map_err(|e| io_fail(&e, &dir))?;
    let tmp = dir.join(format!(".pre-commit.snp-tmp-{}", std::process::id()));
    let _stale: io::Result<()> = fs::remove_file(&tmp);
    let made = make_link(&target, &tmp).map_err(|e| io_fail(&e, &alias))?;
    if let Err(e) = snp_os::fs::rename_with_retry(&tmp, &alias) {
        let _cleanup: io::Result<()> = fs::remove_file(&tmp);
        if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied {
            cx.printer.diag(messages::ALIAS_BUSY);
            return Ok(1);
        }
        return Err(io_fail(&e, &alias));
    }
    let line = match made {
        Made::Symlink => messages::alias_created_symlink(alias.as_os_str(), target.as_os_str()),
        Made::HardLink => messages::alias_created_hardlink(alias.as_os_str(), target.as_os_str()),
        Made::Copy => messages::alias_created_copy(alias.as_os_str(), target.as_os_str()),
    };
    cx.printer.write_line(&line);
    if let Some(found) = resolve_on_path(cx, &alias_name())
        && !snp_os::same_file(&found, &alias).unwrap_or(false)
    {
        let mut line = messages::alias_shadowed(found.as_os_str(), dir.as_os_str());
        line.push(b'\n');
        cx.printer.write_stderr(&line);
    }
    Ok(0)
}

/// `snp self alias uninstall`: removes the alias only when it is snp, restoring `<alias>.snp-backup`.
fn alias_uninstall(cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    let alias = default_dir(cx).join(alias_name());
    if !lexists(&alias) {
        cx.printer
            .write_line(&messages::alias_absent(alias.as_os_str()));
        return Ok(0);
    }
    if !scripts::is_snp(&alias, cx.self_exe) {
        let mut line = messages::alias_not_ours(alias.as_os_str());
        line.push(b'\n');
        cx.printer.write_stderr(&line);
        return Ok(1);
    }
    fs::remove_file(&alias).map_err(|e| io_fail(&e, &alias))?;
    cx.printer
        .write_line(&messages::alias_removed(alias.as_os_str()));
    let backup = backup_of(&alias);
    if lexists(&backup) {
        snp_os::fs::rename_with_retry(&backup, &alias).map_err(|e| io_fail(&e, &backup))?;
        cx.printer
            .write_line(&messages::alias_restored(alias.as_os_str()));
    }
    Ok(0)
}

/// `snp self alias status`: `pre-commit -> {resolved path} ({kind})`.
fn alias_status(cx: &CmdCtx<'_>) -> i32 {
    match resolve_on_path(cx, &alias_name()) {
        Some(found) => {
            let kind = kind_of(&found, cx.self_exe);
            cx.printer
                .write_line(&messages::alias_status(found.as_os_str(), &kind));
        }
        None => cx.printer.write_line(messages::ALIAS_NOT_FOUND.as_bytes()),
    }
    0
}

/// One `self check` result.
#[derive(Debug)]
struct Check {
    ok: bool,
    name: &'static str,
    detail: String,
}

fn check(name: &'static str, ok: bool, detail: impl Into<String>) -> Check {
    Check {
        ok,
        name,
        detail: detail.into(),
    }
}

/// The embedded vectors: a few known answers of every emulation layer the hot path relies on.
fn vector_checks() -> Vec<Check> {
    let mut out = Vec::new();
    let quoted = snp_pycompat::shlex::quote("it's a $x");
    out.push(check(
        "shlex",
        quoted == "'it'\"'\"'s a $x'",
        format!("quote(\"it's a $x\") = {quoted}"),
    ));
    let split = snp_pycompat::shlex::split("a 'b c' \"d\\\"e\"");
    let want: Vec<String> = ["a", "b c", "d\"e"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    out.push(check(
        "shlex.split",
        split.as_ref().is_ok_and(|v| *v == want),
        format!("{split:?}"),
    ));
    let re = snp_pyre::PyRegex::compile(r"^(src|tests)/.*\.py$", snp_pyre::Options::CONFIG);
    let re_ok = re.as_ref().is_ok_and(|r| {
        r.search("src/a/b.py") == Ok(true)
            && r.search("docs/x.py") == Ok(false)
            && r.search("src/a.pyc") == Ok(false)
    });
    out.push(check("re", re_ok, r"^(src|tests)/.*\.py$ over 3 paths"));
    let yaml = snp_yaml::load("a: [1, yes, '1.0', ~]\n", &snp_yaml::Limits::default());
    out.push(check(
        "yaml",
        yaml.as_ref().is_ok_and(|v| {
            let s = format!("{v:?}");
            s.contains("Bool(true)") || s.contains("true")
        }),
        "PyYAML 1.1 resolver (yes → True)",
    ));
    out.push(check(
        "version",
        Brand::PreCommit.version_line() == format!("pre-commit {}", profile::PRE_COMMIT),
        format!(
            "snp {} (pre-commit {} compatible)",
            profile::SNP_VERSION,
            profile::PRE_COMMIT
        ),
    ));
    out
}

/// The hook-script round trip: render, write, classify as the current snp script.
fn script_check() -> Check {
    let rendered = scripts::render(
        OsStr::new(".pre-commit-config.yaml"),
        HookType::PreCommit,
        false,
        Path::new("/usr/bin/snp"),
    );
    let result = tempfile::tempdir().and_then(|d| {
        let p = d.path().join("pre-commit");
        fs::write(&p, &rendered)?;
        classify(&p)
    });
    match result {
        Ok(Owner::Snp { stale: false }) => {
            check("hook script", true, "rendered script classifies as snp")
        }
        Ok(o) => check(
            "hook script",
            false,
            format!("rendered script classifies as {o:?}"),
        ),
        Err(e) => check("hook script", false, format!("temporary directory: {e}")),
    }
}

/// Capability probes: git, the store, the terminal, the CPU count.
fn capability_checks(cx: &CmdCtx<'_>) -> Vec<Check> {
    let mut out = Vec::new();
    match cx.git_exe {
        Some(exe) => {
            let v = exe
                .caps
                .version
                .map_or_else(|| "unknown version".to_owned(), |v| v.to_string());
            out.push(check(
                "git",
                exe.caps.version.is_some(),
                format!(
                    "{} {v}; config hooks: {}",
                    exe.argv0
                        .first()
                        .map(|a| a.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    if exe.caps.config_hooks { "yes" } else { "no" }
                ),
            ));
        }
        None => out.push(check("git", false, "git not found on PATH")),
    }
    let store = &cx.store.location().path;
    let detail = if cx.store.exists() {
        format!("{} (exists)", store.display())
    } else {
        format!("{} (not created yet)", store.display())
    };
    out.push(check("store", true, detail));
    out.push(check(
        "cpu",
        true,
        format!("{} (sched_getaffinity)", snp_proc::xargs::cpu_count()),
    ));
    out.push(check(
        "terminal",
        true,
        format!(
            "stdout tty: {}, columns: {}",
            snp_os::term::stdout_is_tty(),
            snp_os::term::stdout_tty_columns().map_or_else(|| "-".to_owned(), |c| c.to_string())
        ),
    ));
    out
}

/// `snp self check`: the embedded vector self-test and capability probes, one `ok`/`FAIL` line each, exit 1 when
/// any vector fails (a capability that is merely absent, like git, is reported but only fails without git).
fn self_check(cx: &CmdCtx<'_>) -> i32 {
    let mut checks = vector_checks();
    checks.push(script_check());
    checks.extend(capability_checks(cx));
    let mut failed = 0_usize;
    for c in &checks {
        if !c.ok {
            failed = failed.saturating_add(1);
        }
        let level = if c.ok { "ok   " } else { "FAIL " };
        cx.printer
            .write_line(format!("{level}{}: {}", c.name, c.detail).as_bytes());
    }
    let summary = format!(
        "{} check(s), {failed} failed; snp {} at {}",
        checks.len(),
        profile::SNP_VERSION,
        cx.self_exe.display()
    );
    cx.printer.write_line(summary.as_bytes());
    i32::from(failed != 0)
}

/// `snp self …`.
///
/// `alias install|uninstall|status` deliver the `pre-commit` name on request only (11 §A.9.1–§A.9.2; never by
/// default, ADR-049); `check` is the embedded vector self-test plus capability probes (ARCHITECTURE §9.3).
pub fn self_cmd(a: &SelfArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    match a {
        SelfArgs::AliasInstall { dir, force } => alias_install(cx, dir.as_deref(), *force),
        SelfArgs::AliasUninstall => alias_uninstall(cx),
        SelfArgs::AliasStatus => Ok(alias_status(cx)),
        SelfArgs::Check => Ok(self_check(cx)),
        _ => Err(frame::unsupported("self")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_vectors_pass() {
        for c in vector_checks() {
            assert!(c.ok, "{}: {}", c.name, c.detail);
        }
        let c = script_check();
        assert!(c.ok, "{}", c.detail);
    }

    #[test]
    fn backup_name() {
        assert_eq!(
            backup_of(Path::new("/b/pre-commit")),
            Path::new("/b/pre-commit.snp-backup")
        );
    }
}
