//! `cargo xtask oracle install|lock`: the hash-locked pre-commit 4.6.2 oracles `target/oracle/pc462-py31{2,3}`
//! (WBS "Oracles and vectors", ADR-058, 10 §7.4.2).
//!
//! * `install [--py312 REQ] [--py313 REQ]` (re)creates both venvs with `uv venv`: `pc462-py312` on CPython 3.12.3
//!   (default request `3.12.3`) and `pc462-py313` on CPython ≥ 3.13.14 (default `>=3.13.14,<3.14`); a request is
//!   anything `uv venv --python` accepts (a version, a specifier or an interpreter path). Packages come only from
//!   `xtask/oracle/requirements-pc462.txt` (`--require-hashes`, wheels only, no uv config files). Each venv gets the
//!   `pre-commit-snp-prog` launcher, which patches argparse's `prog='pre-commit'` to `snp` before
//!   `pre_commit.main` builds its parser (ADR-058). The interpreter version, `pre-commit --version`
//!   (`pre-commit 4.6.2`) and the launcher (`snp 4.6.2`) are verified.
//! * `lock` regenerates the hash lock from `requirements-pc462.in` (`uv pip compile --universal --generate-hashes`).
//!
//! Owner: WP-00.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail, ensure};

use crate::workspace;

/// The directory holding the lock files (relative to the workspace root).
const ORACLE_DIR: &str = "xtask/oracle";
const LOCK_IN: &str = "requirements-pc462.in";
const LOCK: &str = "requirements-pc462.txt";
/// The pinned oracle version (`pre-commit --version`).
const PRE_COMMIT_VERSION: &str = "4.6.2";

/// What the interpreter of one oracle must be.
#[derive(Debug, Clone, Copy)]
enum Wanted {
    /// Exactly this `major.minor.micro`.
    Exact(u32, u32, u32),
    /// This `major.minor` series, at least this micro.
    AtLeast(u32, u32, u32),
}

impl Wanted {
    fn accepts(self, v: (u32, u32, u32)) -> bool {
        match self {
            Self::Exact(a, b, c) => v == (a, b, c),
            Self::AtLeast(a, b, c) => v.0 == a && v.1 == b && v.2 >= c,
        }
    }
}

/// One oracle venv.
#[derive(Debug)]
struct Oracle {
    name: &'static str,
    flag: &'static str,
    default_request: &'static str,
    wanted: Wanted,
}

const ORACLES: [Oracle; 2] = [
    Oracle {
        name: "pc462-py312",
        flag: "--py312",
        default_request: "3.12.3",
        wanted: Wanted::Exact(3, 12, 3),
    },
    Oracle {
        name: "pc462-py313",
        flag: "--py313",
        default_request: ">=3.13.14,<3.14",
        wanted: Wanted::AtLeast(3, 13, 14),
    },
];

/// The launcher: pre-commit's console script with argparse `prog` patched to `snp` (ADR-058).
const LAUNCHER_BODY: &str = "# pre-commit 4.6.2 with argparse prog 'snp' (ADR-058). Installed by `cargo xtask oracle install`; do not edit.
import argparse

_init = argparse.ArgumentParser.__init__


def _snp_prog_init(self, *args, **kwargs):
    if kwargs.get('prog') == 'pre-commit':
        kwargs['prog'] = 'snp'
    _init(self, *args, **kwargs)


argparse.ArgumentParser.__init__ = _snp_prog_init

from pre_commit.main import main  # noqa: E402

raise SystemExit(main())
";

/// `bin` (Unix) / `Scripts` (Windows) of a venv.
fn venv_bin(venv: &Path) -> PathBuf {
    venv.join(if cfg!(windows) { "Scripts" } else { "bin" })
}

/// An executable inside the venv's bin directory.
fn venv_exe(venv: &Path, name: &str) -> PathBuf {
    let bin = venv_bin(venv);
    if cfg!(windows) {
        bin.join(format!("{name}.exe"))
    } else {
        bin.join(name)
    }
}

/// Runs `cmd`, failing with its output when it exits non-zero; returns stdout.
fn run_ok(cmd: &mut Command, what: &str) -> anyhow::Result<String> {
    let out = cmd.output().with_context(|| format!("running {what}"))?;
    if !out.status.success() {
        bail!(
            "{what} failed ({}):\n{}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `CPython 3.12.3` → (3, 12, 3).
fn parse_version(line: &str) -> Option<(u32, u32, u32)> {
    let v = line.trim().strip_prefix("CPython ")?;
    let mut it = v.split('.').map(str::parse::<u32>);
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some(Ok(a)), Some(Ok(b)), Some(Ok(c)), None) => Some((a, b, c)),
        _ => None,
    }
}

/// Writes the `pre-commit-snp-prog` launcher into the venv.
fn write_launcher(venv: &Path) -> anyhow::Result<PathBuf> {
    let bin = venv_bin(venv);
    if cfg!(windows) {
        let script = bin.join("pre-commit-snp-prog.py");
        std::fs::write(&script, LAUNCHER_BODY)?;
        let cmd = bin.join("pre-commit-snp-prog.cmd");
        std::fs::write(
            &cmd,
            "@\"%~dp0python.exe\" \"%~dp0pre-commit-snp-prog.py\" %*\r\n",
        )?;
        Ok(cmd)
    } else {
        let path = bin.join("pre-commit-snp-prog");
        let python = venv_exe(venv, "python");
        let text = format!("#!{}\n{LAUNCHER_BODY}", python.display());
        std::fs::write(&path, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(path)
    }
}

/// Creates and verifies one oracle venv.
fn install_one(root: &Path, o: &Oracle, request: &str) -> anyhow::Result<()> {
    let venv = root.join("target/oracle").join(o.name);
    if venv.exists() {
        std::fs::remove_dir_all(&venv)
            .with_context(|| format!("removing the old {}", venv.display()))?;
    }
    if let Some(parent) = venv.parent() {
        std::fs::create_dir_all(parent)?;
    }
    run_ok(
        Command::new("uv")
            .args(["venv", "--no-config", "--no-project", "--python", request])
            .arg(&venv)
            .current_dir(root),
        &format!("uv venv --python {request} ({})", o.name),
    )?;
    let python = venv_exe(&venv, "python");
    let version = run_ok(
        Command::new(&python).args([
            "-I",
            "-c",
            "import platform, sys; print(platform.python_implementation(), '.'.join(map(str, sys.version_info[:3])))",
        ]),
        "the venv interpreter",
    )?;
    let v = parse_version(&version)
        .with_context(|| format!("{}: unexpected interpreter `{}`", o.name, version.trim()))?;
    ensure!(
        o.wanted.accepts(v),
        "{}: interpreter is CPython {}.{}.{}, wanted {:?} (pass {} <request>)",
        o.name,
        v.0,
        v.1,
        v.2,
        o.wanted,
        o.flag
    );
    run_ok(
        Command::new("uv")
            .args([
                "pip",
                "install",
                "--no-config",
                "--require-hashes",
                "--only-binary",
                ":all:",
            ])
            .arg("--python")
            .arg(&python)
            .arg("-r")
            .arg(root.join(ORACLE_DIR).join(LOCK))
            .current_dir(root),
        &format!("uv pip install ({})", o.name),
    )?;
    let launcher = write_launcher(&venv)?;
    let want = format!("pre-commit {PRE_COMMIT_VERSION}");
    let got = run_ok(
        Command::new(venv_exe(&venv, "pre-commit")).arg("--version"),
        "pre-commit --version",
    )?;
    ensure!(
        got.trim_end() == want,
        "{}: `pre-commit --version` printed {got:?}",
        o.name
    );
    let got = run_ok(
        Command::new(&launcher).arg("--version"),
        "pre-commit-snp-prog --version",
    )?;
    let want_snp = format!("snp {PRE_COMMIT_VERSION}");
    ensure!(
        got.trim_end() == want_snp,
        "{}: `pre-commit-snp-prog --version` printed {got:?}",
        o.name
    );
    workspace::say(&format!(
        "oracle: {} ready (CPython {}.{}.{}, {want}, launcher {})",
        workspace::rel(root, &venv),
        v.0,
        v.1,
        v.2,
        workspace::rel(root, &launcher)
    ));
    Ok(())
}

/// `oracle install [--py312 REQ] [--py313 REQ]`.
fn install(root: &Path, args: &[OsString]) -> anyhow::Result<()> {
    let mut requests: Vec<String> = ORACLES
        .iter()
        .map(|o| o.default_request.to_owned())
        .collect();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let i = ORACLES
            .iter()
            .position(|o| flag == o.flag)
            .with_context(|| format!("unknown option {}", flag.to_string_lossy()))?;
        let value = it
            .next()
            .with_context(|| format!("{} needs a value", flag.to_string_lossy()))?;
        if let Some(slot) = requests.get_mut(i) {
            *slot = value.to_string_lossy().into_owned();
        }
    }
    run_ok(
        Command::new("uv").arg("--version"),
        "uv --version (is uv installed?)",
    )?;
    for (o, req) in ORACLES.iter().zip(&requests) {
        install_one(root, o, req)?;
    }
    Ok(())
}

/// `oracle lock`: regenerate the hash lock (universal, all platforms and both Python series).
fn lock(root: &Path) -> anyhow::Result<()> {
    run_ok(
        Command::new("uv")
            .args([
                "pip",
                "compile",
                "--no-config",
                "--universal",
                "--generate-hashes",
                "--python-version",
                "3.12",
                "--custom-compile-command",
                "cargo xtask oracle lock",
                LOCK_IN,
                "-o",
                LOCK,
            ])
            .current_dir(root.join(ORACLE_DIR)),
        "uv pip compile",
    )?;
    workspace::say(&format!("oracle: wrote {ORACLE_DIR}/{LOCK}"));
    Ok(())
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    match args.split_first() {
        Some((cmd, rest)) if cmd == "install" => install(&root, rest),
        Some((cmd, [])) if cmd == "lock" => lock(&root),
        _ => bail!(
            "usage: cargo xtask oracle install [--py312 REQUEST] [--py313 REQUEST] | oracle lock"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("CPython 3.12.3\n"), Some((3, 12, 3)));
        assert_eq!(parse_version("PyPy 3.10.1"), None);
        assert!(Wanted::Exact(3, 12, 3).accepts((3, 12, 3)));
        assert!(!Wanted::Exact(3, 12, 3).accepts((3, 12, 4)));
        assert!(Wanted::AtLeast(3, 13, 14).accepts((3, 13, 20)));
        assert!(!Wanted::AtLeast(3, 13, 14).accepts((3, 13, 5)));
        assert!(!Wanted::AtLeast(3, 13, 14).accepts((3, 14, 0)));
    }

    #[test]
    fn the_lock_pins_the_wbs_versions_with_hashes() {
        let root = workspace::root().unwrap();
        let lock = std::fs::read_to_string(root.join(ORACLE_DIR).join(LOCK)).unwrap();
        for pin in [
            "pre-commit==4.6.2",
            "identify==2.6.20",
            "cfgv==3.5.0",
            "pyyaml==6.0.3",
            "virtualenv==21.14.5",
            "nodeenv==1.11.0",
        ] {
            assert!(lock.contains(&format!("{pin} \\")), "{pin} missing");
        }
        let pins = lock.lines().filter(|l| l.contains("==")).count();
        let hashed = lock
            .lines()
            .filter(|l| l.contains("==") && l.ends_with('\\'))
            .count();
        assert_eq!(pins, hashed, "every pin carries hashes");
    }
}
