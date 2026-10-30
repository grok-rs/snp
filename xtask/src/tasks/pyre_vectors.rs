//! `cargo xtask pyre-vectors`: regenerate snp-pyre's CPython-3.12 `re` truth tables (01 Addendum §A.8).
//!
//! ```text
//! cargo xtask pyre-vectors [--python PATH] [--refs DIR] [--seed N] [--only a,b,…]
//!                          [--fuzz-str N] [--fuzz-bytes N] [--out DIR]
//! ```
//!
//! Runs `xtask/oracle/gen_re_vectors.py` under CPython 3.12.3 (default `/usr/bin/python3.12`, isolated mode,
//! `LC_ALL=C.UTF-8` so that `(?L)` sees the C/UTF-8 locale) and writes `crates/snp-pyre/tests/vectors/*.json`
//! and `crates/snp-pyre/src/generated/names312.rs` (then formatted with rustfmt). `--refs` names the reference
//! repositories (default `~/.cache/snp-dev/refs`) whose manifests and paths seed the corpus; `NameAliases.txt` is
//! taken from the `unicode_names2` package that `cargo metadata` resolves. `--only` selects among `names`,
//! `classes`, `edge`, `corpus`, `nesting`, `fuzz_str`, `fuzz_bytes`; `--out` redirects the vectors (exploratory
//! runs with other seeds: build the tests with `SNP_PYRE_VECTORS_DIR=<dir>`). The output is deterministic.
//!
//! Owner: WP-13.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail, ensure};

use crate::workspace;

/// The generator, relative to the workspace root.
const SCRIPT: &str = "xtask/oracle/gen_re_vectors.py";
/// The committed vectors.
const VECTORS: &str = "crates/snp-pyre/tests/vectors";
/// The generated Rust tables.
const GENERATED: &str = "crates/snp-pyre/src/generated";
/// The oracle interpreter (WBS "Oracles and vectors": the host CPython 3.12.3).
const DEFAULT_PYTHON: &str = "/usr/bin/python3.12";
/// Options passed through to the script with their value.
const PASSTHROUGH: [&str; 5] = ["--refs", "--seed", "--only", "--fuzz-str", "--fuzz-bytes"];

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    let mut python = PathBuf::from(DEFAULT_PYTHON);
    let mut out = root.join(VECTORS);
    let mut passthrough: Vec<OsString> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let name = arg.to_str().unwrap_or_default();
        match name {
            "-h" | "--help" => {
                workspace::say(
                    "cargo xtask pyre-vectors [--python PATH] [--refs DIR] [--seed N] [--only a,b] \
                     [--fuzz-str N] [--fuzz-bytes N] [--out DIR]",
                );
                return Ok(());
            }
            "--python" => python = it.next().context("--python needs a value")?.into(),
            "--out" => out = it.next().context("--out needs a value")?.into(),
            _ if PASSTHROUGH.contains(&name) => {
                passthrough.push(arg.clone());
                passthrough.push(
                    it.next()
                        .with_context(|| format!("{name} needs a value"))?
                        .clone(),
                );
            }
            _ => bail!(
                "pyre-vectors: unknown argument `{}` (see --help)",
                arg.display()
            ),
        }
    }
    check_python(&python)?;
    let aliases = name_aliases(&root)?;
    let generated = root.join(GENERATED);
    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    std::fs::create_dir_all(&generated)
        .with_context(|| format!("creating {}", generated.display()))?;
    let status = Command::new(&python)
        .arg("-I")
        .arg(root.join(SCRIPT))
        .arg("--out")
        .arg(&out)
        .arg("--gen")
        .arg(&generated)
        .arg("--aliases")
        .arg(&aliases)
        .args(&passthrough)
        .env("LC_ALL", "C.UTF-8")
        .env("PYTHONHASHSEED", "0")
        .current_dir(&root)
        .status()
        .with_context(|| format!("running {}", python.display()))?;
    ensure!(status.success(), "{SCRIPT} failed ({status})");
    let names = generated.join("names312.rs");
    if names.is_file() {
        let status = Command::new("rustfmt")
            .arg("--edition")
            .arg("2024")
            .arg(&names)
            .current_dir(&root)
            .status()
            .context("running rustfmt")?;
        ensure!(
            status.success(),
            "rustfmt {} failed ({status})",
            names.display()
        );
    }
    let shown = if out.starts_with(&root) {
        workspace::rel(&root, &out)
    } else {
        out.display().to_string()
    };
    workspace::say(&format!("pyre-vectors: wrote {shown} and {GENERATED}"));
    Ok(())
}

/// The interpreter must be CPython 3.12.3 (the regex grammar profile, ADR-020).
fn check_python(python: &Path) -> anyhow::Result<()> {
    let out = Command::new(python)
        .args([
            "-I",
            "-c",
            "import sys, unicodedata; print(sys.implementation.name, '.'.join(map(str, sys.version_info[:3])), \
             unicodedata.unidata_version)",
        ])
        .output()
        .with_context(|| format!("running {}", python.display()))?;
    let got = String::from_utf8_lossy(&out.stdout);
    ensure!(
        out.status.success() && got.trim() == "cpython 3.12.3 15.0.0",
        "{} is `{}`, the vectors need CPython 3.12.3 with UCD 15.0.0 (pass --python)",
        python.display(),
        got.trim()
    );
    Ok(())
}

/// `NameAliases.txt` of the `unicode_names2` package the workspace resolves.
fn name_aliases(root: &Path) -> anyhow::Result<PathBuf> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(root)
        .output()
        .context("running `cargo metadata`")?;
    ensure!(
        out.status.success(),
        "`cargo metadata` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parsing `cargo metadata`")?;
    let packages = meta
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .context("`cargo metadata` has no packages")?;
    let manifest = packages
        .iter()
        .find(|p| p.get("name").is_some_and(|n| n == "unicode_names2"))
        .and_then(|p| p.get("manifest_path")?.as_str())
        .context(
            "unicode_names2 is not in the dependency graph (feature py-re-names of snp-pyre)",
        )?;
    let path = Path::new(manifest)
        .with_file_name("data")
        .join("NameAliases.txt");
    ensure!(path.is_file(), "{} is missing", path.display());
    Ok(path)
}
