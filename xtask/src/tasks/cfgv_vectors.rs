//! `cargo xtask cfgv-vectors [--python PATH]`: regenerate snp-cfgv's Python-parity vectors and its Unicode table.
//!
//! Runs, under the pre-commit 4.7.0 oracle interpreter (`target/oracle/pc470-py312`: CPython 3.12.3, Unicode 15.0.0,
//! with the hash-locked cfgv 3.5.0 of `xtask/oracle/requirements-pc470.txt`; `cargo xtask oracle install`), with
//! `python -I` from the workspace root:
//!
//! * `xtask/oracle/gen_cfgv_vectors.py <site-packages> crates/snp-cfgv/tests/vectors`: `repr()`/`str()` of values,
//!   UTF-8 decoding errors, every cfgv message, `apply_defaults`/`remove_defaults` and `load_from_filename` cases,
//!   computed by CPython and the real cfgv (`repr.json`, `utf8.json`, `messages.json`);
//! * `xtask/oracle/gen_cfgv_unicode_tables.py crates/snp-cfgv/src/py/tables.rs`: the `str.isprintable()` ranges.
//!
//! Both outputs are committed and deterministic; CI needs no Python (`tests/parity.rs` reads the vectors).
//! `--python` selects another CPython 3.12 that has cfgv 3.5.0 installed.
//!
//! Owner: WP-00 (integration lead; ported from cfgv-rs's `scripts/`, ADR-116).

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, bail};

use super::identify_vectors;
use crate::workspace;

/// The vector generator (relative to the workspace root).
const VECTORS_SCRIPT: &str = "xtask/oracle/gen_cfgv_vectors.py";
/// The Unicode table generator (relative to the workspace root).
const TABLES_SCRIPT: &str = "xtask/oracle/gen_cfgv_unicode_tables.py";
/// The committed vectors (relative to the workspace root).
const VECTORS: &str = "crates/snp-cfgv/tests/vectors";
/// The committed table (relative to the workspace root).
const TABLES: &str = "crates/snp-cfgv/src/py/tables.rs";

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    let Some(python) = identify_vectors::oracle_python(&root, args, "cfgv-vectors")? else {
        return Ok(()); // --help
    };
    let site = identify_vectors::site_packages(&python, &root)?;
    if !site.join("cfgv.py").is_file() {
        bail!(
            "{} has no cfgv.py: the interpreter lacks cfgv 3.5.0",
            site.display()
        );
    }
    run_script(
        &python,
        &root,
        VECTORS_SCRIPT,
        &[site.as_os_str(), root.join(VECTORS).as_os_str()],
    )?;
    run_script(
        &python,
        &root,
        TABLES_SCRIPT,
        &[root.join(TABLES).as_os_str()],
    )?;
    workspace::say(&format!(
        "cfgv-vectors: wrote {VECTORS}/*.json and {TABLES}"
    ));
    Ok(())
}

/// Runs `python -I SCRIPT ARGS…` from the workspace root (which holds no `.py` file).
fn run_script(python: &Path, root: &Path, script: &str, args: &[&OsStr]) -> anyhow::Result<()> {
    let status = Command::new(python)
        .arg("-I")
        .arg(root.join(script))
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("running {}", python.display()))?;
    if !status.success() {
        bail!("{script} failed ({status})");
    }
    Ok(())
}
