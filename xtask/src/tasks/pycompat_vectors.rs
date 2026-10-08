//! `cargo xtask pycompat-vectors [--check] [--py312 PYTHON] [--py313 PYTHON]`: regenerate
//! snp-pycompat's CPython vectors and Unicode tables (`xtask/oracle/gen_pycompat_*.py`, 15 §A1,
//! ADR-053, WBS WP-11).
//!
//! * `crates/snp-pycompat/tests/vectors/pycompat-vectors.cpython312.json` — the generator under
//!   CPython 3.12.3;
//! * `crates/snp-pycompat/tests/vectors/pycompat-delta-cpython313.json` — the generator under
//!   CPython 3.13.14 with `--delta-against` the 3.12 file (only the entries that differ);
//! * `crates/snp-pycompat/src/generated/tables.rs` — `gen_pycompat_tables.py` under CPython
//!   3.13.14 (UCD 15.1, ADR-020).
//!
//! The interpreters default to the oracle venvs (`target/oracle/pc462-py31{2,3}`, built by
//! `cargo xtask oracle install`), else `python3.12` / `python3.13` on `PATH`; their exact versions
//! are verified because every output records them. The generators run isolated (`-I`) from an empty
//! scratch directory under `target/`. Every run also regenerates the original 59-section file with
//! `--legacy` and requires its recorded sha256 (provenance of the recovered generator, ADR-053).
//!
//! Without `--check` the outputs are written to the tree; with `--check` nothing is written and
//! every output must be byte-identical to the committed file (CI drift check). Either way the
//! sha256 of each output must equal the value recorded below: a generator change updates them.
//!
//! Owner: WP-11.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail, ensure};

use crate::workspace;

/// The vector generator (relative to the workspace root).
const GEN_VECTORS: &str = "xtask/oracle/gen_pycompat_vectors.py";
/// The Unicode-table generator.
const GEN_TABLES: &str = "xtask/oracle/gen_pycompat_tables.py";

/// The sha256 of the original 59-section vector file (15 §A1), reproduced with `--legacy`.
const LEGACY_SHA256: &str = "8c642b5fdf8c2d9b86786aa8bbcefc81cd97ed22eb77396e5095261cd7710004";

/// One generated file: where it is committed and the sha256 recorded for it.
#[derive(Debug, Clone, Copy)]
struct Output {
    /// The committed path (relative to the workspace root).
    path: &'static str,
    /// The scratch file name.
    scratch: &'static str,
    /// The recorded sha256.
    sha256: &'static str,
}

const VECTORS: Output = Output {
    path: "crates/snp-pycompat/tests/vectors/pycompat-vectors.cpython312.json",
    scratch: "pycompat-vectors.cpython312.json",
    sha256: "6a268c468325e59559549b411bb676cecbc76d7a044225f0086a7735cc862e04",
};

const DELTA: Output = Output {
    path: "crates/snp-pycompat/tests/vectors/pycompat-delta-cpython313.json",
    scratch: "pycompat-delta-cpython313.json",
    sha256: "f79d739df1614f8515f4ea8bda1d208175f0020f723ea44acbbc5150d3b6a462",
};

const TABLES: Output = Output {
    path: "crates/snp-pycompat/src/generated/tables.rs",
    scratch: "tables.rs",
    sha256: "7437aaf6d75237067857f39a3be8cdff8bb7e2198a67e697750aa53a0463863b",
};

/// One required interpreter.
#[derive(Debug, Clone, Copy)]
struct Interpreter {
    /// The command-line flag that overrides it.
    flag: &'static str,
    /// The oracle venv that provides it by default.
    venv: &'static str,
    /// The fallback command on `PATH`.
    fallback: &'static str,
    /// `platform.python_version()` it must report.
    version: &'static str,
    /// `unicodedata.unidata_version` it must report.
    unicode: &'static str,
}

const PY312: Interpreter = Interpreter {
    flag: "--py312",
    venv: "pc462-py312",
    fallback: "python3.12",
    version: "3.12.3",
    unicode: "15.0.0",
};

const PY313: Interpreter = Interpreter {
    flag: "--py313",
    venv: "pc462-py313",
    fallback: "python3.13",
    version: "3.13.14",
    unicode: "15.1.0",
};

/// The parsed command line.
#[derive(Debug, Default)]
struct Opts {
    check: bool,
    py312: Option<OsString>,
    py313: Option<OsString>,
}

fn parse_args(args: &[OsString]) -> anyhow::Result<Opts> {
    let mut opts = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--check" {
            opts.check = true;
        } else if a == PY312.flag {
            opts.py312 = Some(it.next().context("--py312 needs an interpreter")?.clone());
        } else if a == PY313.flag {
            opts.py313 = Some(it.next().context("--py313 needs an interpreter")?.clone());
        } else {
            bail!(
                "unknown argument {}; usage: cargo xtask pycompat-vectors [--check] [--py312 PYTHON] [--py313 PYTHON]",
                a.to_string_lossy()
            );
        }
    }
    Ok(opts)
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

/// The interpreter to use for `want`, verified to be exactly the expected CPython.
fn interpreter(
    root: &Path,
    want: Interpreter,
    given: Option<&OsString>,
) -> anyhow::Result<OsString> {
    let python = if let Some(p) = given {
        p.clone()
    } else {
        let venv = root.join("target/oracle").join(want.venv);
        let exe = if cfg!(windows) {
            venv.join("Scripts/python.exe")
        } else {
            venv.join("bin/python")
        };
        if exe.is_file() {
            exe.into_os_string()
        } else {
            OsString::from(want.fallback)
        }
    };
    let report = run_ok(
        Command::new(&python).args([
            "-I",
            "-c",
            "import platform, unicodedata; print(platform.python_implementation(), platform.python_version(), unicodedata.unidata_version)",
        ]),
        &format!("{} --version", python.to_string_lossy()),
    )?;
    let expected = format!("CPython {} {}", want.version, want.unicode);
    ensure!(
        report.trim() == expected,
        "{} reports `{}`, the vectors need `{expected}` (pass {} <python>)",
        python.to_string_lossy(),
        report.trim(),
        want.flag
    );
    Ok(python)
}

/// `python -I script args…` from the empty directory `cwd`.
fn run_script(
    python: &OsStr,
    script: &Path,
    args: &[&OsStr],
    cwd: &Path,
    what: &str,
) -> anyhow::Result<()> {
    run_ok(
        Command::new(python)
            .arg("-I")
            .arg(script)
            .args(args)
            .current_dir(cwd),
        what,
    )
    .map(drop)
}

/// The sha256 of `path` (through `hashlib`: xtask has no hashing dependency).
fn sha256(python: &OsStr, path: &Path) -> anyhow::Result<String> {
    let out = run_ok(
        Command::new(python)
            .args([
                "-I",
                "-c",
                "import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], 'rb').read()).hexdigest())",
            ])
            .arg(path),
        "hashlib.sha256",
    )?;
    Ok(out.trim().to_owned())
}

/// A fresh, empty directory.
fn fresh_dir(path: &Path) -> anyhow::Result<PathBuf> {
    if path.exists() {
        std::fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))?;
    }
    std::fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    Ok(path.to_path_buf())
}

/// Runs every generator into `work/out` and returns that directory.
fn generate(root: &Path, py312: &OsStr, py313: &OsStr, work: &Path) -> anyhow::Result<PathBuf> {
    let cwd = fresh_dir(&work.join("cwd"))?;
    let out = fresh_dir(&work.join("out"))?;
    let gen_vectors = root.join(GEN_VECTORS);
    let gen_tables = root.join(GEN_TABLES);
    let legacy = out.join("legacy.json");
    let vectors = out.join(VECTORS.scratch);
    let delta = out.join(DELTA.scratch);
    let tables = out.join(TABLES.scratch);

    let fixture = fresh_dir(&work.join("fixture-legacy"))?;
    run_script(
        py312,
        &gen_vectors,
        &[
            legacy.as_os_str(),
            fixture.as_os_str(),
            OsStr::new("--legacy"),
        ],
        &cwd,
        "gen_pycompat_vectors.py --legacy (3.12)",
    )?;
    let fixture = fresh_dir(&work.join("fixture-312"))?;
    run_script(
        py312,
        &gen_vectors,
        &[vectors.as_os_str(), fixture.as_os_str()],
        &cwd,
        "gen_pycompat_vectors.py (3.12)",
    )?;
    let fixture = fresh_dir(&work.join("fixture-313"))?;
    run_script(
        py313,
        &gen_vectors,
        &[
            delta.as_os_str(),
            fixture.as_os_str(),
            OsStr::new("--delta-against"),
            vectors.as_os_str(),
        ],
        &cwd,
        "gen_pycompat_vectors.py --delta-against (3.13)",
    )?;
    run_script(
        py313,
        &gen_tables,
        &[tables.as_os_str()],
        &cwd,
        "gen_pycompat_tables.py (3.13)",
    )?;
    let legacy_sha = sha256(py312, &legacy)?;
    ensure!(
        legacy_sha == LEGACY_SHA256,
        "the --legacy vectors hash to {legacy_sha}, not the original {LEGACY_SHA256}: the first 59 \
         sections of {GEN_VECTORS} must stay byte-identical to the recovered generator (ADR-053)"
    );
    Ok(out)
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let opts = parse_args(args)?;
    let root = workspace::root()?;
    let py312 = interpreter(&root, PY312, opts.py312.as_ref())?;
    let py313 = interpreter(&root, PY313, opts.py313.as_ref())?;
    let work = root.join("target/xtask/pycompat-vectors");
    let out = generate(&root, &py312, &py313, &work)?;
    workspace::say(&format!(
        "pycompat-vectors: --legacy reproduces the original vectors ({LEGACY_SHA256})"
    ));

    let mut problems: Vec<String> = Vec::new();
    for o in [VECTORS, DELTA, TABLES] {
        let fresh = out.join(o.scratch);
        let committed = root.join(o.path);
        let sha = sha256(&py312, &fresh)?;
        if sha != o.sha256 {
            problems.push(format!(
                "{}: sha256 {sha}, recorded {} (update xtask/src/tasks/pycompat_vectors.rs)",
                o.path, o.sha256
            ));
        }
        if opts.check {
            let new =
                std::fs::read(&fresh).with_context(|| format!("reading {}", fresh.display()))?;
            let old = std::fs::read(&committed).unwrap_or_default();
            if new != old {
                problems.push(format!(
                    "{}: differs from the regenerated file {} (run `cargo xtask pycompat-vectors`)",
                    o.path,
                    fresh.display()
                ));
            }
        } else {
            std::fs::copy(&fresh, &committed)
                .with_context(|| format!("writing {}", committed.display()))?;
            workspace::say(&format!("pycompat-vectors: wrote {} ({sha})", o.path));
        }
    }
    if problems.is_empty() {
        if opts.check {
            std::fs::remove_dir_all(&work).unwrap_or_default();
            workspace::say("pycompat-vectors: the committed vectors and tables are up to date");
        }
        Ok(())
    } else {
        bail!("{}", problems.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        let o = parse_args(&[]).unwrap();
        assert!(!o.check && o.py312.is_none() && o.py313.is_none());
        let o = parse_args(&[
            OsString::from("--check"),
            OsString::from("--py313"),
            OsString::from("/x/python"),
        ])
        .unwrap();
        assert!(o.check);
        assert_eq!(o.py313.as_deref(), Some(OsStr::new("/x/python")));
        assert!(parse_args(&[OsString::from("--py312")]).is_err());
        assert!(parse_args(&[OsString::from("--frobnicate")]).is_err());
    }

    #[test]
    fn recorded_hashes_are_sha256_hex() {
        for h in [LEGACY_SHA256, VECTORS.sha256, DELTA.sha256, TABLES.sha256] {
            assert_eq!(h.len(), 64);
            assert!(
                h.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            );
        }
    }

    #[test]
    fn the_committed_files_exist() {
        let root = workspace::root().unwrap();
        for o in [VECTORS, DELTA, TABLES] {
            assert!(root.join(o.path).is_file(), "{}", o.path);
        }
        for g in [GEN_VECTORS, GEN_TABLES] {
            assert!(root.join(g).is_file(), "{g}");
        }
    }
}
