//! `cargo xtask identify-vectors [--python PATH]`: regenerate snp-identify's Python-parity vectors.
//!
//! Runs `xtask/oracle/gen_identify_vectors.py --identify <site-packages> --out
//! crates/snp-identify/tests/data/identify-vectors.json` under the pre-commit 4.7.0 oracle interpreter
//! (`target/oracle/pc470-py312`, whose site-packages hold the hash-locked identify 2.6.20 of
//! `xtask/oracle/requirements-pc470.txt`; `cargo xtask oracle install`) with `python -I`. The script imports and
//! runs identify on file names, shebangs, interpreters and files it creates in a temporary directory; the output is
//! committed and deterministic, and CI needs no Python (`tests/parity.rs` reads it). `--python` selects another
//! interpreter with identify installed. The tag tables themselves come from `cargo xtask identify-tables`.
//!
//! Owner: WP-00 (integration lead; ported from file-identify's `xtask/parity/gen_vectors.py`, ADR-116).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail};

use crate::workspace;

/// The generator (relative to the workspace root).
const SCRIPT: &str = "xtask/oracle/gen_identify_vectors.py";
/// The committed vectors (relative to the workspace root).
const OUT: &str = "crates/snp-identify/tests/data/identify-vectors.json";
/// The default oracle venv (relative to the workspace root).
const ORACLE: &str = "target/oracle/pc470-py312";

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    let Some(python) = oracle_python(&root, args, "identify-vectors")? else {
        return Ok(()); // --help
    };
    let site = site_packages(&python, &root)?;
    if !site.join("identify").join("identify.py").is_file() {
        bail!("{} has no identify package", site.display());
    }
    let status = Command::new(&python)
        .arg("-I")
        .arg(root.join(SCRIPT))
        .arg("--identify")
        .arg(&site)
        .arg("--out")
        .arg(root.join(OUT))
        .current_dir(&root)
        .status()
        .with_context(|| format!("running {}", python.display()))?;
    if !status.success() {
        bail!("{SCRIPT} failed ({status})");
    }
    workspace::say(&format!("identify-vectors: wrote {OUT}"));
    Ok(())
}

/// The `purelib` directory of `python` (where pip installed the oracle's packages).
pub(super) fn site_packages(python: &Path, root: &Path) -> anyhow::Result<PathBuf> {
    let out = Command::new(python)
        .args([
            "-I",
            "-c",
            "import sysconfig; print(sysconfig.get_paths()['purelib'])",
        ])
        .current_dir(root)
        .output()
        .with_context(|| format!("running {}", python.display()))?;
    if !out.status.success() {
        bail!(
            "{} could not report its site-packages ({})",
            python.display(),
            out.status
        );
    }
    let text = String::from_utf8(out.stdout).context("site-packages path is not UTF-8")?;
    Ok(PathBuf::from(text.trim_end()))
}

/// Parses `[--python PATH] [-h|--help]` for the oracle-vector tasks of the absorbed crates and returns the
/// interpreter (the pc470-py312 oracle by default), or `None` after printing the usage.
pub(super) fn oracle_python(
    root: &Path,
    args: &[OsString],
    task: &str,
) -> anyhow::Result<Option<PathBuf>> {
    let mut python: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--python") => {
                python = Some(PathBuf::from(it.next().context("--python needs a path")?));
            }
            Some("-h" | "--help") => {
                workspace::say(&format!("usage: cargo xtask {task} [--python PATH]"));
                return Ok(None);
            }
            _ => bail!("{task}: unexpected argument {}", arg.to_string_lossy()),
        }
    }
    let python = python.unwrap_or_else(|| {
        let venv = root.join(ORACLE);
        if cfg!(windows) {
            venv.join("Scripts").join("python.exe")
        } else {
            venv.join("bin").join("python")
        }
    });
    if !python.exists() {
        bail!(
            "{} not found: run `cargo xtask oracle install` (or pass --python)",
            python.display()
        );
    }
    Ok(Some(python))
}
