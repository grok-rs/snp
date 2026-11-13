//! Helpers shared by the WP-00 tasks: the workspace root, `cargo metadata`, Rust-source walking and stdout.
//!
//! Owner: WP-00.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail};

/// The workspace root (the parent of this crate's manifest directory).
pub(crate) fn root() -> anyhow::Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .context("xtask has no parent directory")
}

/// `cargo metadata --format-version 1 --no-deps` of the workspace at `root`.
pub(crate) fn metadata(root: &Path) -> anyhow::Result<serde_json::Value> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .context("running `cargo metadata`")?;
    if !out.status.success() {
        bail!(
            "`cargo metadata` failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    serde_json::from_slice(&out.stdout).context("parsing `cargo metadata` output")
}

/// Every `*.rs` file below `dir` (sorted; `target/` and hidden directories skipped). A missing `dir` is empty.
pub(crate) fn rust_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if dir.is_dir() {
        walk(dir, &mut out)?;
    }
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            if name != OsStr::new("target") && !name.as_encoded_bytes().starts_with(b".") {
                walk(&path, out)?;
            }
        } else if path.extension() == Some(OsStr::new("rs")) {
            out.push(path);
        }
    }
    Ok(())
}

/// `path` relative to `root`, with `/` separators (stable report and TSV output on every OS).
pub(crate) fn rel(root: &Path, path: &Path) -> String {
    let p = path.trim_prefix(root);
    p.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Writes one line to stdout (xtask's report channel; the workspace denies `print!`).
pub(crate) fn say(line: &str) {
    writeln!(io::stdout(), "{line}").unwrap_or_default();
}
