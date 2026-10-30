//! `cargo xtask yaml-vectors [--python PATH] [--out DIR]`: regenerate snp-yaml's PyYAML vectors and error corpus.
//!
//! Runs `xtask/oracle/gen_yaml_vectors.py` under the pre-commit 4.6.2 oracle interpreter
//! (`target/oracle/pc462-py312`, CPython 3.12.3 with PyYAML 6.0.3 and libyaml 0.2.5; `cargo xtask oracle install`)
//! with `python -I`, writing `crates/snp-yaml/tests/vectors/*.json`. The vectors are committed; CI needs no
//! Python. `--python` selects another interpreter (it must have PyYAML with libyaml), `--out` another directory.
//!
//! Owner: WP-14.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context as _, bail};

use crate::workspace;

/// The generator script (relative to the workspace root).
const SCRIPT: &str = "xtask/oracle/gen_yaml_vectors.py";
/// The default output directory (relative to the workspace root).
const OUT: &str = "crates/snp-yaml/tests/vectors";
/// The default oracle venv (relative to the workspace root).
const ORACLE: &str = "target/oracle/pc462-py312";

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    let mut python: Option<PathBuf> = None;
    let mut out = root.join(OUT);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--python") => {
                python = Some(PathBuf::from(it.next().context("--python needs a path")?));
            }
            Some("--out") => out = PathBuf::from(it.next().context("--out needs a directory")?),
            Some("-h" | "--help") => {
                workspace::say("usage: cargo xtask yaml-vectors [--python PATH] [--out DIR]");
                return Ok(());
            }
            _ => bail!(
                "yaml-vectors: unexpected argument {}",
                arg.to_string_lossy()
            ),
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
    let status = Command::new(&python)
        .arg("-I")
        .arg(root.join(SCRIPT))
        .arg(&out)
        .current_dir(&root)
        .status()
        .with_context(|| format!("running {}", python.display()))?;
    if !status.success() {
        bail!("{SCRIPT} failed ({status})");
    }
    workspace::say(&format!(
        "yaml-vectors: wrote {}",
        workspace::rel(&root, &out)
    ));
    Ok(())
}
