//! `cargo xtask identify-tables [--src DIR] [--version V] [--out FILE] [--check]`: regenerate snp-identify's tag
//! tables (`crates/snp-identify/src/generated.rs`) from Python identify.
//!
//! `--src` is the `identify/` package directory of an identify source tree, unpacked wheel or installed copy (it
//! must contain `extensions.py` and `interpreters.py`); the default is the one installed in the pre-commit 4.7.0
//! oracle (`target/oracle/pc470-py312`, identify 2.6.20, hash-locked by `xtask/oracle/requirements-pc470.txt`;
//! `cargo xtask oracle install`). The files are parsed as data by [`pyliteral`]; no Python code is imported or
//! executed. The version comes from `setup.cfg` or the `identify-*.dist-info` next to the package unless `--version`
//! names it. `--check` renders in memory and fails, with a per-table drift report, when the committed file differs.
//!
//! The parity vectors that pin the behaviour on top of these tables come from `cargo xtask identify-vectors`.
//!
//! Owner: WP-00 (integration lead; ported from file-identify's own xtask `gen-identify`, ADR-116).

mod codegen;
mod pyliteral;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow, bail};
use pyliteral::Value;

use crate::workspace;

/// The generated file (relative to the workspace root).
const OUT: &str = "crates/snp-identify/src/generated.rs";
/// The default oracle venv (relative to the workspace root).
const ORACLE: &str = "target/oracle/pc470-py312";
/// The usage line.
const USAGE: &str =
    "usage: cargo xtask identify-tables [--src DIR] [--version V] [--out FILE] [--check]";

/// A table as parsed from Python: key -> set of tags (sorted).
pub(crate) type PyTable = BTreeMap<String, BTreeSet<String>>;

/// The four identify tables plus the version they came from.
#[derive(Debug)]
pub(crate) struct Tables {
    pub(crate) version: String,
    pub(crate) extensions: PyTable,
    pub(crate) extensions_need_binary_check: PyTable,
    pub(crate) names: PyTable,
    pub(crate) interpreters: PyTable,
}

/// identify's `TYPE_TAGS | MODE_TAGS | ENCODING_TAGS`.
pub(crate) const BASE_TAGS: [&str; 8] = [
    "directory",
    "file",
    "symlink",
    "socket",
    "executable",
    "non-executable",
    "text",
    "binary",
];

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    let mut src: Option<PathBuf> = None;
    let mut version: Option<String> = None;
    let mut out = root.join(OUT);
    let mut check = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--src") => {
                src = Some(PathBuf::from(it.next().context("--src needs a directory")?));
            }
            Some("--version") => {
                let v = it.next().context("--version needs a value")?;
                version = Some(v.to_str().context("--version is not UTF-8")?.to_owned());
            }
            Some("--out") => out = PathBuf::from(it.next().context("--out needs a file")?),
            Some("--check") => check = true,
            Some("-h" | "--help") => {
                workspace::say(USAGE);
                return Ok(());
            }
            _ => bail!(
                "identify-tables: unexpected argument {}\n{USAGE}",
                arg.to_string_lossy()
            ),
        }
    }
    let src = match src {
        Some(src) => src,
        None => oracle_package(&root)?,
    };
    let version = match version {
        Some(v) => v,
        None => detect_version(&src)?,
    };
    let tables = load_tables(&src, version)?;
    validate(&tables)?;
    let code = codegen::render(&tables).map_err(|e| anyhow!("rendering: {e}"))?;
    let shown = workspace::rel(&root, &out);
    if check {
        let current = fs::read_to_string(&out).with_context(|| format!("reading {shown}"))?;
        if current == code {
            workspace::say(&format!(
                "identify-tables: {shown} is up to date with identify {}",
                tables.version
            ));
            return Ok(());
        }
        report_drift(&current, &tables);
        bail!(
            "{shown} is out of date with identify {}: run `cargo xtask identify-tables`",
            tables.version
        );
    }
    fs::write(&out, &code).with_context(|| format!("writing {shown}"))?;
    workspace::say(&format!(
        "identify-tables: wrote {shown} (identify {}, {} tags, {} extensions, {} binary-check extensions, {} names, \
         {} interpreters)",
        tables.version,
        all_tags(&tables).len(),
        tables.extensions.len(),
        tables.extensions_need_binary_check.len(),
        tables.names.len(),
        tables.interpreters.len()
    ));
    Ok(())
}

/// The `identify` package installed in the oracle venv.
fn oracle_package(root: &Path) -> anyhow::Result<PathBuf> {
    let venv = root.join(ORACLE);
    let mut candidates = vec![venv.join("Lib").join("site-packages").join("identify")];
    if let Ok(lib) = fs::read_dir(venv.join("lib")) {
        for entry in lib.flatten() {
            candidates.push(entry.path().join("site-packages").join("identify"));
        }
    }
    candidates
        .into_iter()
        .find(|p| p.join("extensions.py").is_file())
        .with_context(|| {
            format!(
                "no identify package in {}: run `cargo xtask oracle install` (or pass --src)",
                workspace::rel(root, &venv)
            )
        })
}

/// Find the identify version next to the package directory: `setup.cfg`
/// (source checkout / sdist) or `identify-*.dist-info/METADATA` (wheel or installed copy).
fn detect_version(pkg: &Path) -> anyhow::Result<String> {
    let parent = pkg.parent().unwrap_or(pkg);
    if let Ok(cfg) = fs::read_to_string(parent.join("setup.cfg")) {
        let mut in_metadata = false;
        for line in cfg.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_metadata = line == "[metadata]";
                continue;
            }
            let version = line
                .split_once('=')
                .filter(|(key, _)| in_metadata && key.trim() == "version");
            if let Some((_, v)) = version {
                return Ok(v.trim().to_owned());
            }
        }
    }
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("identify-") && name.ends_with(".dist-info") {
                let meta = fs::read_to_string(entry.path().join("METADATA")).unwrap_or_default();
                if let Some(v) = meta.lines().find_map(|l| l.strip_prefix("Version: ")) {
                    return Ok(v.trim().to_owned());
                }
            }
        }
    }
    bail!(
        "could not detect the identify version near {}; pass --version",
        pkg.display()
    )
}

/// Parses the four tables out of `extensions.py` and `interpreters.py`.
fn load_tables(pkg: &Path, version: String) -> anyhow::Result<Tables> {
    let read = |name: &str| {
        let path = pkg.join(name);
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    };
    let ext_env = pyliteral::eval_module(&read("extensions.py")?)
        .map_err(|e| anyhow!("extensions.py: {e}"))?;
    let int_env = pyliteral::eval_module(&read("interpreters.py")?)
        .map_err(|e| anyhow!("interpreters.py: {e}"))?;
    let table =
        |env: &BTreeMap<String, Value>, file: &str, name: &str| -> anyhow::Result<PyTable> {
            match env.get(name) {
                Some(Value::Dict(items)) => items
                    .iter()
                    .map(|(k, v)| match v {
                        Value::Set(s) => Ok((k.clone(), s.clone())),
                        _ => Err(anyhow!("{file}: {name}[{k:?}] is not a set")),
                    })
                    .collect(),
                Some(_) => bail!("{file}: {name} is not a dict"),
                None => bail!("{file}: {name} is not defined"),
            }
        };
    Ok(Tables {
        version,
        extensions: table(&ext_env, "extensions.py", "EXTENSIONS")?,
        extensions_need_binary_check: table(
            &ext_env,
            "extensions.py",
            "EXTENSIONS_NEED_BINARY_CHECK",
        )?,
        names: table(&ext_env, "extensions.py", "NAMES")?,
        interpreters: table(&int_env, "interpreters.py", "INTERPRETERS")?,
    })
}

/// Upstream invariants (identify's `tests/extensions_test.py`) plus the
/// assumptions the Rust implementation relies on.
fn validate(t: &Tables) -> anyhow::Result<()> {
    let encoding = |tags: &BTreeSet<String>| {
        tags.iter()
            .filter(|x| *x == "text" || *x == "binary")
            .count()
    };
    let mut problems = Vec::new();
    for (k, v) in &t.extensions {
        if encoding(v) != 1 {
            problems.push(format!(
                "EXTENSIONS[{k:?}] must have exactly one of text/binary: {v:?}"
            ));
        }
    }
    for (k, v) in &t.names {
        if encoding(v) != 1 {
            problems.push(format!(
                "NAMES[{k:?}] must have exactly one of text/binary: {v:?}"
            ));
        }
    }
    for (k, v) in &t.extensions_need_binary_check {
        if encoding(v) != 0 {
            problems.push(format!(
                "EXTENSIONS_NEED_BINARY_CHECK[{k:?}] must not have text/binary: {v:?}"
            ));
        }
        if t.extensions.contains_key(k) {
            problems.push(format!(
                "{k:?} is in both EXTENSIONS and EXTENSIONS_NEED_BINARY_CHECK"
            ));
        }
    }
    for (k, v) in &t.interpreters {
        if encoding(v) != 0 {
            problems.push(format!("INTERPRETERS[{k:?}] has an encoding tag: {v:?}"));
        }
    }
    for table in [
        &t.extensions,
        &t.extensions_need_binary_check,
        &t.names,
        &t.interpreters,
    ] {
        for (k, v) in table {
            if v.is_empty() {
                problems.push(format!("{k:?} maps to an empty tag set"));
            }
        }
    }
    if all_tags(t).len() > usize::from(u16::MAX) {
        problems.push("too many tags for u16 ids".to_owned());
    }
    if problems.is_empty() {
        Ok(())
    } else {
        bail!("{}", problems.join("\n"))
    }
}

/// `identify.ALL_TAGS`, sorted.
pub(crate) fn all_tags(t: &Tables) -> BTreeSet<String> {
    let mut all: BTreeSet<String> = BASE_TAGS.iter().map(|s| (*s).to_owned()).collect();
    for table in [
        &t.extensions,
        &t.extensions_need_binary_check,
        &t.names,
        &t.interpreters,
    ] {
        for tags in table.values() {
            all.extend(tags.iter().cloned());
        }
    }
    all
}

/// Reports what changed compared with the checked-in file (parsed back from its data lines).
fn report_drift(current: &str, t: &Tables) {
    let old = codegen::parse_back(current);
    let new: [(&str, &PyTable); 4] = [
        ("EXTENSIONS", &t.extensions),
        (
            "EXTENSIONS_NEED_BINARY_CHECK",
            &t.extensions_need_binary_check,
        ),
        ("NAMES", &t.names),
        ("INTERPRETERS", &t.interpreters),
    ];
    let empty = PyTable::new();
    for (name, table) in new {
        let before = old.get(name).unwrap_or(&empty);
        for (k, v) in table {
            match before.get(k) {
                None => workspace::say(&format!("  {name}: + {k:?} -> {v:?}")),
                Some(o) if o != v => workspace::say(&format!("  {name}: ~ {k:?} {o:?} -> {v:?}")),
                Some(_) => {}
            }
        }
        for k in before.keys().filter(|k| !table.contains_key(*k)) {
            workspace::say(&format!("  {name}: - {k:?}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed tables are what the generator renders from the oracle's identify (when installed): the
    /// regeneration path stays exercised and the generated file cannot drift from its generator.
    #[test]
    fn committed_tables_match_the_oracle_identify() {
        let root = workspace::root().unwrap();
        let Ok(src) = oracle_package(&root) else {
            return; // no oracle on this machine (CI installs it only in the oracle jobs)
        };
        let tables = load_tables(&src, detect_version(&src).unwrap()).unwrap();
        validate(&tables).unwrap();
        let code = codegen::render(&tables).unwrap();
        let committed = fs::read_to_string(root.join(OUT)).unwrap();
        assert!(
            code == committed,
            "{OUT} is stale: run `cargo xtask identify-tables`"
        );
    }
}
