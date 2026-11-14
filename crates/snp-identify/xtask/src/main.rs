//! Development tasks for `file-identify`.
//!
//! ```text
//! cargo xtask gen-identify --src <dir> [--version <v>] [--out <file>] [--check]
//! ```
//!
//! `<dir>` is the `identify/` package directory of a Python identify source
//! tree or unpacked wheel (it must contain `extensions.py` and
//! `interpreters.py`). The files are parsed as data by [`pyliteral`]; no
//! Python code is imported or executed.

mod codegen;
mod pyliteral;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pyliteral::Value;

/// A table as parsed from Python: key -> set of tags (sorted).
pub type PyTable = BTreeMap<String, BTreeSet<String>>;

/// The four identify tables plus the version they came from.
pub struct Tables {
    pub version: String,
    pub extensions: PyTable,
    pub extensions_need_binary_check: PyTable,
    pub names: PyTable,
    pub interpreters: PyTable,
}

/// identify's `TYPE_TAGS | MODE_TAGS | ENCODING_TAGS`.
pub const BASE_TAGS: [&str; 8] = [
    "directory",
    "file",
    "symlink",
    "socket",
    "executable",
    "non-executable",
    "text",
    "binary",
];

const USAGE: &str = "usage: cargo xtask gen-identify --src <identify-package-dir> [--version <v>] [--out <file>] [--check]";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<ExitCode, String> {
    let mut it = args.into_iter();
    match it.next().as_deref() {
        Some("gen-identify") => {}
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        Some(other) => return Err(format!("unknown task {other:?}\n{USAGE}")),
    }
    let mut src: Option<PathBuf> = None;
    let mut version: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut check = false;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--src" => src = it.next().map(PathBuf::from),
            "--version" => version = it.next(),
            "--out" => out = it.next().map(PathBuf::from),
            "--check" => check = true,
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    let src = src.ok_or_else(|| format!("--src is required\n{USAGE}"))?;
    let out = out.unwrap_or_else(|| workspace_root().join("src").join("generated.rs"));
    let version = match version {
        Some(v) => v,
        None => detect_version(&src)?,
    };

    let tables = load_tables(&src, version)?;
    validate(&tables)?;
    let code = codegen::render(&tables)?;

    if check {
        let current =
            fs::read_to_string(&out).map_err(|e| format!("reading {}: {e}", out.display()))?;
        if current == code {
            println!(
                "{} is up to date with identify {}",
                out.display(),
                tables.version
            );
            return Ok(ExitCode::SUCCESS);
        }
        eprintln!(
            "{} is out of date with identify {} — run `cargo xtask gen-identify --src {}`",
            out.display(),
            tables.version,
            src.display()
        );
        report_drift(&current, &tables);
        return Ok(ExitCode::FAILURE);
    }
    fs::write(&out, code).map_err(|e| format!("writing {}: {e}", out.display()))?;
    println!(
        "wrote {} (identify {}, {} tags, {} extensions, {} binary-check extensions, {} names, {} interpreters)",
        out.display(),
        tables.version,
        all_tags(&tables).len(),
        tables.extensions.len(),
        tables.extensions_need_binary_check.len(),
        tables.names.len(),
        tables.interpreters.len()
    );
    Ok(ExitCode::SUCCESS)
}

fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map_or(manifest.clone(), Path::to_path_buf)
}

/// Find the identify version next to the package directory: `setup.cfg`
/// (source checkout / sdist) or `identify-*.dist-info/METADATA` (wheel).
fn detect_version(pkg: &Path) -> Result<String, String> {
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
    Err(format!(
        "could not detect the identify version near {}; pass --version",
        pkg.display()
    ))
}

fn load_tables(pkg: &Path, version: String) -> Result<Tables, String> {
    let read = |name: &str| {
        let path = pkg.join(name);
        fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))
    };
    let ext_env = pyliteral::eval_module(&read("extensions.py")?)
        .map_err(|e| format!("extensions.py: {e}"))?;
    let int_env = pyliteral::eval_module(&read("interpreters.py")?)
        .map_err(|e| format!("interpreters.py: {e}"))?;
    let table =
        |env: &BTreeMap<String, Value>, file: &str, name: &str| -> Result<PyTable, String> {
            match env.get(name) {
                Some(Value::Dict(items)) => items
                    .iter()
                    .map(|(k, v)| match v {
                        Value::Set(s) => Ok((k.clone(), s.clone())),
                        _ => Err(format!("{file}: {name}[{k:?}] is not a set")),
                    })
                    .collect(),
                Some(_) => Err(format!("{file}: {name} is not a dict")),
                None => Err(format!("{file}: {name} is not defined")),
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
fn validate(t: &Tables) -> Result<(), String> {
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
        Err(problems.join("\n"))
    }
}

/// `identify.ALL_TAGS`, sorted.
#[must_use]
pub fn all_tags(t: &Tables) -> BTreeSet<String> {
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

/// Print a human-readable summary of what changed compared with the
/// checked-in file (parsed back from its data lines).
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
    for (name, table) in new {
        let empty = PyTable::new();
        let before = old.get(name).unwrap_or(&empty);
        for (k, v) in table {
            match before.get(k) {
                None => eprintln!("  {name}: + {k:?} -> {v:?}"),
                Some(o) if o != v => eprintln!("  {name}: ~ {k:?} {o:?} -> {v:?}"),
                Some(_) => {}
            }
        }
        for k in before.keys().filter(|k| !table.contains_key(*k)) {
            eprintln!("  {name}: - {k:?}");
        }
    }
}
