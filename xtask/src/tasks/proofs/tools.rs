//! The tool-backed commands of `cargo xtask proofs`: Verus (V1-V4), Lean (L1-L4), the oracle tests, mutants,
//! install and status (docs/VERIFICATION.md §3.3, §3.4, §6).
//!
//! Tools are found through `.github/scripts/proofs-tools.sh path` (the pinned install directory, which
//! `proofs install` fills), falling back to `verus`/`verusfmt` on `PATH`; the Verus version must equal the registry's
//! pin. Lean comes from elan (`lake` on `PATH`, toolchain from `proofs/lean/lean-toolchain`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context as _, bail};
use serde_json::Value;

use super::{
    LEAN_DIR, REGISTRY, ROOT, Registry, SPEC_DIR, Status, TOOLS_SCRIPT, Tree, fingerprint, guards,
};
use crate::workspace;

/// Axioms a Lean theorem may depend on (L2).
const AXIOMS: [&str; 3] = ["propext", "Classical.choice", "Quot.sound"];
/// Forbidden in Lean sources (L4); `partial` is allowed only in the `*Main.lean` I/O drivers.
const LEAN_FORBIDDEN: [&str; 8] = [
    "sorry",
    "admit",
    "native_decide",
    "implemented_by",
    "extern",
    "partial",
    "axiom",
    "unsafe",
];

/// `proofs verus` arguments.
#[derive(Debug, Default)]
pub(crate) struct VerusArgs {
    /// `--module M`: verify one mount (iteration aid; Verus refuses `--verify-module` with `--no-cheating`).
    module: Option<String>,
    /// `--json F`: keep Verus's JSON report.
    json: Option<PathBuf>,
}

impl VerusArgs {
    pub(crate) fn parse(args: &[String]) -> anyhow::Result<Self> {
        let mut out = Self::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--module" => {
                    out.module = Some(it.next().context("--module needs a mount name")?.clone());
                }
                "--json" => {
                    out.json = Some(PathBuf::from(it.next().context("--json needs a file")?));
                }
                other => bail!(
                    "proofs verus: unknown argument {other} (usage: proofs verus [--module M] [--json F])"
                ),
            }
        }
        Ok(out)
    }
}

/// `proofs verus`: V1-V4 (with `--module`: V1/V2 for one mount and V4; not a gate).
pub(crate) fn verus(root: &Path, args: &VerusArgs) -> anyhow::Result<()> {
    let tree = Tree::disk(root);
    let reg = Registry::load(&tree)?;
    let tools = Tools::find(root, &reg)?;
    let mut problems = Vec::new();

    let kernels: Vec<&super::Kernel> = reg
        .kernels
        .iter()
        .filter(|k| k.status == Status::Verified)
        .filter(|k| args.module.as_ref().is_none_or(|m| *m == k.mount))
        .collect();
    if let Some(m) = &args.module
        && kernels.is_empty()
    {
        bail!("no verified kernel is mounted as `{m}`");
    }

    let started = Instant::now();
    let run = match &args.module {
        Some(m) => {
            workspace::say(&format!(
                "proofs verus: --module {m} runs without --no-cheating (Verus refuses the combination): an iteration aid, not a gate"
            ));
            tools.run_verus(&root.join(ROOT), &["--verify-module", m])?
        }
        None => tools.run_verus(&root.join(ROOT), &["--no-cheating"])?,
    };
    let secs = started.elapsed().as_secs_f64();
    if let Some(path) = &args.json {
        std::fs::write(path, &run.stdout).with_context(|| format!("writing {}", path.display()))?;
    }
    let report = run.report();

    // V1
    let (verified, errors) = report.as_ref().map_or((0, u64::MAX), counts);
    // A module run's report has no `success` key; `encountered-error` is always there.
    let flag = |k: &str| {
        report
            .as_ref()
            .and_then(|r| r.pointer(&format!("/verification-results/{k}")))
            .and_then(Value::as_bool)
    };
    let success = run.ok
        && errors == 0
        && flag("encountered-error") == Some(false)
        && flag("encountered-vir-error") != Some(true)
        && flag("success") != Some(false);
    if !success {
        workspace::say(run.stderr.trim_end());
        problems.push(format!(
            "V1: verification failed (exit ok: {}, verified {verified}, errors {})",
            run.ok,
            if errors == u64::MAX {
                "?".to_owned()
            } else {
                errors.to_string()
            }
        ));
    }

    // V2
    if let Some(report) = &report {
        let names: BTreeSet<&str> = report
            .get("func-details")
            .and_then(Value::as_object)
            .map_or_default(|o| o.keys().map(String::as_str).collect());
        for k in &kernels {
            let src = tree
                .read(&k.file)
                .with_context(|| format!("reading {}", k.file))?;
            for missing in manifest_gaps(&k.mount, &guards::kernel_fns(&src)?, &names) {
                problems.push(format!(
                    "V2: {} `{missing}` is missing from Verus's report (unverified function: is the mount marked #[verus_verify]?)",
                    k.file
                ));
            }
        }
        if args.module.is_none() {
            let floor: u64 = kernels.iter().map(|k| k.verified_min).sum();
            if verified < floor {
                problems.push(format!(
                    "V2: {verified} item(s) verified, below the registry floor {floor} (sum of verified_min)"
                ));
            }
        }
        timings(report);
    }
    workspace::say(&format!(
        "proofs verus: {verified} verified, {} error(s) in {secs:.1} s",
        if errors == u64::MAX {
            "?".to_owned()
        } else {
            errors.to_string()
        }
    ));

    // V3
    if args.module.is_none() {
        problems.extend(run_mutants(root, &tree, &reg, &tools)?);
    }

    // V4
    problems.extend(tools.verusfmt_check(root, &tree)?);

    finish("proofs verus", &problems)
}

/// `proofs mutants`: V3 alone.
pub(crate) fn mutants(root: &Path) -> anyhow::Result<()> {
    let tree = Tree::disk(root);
    let reg = Registry::load(&tree)?;
    let tools = Tools::find(root, &reg)?;
    let problems = run_mutants(root, &tree, &reg, &tools)?;
    finish("proofs mutants", &problems)
}

fn finish(what: &str, problems: &[String]) -> anyhow::Result<()> {
    if problems.is_empty() {
        workspace::say(&format!("{what}: ok"));
        return Ok(());
    }
    for p in problems {
        workspace::say(&format!("{what}: {p}"));
    }
    bail!("{} problem(s)", problems.len())
}

/// (verified, errors) of a Verus JSON report.
fn counts(r: &Value) -> (u64, u64) {
    let get = |k: &str| {
        r.pointer(&format!("/verification-results/{k}"))
            .and_then(Value::as_u64)
    };
    (
        get("verified").unwrap_or(0),
        get("errors").unwrap_or(u64::MAX),
    )
}

/// Functions of a kernel file (`kernel_fns`) that Verus's `func-details` does not list under `kernels::<mount>::`.
pub(crate) fn manifest_gaps(
    mount: &str,
    fns: &[(String, bool)],
    names: &BTreeSet<&str>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (path, is_method) in fns {
        let key = format!("kernels::{mount}::{path}");
        if names.contains(key.as_str()) {
            continue;
        }
        // An impl whose self type is not a path is named `impl&%N` by Verus.
        let alt = is_method.then(|| {
            let (prefix, name) = path.rsplit_once("::").unwrap_or(("", path));
            let module = prefix.rsplit_once("::").map_or("", |(m, _)| m);
            let base = if module.is_empty() {
                format!("kernels::{mount}::")
            } else {
                format!("kernels::{mount}::{module}::")
            };
            names.iter().any(|n| {
                n.strip_prefix(base.as_str())
                    .and_then(|rest| rest.strip_suffix(&format!("::{name}")))
                    .is_some_and(|mid| mid.starts_with("impl&%") && !mid.contains("::"))
            })
        });
        if alt != Some(true) {
            out.push(path.clone());
        }
    }
    out
}

/// The slowest verified functions of the kernels and specs (from `--time`).
fn timings(report: &Value) {
    let mut rows: Vec<(u64, String)> = Vec::new();
    let modules = report
        .pointer("/times-ms/smt/smt-run-module-times")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for m in &modules {
        for f in m
            .get("function-breakdown")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(name), Some(ms)) = (
                f.get("function").and_then(Value::as_str),
                f.get("time").and_then(Value::as_u64),
            ) && name.starts_with("kernels::")
            {
                rows.push((ms, name.to_owned()));
            }
        }
    }
    rows.sort_by(|a, b| b.cmp(a));
    for (ms, name) in rows.iter().take(5) {
        workspace::say(&format!("  {ms:>6} ms  {name}"));
    }
}

/// The located tools.
#[derive(Debug)]
struct Tools {
    verus: PathBuf,
    verusfmt: PathBuf,
}

/// One Verus run.
#[derive(Debug)]
struct VerusRun {
    ok: bool,
    stdout: Vec<u8>,
    stderr: String,
}

impl VerusRun {
    fn report(&self) -> Option<Value> {
        serde_json::from_slice(&self.stdout).ok()
    }
}

impl Tools {
    /// The pinned Verus and verusfmt: the install directory of `proofs-tools.sh`, else `PATH`.
    fn find(root: &Path, reg: &Registry) -> anyhow::Result<Self> {
        let dir = tools_dir(root)?;
        let pick = |name: &str| {
            let p = dir.join(name);
            if p.is_file() { p } else { PathBuf::from(name) }
        };
        let tools = Self {
            verus: pick("verus"),
            verusfmt: pick("verusfmt"),
        };
        let out = Command::new(&tools.verus)
            .args(["--version", "--output-json"])
            .stdin(Stdio::null())
            .output()
            .with_context(|| {
                format!(
                    "running {} (install the pinned Verus with `cargo xtask proofs install`)",
                    tools.verus.display()
                )
            })?;
        let v: Value = serde_json::from_slice(&out.stdout).with_context(|| {
            format!(
                "parsing `{} --version --output-json`",
                tools.verus.display()
            )
        })?;
        let got = v
            .pointer("/verus/version")
            .and_then(Value::as_str)
            .unwrap_or("?");
        if got != reg.verus.version {
            bail!(
                "{} is Verus {got}, but {REGISTRY} pins {} (run `cargo xtask proofs install`)",
                tools.verus.display(),
                reg.verus.version
            );
        }
        Ok(tools)
    }

    fn run_verus(&self, root_file: &Path, extra: &[&str]) -> anyhow::Result<VerusRun> {
        let out = Command::new(&self.verus)
            .arg(root_file)
            .args(["--crate-type=lib", "--edition=2024"])
            .args(extra)
            .args(["--output-json", "--time"])
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("running {}", self.verus.display()))?;
        Ok(VerusRun {
            ok: out.status.success(),
            stdout: out.stdout,
            stderr: String::from_utf8_lossy_owned(out.stderr),
        })
    }

    /// V4: `verusfmt --check` on the root and every spec file.
    fn verusfmt_check(&self, root: &Path, tree: &Tree) -> anyhow::Result<Vec<String>> {
        let mut files = vec![ROOT.to_owned()];
        files.extend(tree.rust_files(SPEC_DIR)?);
        let out = Command::new(&self.verusfmt)
            .arg("--check")
            .args(&files)
            .current_dir(root)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("running {}", self.verusfmt.display()))?;
        if out.status.success() {
            return Ok(Vec::new());
        }
        Ok(vec![format!(
            "V4: verusfmt --check fails (run `verusfmt {}`):\n{}{}",
            files.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )])
    }
}

/// The pinned tools directory (`proofs-tools.sh path`).
fn tools_dir(root: &Path) -> anyhow::Result<PathBuf> {
    let out = Command::new("bash")
        .arg(root.join(TOOLS_SCRIPT))
        .arg("path")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {TOOLS_SCRIPT} path"))?;
    if !out.status.success() {
        bail!(
            "{TOOLS_SCRIPT} path failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// V3: every registered mutant, applied to a scratch copy of the root, the specs and the mounted files, must make
/// verification fail. A `find` text that no longer occurs is an error (write a fresh mutant).
fn run_mutants(
    root: &Path,
    tree: &Tree,
    reg: &Registry,
    tools: &Tools,
) -> anyhow::Result<Vec<String>> {
    let mut problems = Vec::new();
    let verified: Vec<&super::Kernel> = reg
        .kernels
        .iter()
        .filter(|k| k.status == Status::Verified)
        .collect();
    let scratch = root.join("target/proofs/mutant");
    let mut n = 0_usize;
    for k in &verified {
        for m in &k.mutants {
            n = n.saturating_add(1);
            let src = tree
                .read(&k.file)
                .with_context(|| format!("reading {}", k.file))?;
            let Some(mutated) = apply_mutant(&src, &m.find, &m.replace) else {
                problems.push(format!(
                    "V3: stale mutant of {}: {:?} does not occur in {} (write a fresh mutant)",
                    k.id, m.find, k.file
                ));
                continue;
            };
            if scratch.exists() {
                std::fs::remove_dir_all(&scratch)
                    .with_context(|| format!("removing {}", scratch.display()))?;
            }
            copy_tree(&root.join("proofs/verus"), &scratch.join("proofs/verus"))?;
            for other in &verified {
                let dst = scratch.join(&other.file);
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(root.join(&other.file), &dst)
                    .with_context(|| format!("copying {}", other.file))?;
            }
            std::fs::write(scratch.join(&k.file), mutated)?;
            let run = tools.run_verus(&scratch.join(ROOT), &["--no-cheating"])?;
            let (_, errors) = run.report().as_ref().map_or((0, 0), counts);
            let label = format!("{} mutant {:?} -> {:?}", k.id, m.find, m.replace);
            if !run.ok && errors > 0 && errors != u64::MAX {
                workspace::say(&format!(
                    "proofs mutants: killed ({errors} error(s)): {label}"
                ));
            } else if run.ok {
                problems.push(format!(
                    "V3: MUTANT SURVIVED (verification still succeeds): {label}"
                ));
            } else {
                problems.push(format!(
                    "V3: mutant fails before verification (it must verify-fail, not break the build): {label}\n{}",
                    run.stderr.trim_end()
                ));
            }
        }
    }
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch)
            .with_context(|| format!("removing {}", scratch.display()))?;
    }
    workspace::say(&format!("proofs mutants: {n} registered mutant(s) run"));
    Ok(problems)
}

/// `src` with the first occurrence of `find` replaced, or `None` when it does not occur.
pub(crate) fn apply_mutant(src: &str, find: &str, replace: &str) -> Option<String> {
    if find.is_empty() || !src.contains(find) {
        return None;
    }
    Some(src.replacen(find, replace, 1))
}

fn copy_tree(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "out" {
            continue; // proofs/verus/out/: local reports (gitignored)
        }
        let from = entry.path();
        let to = dst.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).with_context(|| format!("copying {}", from.display()))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Lean

/// `proofs lean`: L1-L4.
pub(crate) fn lean(root: &Path) -> anyhow::Result<()> {
    let tree = Tree::disk(root);
    let reg = Registry::load(&tree)?;
    let dir = root.join(LEAN_DIR);
    let mut problems = Vec::new();

    // L4 first: it needs no tool.
    problems.extend(lean_sources(&dir)?);

    // L1
    let build = lake(&dir, &["build", "--wfail"])?;
    if !build.0 {
        problems.push(format!("L1: `lake build --wfail` fails:\n{}", build.1));
    }

    // L2
    if build.0 {
        let (ok, text) = lake(&dir, &["env", "lean", "Axioms.lean"])?;
        if ok {
            let axioms_src =
                std::fs::read_to_string(dir.join("Axioms.lean")).context("reading Axioms.lean")?;
            let wanted: Vec<String> = reg.models.iter().flat_map(|m| m.theorems.clone()).collect();
            problems.extend(axiom_problems(&text, &axioms_src, &wanted));
        } else {
            problems.push(format!("L2: `lake env lean Axioms.lean` fails:\n{text}"));
        }
    }

    // L3
    if build.0 {
        match leanchecker(&dir)? {
            None => workspace::say(
                "proofs lean: L3 skipped: leanchecker is not part of this Lean toolchain",
            ),
            Some((true, _)) => {}
            Some((false, text)) => problems.push(format!(
                "L3: `lake env leanchecker SnpModels` fails:\n{text}"
            )),
        }
    }
    finish("proofs lean", &problems)
}

/// Runs `lake <args>` in `dir`: (success, stdout + stderr).
fn lake(dir: &Path, args: &[&str]) -> anyhow::Result<(bool, String)> {
    let out = Command::new("lake")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .context("running lake (install elan; proofs/lean/lean-toolchain pins the Lean release)")?;
    let mut text = String::from_utf8_lossy_owned(out.stdout);
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

/// L3: `None` when the toolchain has no `leanchecker`.
fn leanchecker(dir: &Path) -> anyhow::Result<Option<(bool, String)>> {
    let (ok, prefix) = lake(dir, &["env", "lean", "--print-prefix"])?;
    if !ok {
        bail!("`lake env lean --print-prefix` failed: {prefix}");
    }
    let bin = Path::new(prefix.trim()).join("bin");
    if !bin.join("leanchecker").is_file() && !bin.join("leanchecker.exe").is_file() {
        return Ok(None);
    }
    lake(dir, &["env", "leanchecker", "SnpModels"]).map(Some)
}

/// L2 over the output of `lean Axioms.lean`: every printed theorem depends only on the three standard axioms, every
/// `#print axioms` line produced an answer, and every registered theorem is printed.
pub(crate) fn axiom_problems(output: &str, axioms_src: &str, registered: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut printed = BTreeSet::new();
    let joined = output.replace('\n', " ");
    for chunk in joined.split(" '").map(|c| c.trim_start_matches('\'')) {
        let Some((name, rest)) = chunk.split_once('\'') else {
            continue;
        };
        let rest = rest.trim_start();
        if rest.starts_with("does not depend on any axioms") {
            printed.insert(name.to_owned());
        } else if let Some(list) = rest.strip_prefix("depends on axioms:") {
            printed.insert(name.to_owned());
            let list = list.trim_start();
            let inner = list
                .strip_prefix('[')
                .and_then(|l| l.split_once(']'))
                .map_or("", |(i, _)| i);
            for ax in inner.split(',').map(str::trim).filter(|a| !a.is_empty()) {
                if !AXIOMS.contains(&ax) {
                    problems.push(format!(
                        "L2: {name} depends on axiom {ax} (allowed: {})",
                        AXIOMS.join(", ")
                    ));
                }
            }
        }
    }
    let requested: Vec<&str> = axioms_src
        .lines()
        .filter_map(|l| l.trim().strip_prefix("#print axioms"))
        .map(str::trim)
        .collect();
    if requested.is_empty() {
        problems.push("L2: Axioms.lean prints no theorem".to_owned());
    }
    for t in &requested {
        if !printed.contains(*t) {
            problems.push(format!("L2: no axiom report for {t}"));
        }
    }
    for t in registered {
        if !requested.contains(&t.as_str()) {
            problems.push(format!(
                "L2: registered theorem {t} is missing from Axioms.lean (add `#print axioms {t}`)"
            ));
        }
    }
    problems
}

/// L4 over the Lake package: forbidden tokens in every `.lean` file (outside `.lake/`), and no dependency.
fn lean_sources(dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut problems = Vec::new();
    let mut files = Vec::new();
    lean_files(dir, &mut files)?;
    for f in &files {
        let text =
            std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        let name = f
            .file_name()
            .map_or_default(|n| n.to_string_lossy().into_owned());
        let driver = name.ends_with("Main.lean");
        for word in forbidden_lean_words(&text) {
            if word == "partial" && driver {
                continue;
            }
            problems.push(format!(
                "L4: {} uses `{word}` (models carry no sorry/admit/native_decide/implemented_by/extern/axiom/unsafe; partial only in *Main.lean)",
                workspace::rel(dir, f)
            ));
        }
    }
    let lakefile = std::fs::read_to_string(dir.join("lakefile.toml")).unwrap_or_default();
    if lakefile
        .lines()
        .any(|l| l.trim_start().starts_with("[[require]]"))
    {
        problems.push("L4: lakefile.toml has a `require`: the package uses Lean's core library only (ADR-095)".to_owned());
    }
    if let Ok(m) = std::fs::read_to_string(dir.join("lake-manifest.json"))
        && let Ok(v) = serde_json::from_str::<Value>(&m)
        && v.get("packages")
            .and_then(Value::as_array)
            .is_some_and(|p| !p.is_empty())
    {
        problems.push(
            "L4: lake-manifest.json lists packages: the package uses Lean's core library only"
                .to_owned(),
        );
    }
    Ok(problems)
}

fn lean_files(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if !entry.file_name().as_encoded_bytes().starts_with(b".") {
                lean_files(&path, out)?;
            }
        } else if path.extension().is_some_and(|e| e == "lean") {
            out.push(path);
        }
    }
    out.sort();
    Ok(())
}

/// The forbidden words of L4 that occur as whole identifiers outside comments and string literals.
pub(crate) fn forbidden_lean_words(src: &str) -> BTreeSet<&'static str> {
    let code = strip_lean_comments(src);
    let mut found = BTreeSet::new();
    let is_ident = |c: char| c.is_alphanumeric() || matches!(c, '_' | '\'' | '.' | '!' | '?');
    for word in code.split(|c: char| !is_ident(c)) {
        let word = word.trim_end_matches(['!', '?']);
        if let Some(w) = LEAN_FORBIDDEN.iter().find(|f| **f == word) {
            found.insert(*w);
        }
    }
    found
}

/// `src` without `--` line comments, nested `/- … -/` block comments (doc comments included) and string literals.
fn strip_lean_comments(src: &str) -> String {
    let mut out = String::new();
    let mut chars = src.chars().peekable();
    let mut depth = 0_usize;
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            if c == '\\' {
                chars.next();
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if depth > 0 {
            if c == '/' && chars.peek() == Some(&'-') {
                chars.next();
                depth = depth.saturating_add(1);
            } else if c == '-' && chars.peek() == Some(&'/') {
                chars.next();
                depth = depth.saturating_sub(1);
            }
            continue;
        }
        match (c, chars.peek()) {
            ('/', Some('-')) => {
                chars.next();
                depth = 1;
                out.push(' ');
            }
            ('-', Some('-')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('"', _) => {
                in_string = true;
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Oracles, install, status

/// `proofs oracles [-k ID]`: builds the Lean executables, then runs each model's home-crate oracle test.
pub(crate) fn oracles(root: &Path, only: Option<&str>) -> anyhow::Result<()> {
    let tree = Tree::disk(root);
    let reg = Registry::load(&tree)?;
    let models: Vec<&super::Model> = reg
        .models
        .iter()
        .filter(|m| only.is_none_or(|k| k == m.id))
        .collect();
    if let Some(k) = only
        && models.is_empty()
    {
        bail!("no [[model]] with id {k}");
    }
    if models.is_empty() {
        workspace::say("proofs oracles: no [[model]] is registered: nothing to run");
        return Ok(());
    }
    let dir = root.join(LEAN_DIR);
    let (ok, text) = lake(&dir, &["build", "--wfail"])?;
    if !ok {
        bail!("`lake build --wfail` fails:\n{text}");
    }
    let bin = dir.join(".lake/build/bin");
    let nextest = Command::new("cargo")
        .args(["nextest", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    for m in models {
        if !bin.join(&m.oracle).is_file() && !bin.join(format!("{}.exe", m.oracle)).is_file() {
            bail!(
                "model {}: oracle {} was not built in {}",
                m.id,
                m.oracle,
                bin.display()
            );
        }
        let (package, test) = test_target(&tree, &m.oracle_test)?;
        let mut cmd = Command::new("cargo");
        if nextest {
            cmd.args(["nextest", "run"]);
        } else {
            cmd.arg("test");
        }
        cmd.args(["--locked", "--release", "-p", &package, "--test", &test])
            .env("SNP_LEAN_ORACLES", &bin)
            .env("SNP_REQUIRE_LEAN_ORACLES", "1")
            .current_dir(root);
        workspace::say(&format!(
            "proofs oracles: {} ({package} --test {test})",
            m.id
        ));
        let status = cmd.status().context("running cargo")?;
        if !status.success() {
            bail!("model {}: the oracle test {} fails", m.id, m.oracle_test);
        }
    }
    workspace::say("proofs oracles: ok");
    Ok(())
}

/// `crates/<dir>/tests/<stem>.rs` → (package name, test target).
fn test_target(tree: &Tree, test: &str) -> anyhow::Result<(String, String)> {
    let mut parts = test.split('/');
    let (Some("crates"), Some(dir), Some("tests"), Some(file), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        bail!("oracle_test {test} must be crates/<crate>/tests/<name>.rs");
    };
    let stem = file
        .strip_suffix(".rs")
        .with_context(|| format!("{test} is not a .rs file"))?;
    let manifest = tree
        .read(&format!("crates/{dir}/Cargo.toml"))
        .with_context(|| format!("crates/{dir}/Cargo.toml is missing"))?;
    let t: toml::Table = manifest.parse()?;
    let name = t
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .with_context(|| format!("crates/{dir}/Cargo.toml has no package name"))?;
    Ok((name.to_owned(), stem.to_owned()))
}

/// `proofs install`: `proofs-tools.sh install`.
pub(crate) fn install(root: &Path) -> anyhow::Result<()> {
    let status = Command::new("bash")
        .arg(root.join(TOOLS_SCRIPT))
        .arg("install")
        .current_dir(root)
        .status()
        .with_context(|| format!("running {TOOLS_SCRIPT}"))?;
    if !status.success() {
        bail!("{TOOLS_SCRIPT} install failed ({status})");
    }
    Ok(())
}

/// `proofs status`: pins, installed tools, kernels, models and anchor freshness.
pub(crate) fn status(root: &Path) -> anyhow::Result<()> {
    let tree = Tree::disk(root);
    let reg = Registry::load(&tree)?;
    workspace::say(&format!(
        "Verus {} (rustc {}); Lean {}",
        reg.verus.version,
        reg.verus.rustc,
        tree.read(&format!("{LEAN_DIR}/lean-toolchain"))
            .unwrap_or_default()
            .trim()
    ));
    let installed = tools_dir(root).ok().map(|d| (d.join("verus").is_file(), d));
    match installed {
        Some((true, d)) => workspace::say(&format!("tools: installed in {}", d.display())),
        Some((false, d)) => workspace::say(&format!(
            "tools: not installed in {} (`cargo xtask proofs install`; `verus` on PATH is used if it is the pinned one)",
            d.display()
        )),
        None => workspace::say("tools: unknown (proofs-tools.sh did not run)"),
    }
    workspace::say("");
    workspace::say(&format!(
        "{:<24} {:<9} {:<28} {:>5} {:>8}",
        "kernel", "status", "mount", "min", "mutants"
    ));
    for k in &reg.kernels {
        let st = match k.status {
            Status::Verified => "verified",
            Status::Pending => "pending",
        };
        workspace::say(&format!(
            "{:<24} {st:<9} {:<28} {:>5} {:>8}",
            k.id,
            k.mount,
            k.verified_min,
            k.mutants.len()
        ));
    }
    if reg.models.is_empty() {
        workspace::say("\nno [[model]] registered");
    }
    for m in &reg.models {
        workspace::say(&format!(
            "\nmodel {} ({}, oracle {}):",
            m.id, m.lean, m.oracle
        ));
        for a in &m.anchors {
            let state = match fingerprint::of_anchor(&tree, &a.item) {
                Ok(fp) if fp == a.fingerprint => "fresh".to_owned(),
                Ok(fp) => format!("STALE (now {fp})"),
                Err(e) => format!("ERROR ({e:#})"),
            };
            workspace::say(&format!("  {} {}: {state}", a.item, a.fingerprint));
        }
    }
    let problems = guards::check(&tree, &reg)?;
    workspace::say(&format!(
        "\nguards: {} violation(s) (`cargo xtask proofs check`)",
        problems.len()
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutants_replace_the_first_occurrence_only() {
        assert_eq!(
            apply_mutant("a <= b; a <= b", "<=", "<").unwrap(),
            "a < b; a <= b"
        );
        assert_eq!(apply_mutant("x", "y", "z"), None);
        assert_eq!(apply_mutant("x", "", "z"), None);
    }

    #[test]
    fn the_manifest_finds_missing_functions() {
        let names: BTreeSet<&str> = [
            "kernels::m::f",
            "kernels::m::Scalar::find",
            "kernels::m::impl&%3::len",
            "kernels::m::inner::g",
        ]
        .into_iter()
        .collect();
        let fns = vec![
            ("f".to_owned(), false),
            ("Scalar::find".to_owned(), true),
            ("_::len".to_owned(), true),
            ("inner::g".to_owned(), false),
            ("h".to_owned(), false),
            ("Scalar::other".to_owned(), true),
        ];
        assert_eq!(manifest_gaps("m", &fns, &names), ["h", "Scalar::other"]);
    }

    #[test]
    fn the_manifest_of_k1_is_its_one_function() {
        let root = workspace::root().unwrap();
        let src =
            std::fs::read_to_string(root.join("crates/snp-proc/src/xargs/partition.rs")).unwrap();
        assert_eq!(
            guards::kernel_fns(&src).unwrap(),
            [("partition_lens".to_owned(), false)]
        );
    }

    #[test]
    fn axiom_reports_are_audited() {
        let out = "'A.x' depends on axioms: [propext, Classical.choice, Quot.sound]\n'A.y' does not depend on any axioms\n'A.z' depends on axioms: [propext,\n sorryAx]\n";
        let src = "import A\n#print axioms A.x\n#print axioms A.y\n#print axioms A.z\n#print axioms A.w\n";
        let p = axiom_problems(out, src, &["A.x".to_owned(), "A.v".to_owned()]);
        assert_eq!(p.len(), 3, "{p:#?}");
        assert!(p.iter().any(|m| m.contains("sorryAx")));
        assert!(p.iter().any(|m| m.contains("no axiom report for A.w")));
        assert!(p.iter().any(|m| m.contains("A.v is missing")));
        assert_eq!(
            axiom_problems(out.lines().next().unwrap(), "#print axioms A.x", &[]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn forbidden_lean_words_ignore_comments_strings_and_longer_names() {
        let src = "-- sorry in a comment\n/- admit /- nested sorry -/ still comment -/\n\
                   theorem t : True := trivial\ndef s := \"sorry\"\n#print axioms t\ndef axiomsOk := 1\n";
        assert!(forbidden_lean_words(src).is_empty());
        let bad = "theorem t : 1 = 1 := by sorry\npartial def f : Nat → Nat := f\naxiom a : False\n\
                   @[extern \"c\"] opaque g : Nat\ntheorem u : True := by native_decide\n";
        let found: Vec<&str> = forbidden_lean_words(bad).into_iter().collect();
        assert_eq!(
            found,
            ["axiom", "extern", "native_decide", "partial", "sorry"]
        );
    }

    #[test]
    fn the_committed_lean_package_has_no_forbidden_token() {
        let root = workspace::root().unwrap();
        let p = lean_sources(&root.join(LEAN_DIR)).unwrap();
        assert!(p.is_empty(), "{p:#?}");
    }

    #[test]
    fn verus_arguments_parse() {
        let a = VerusArgs::parse(&[
            "--module".into(),
            "m".into(),
            "--json".into(),
            "o.json".into(),
        ])
        .unwrap();
        assert_eq!(a.module.as_deref(), Some("m"));
        assert_eq!(a.json, Some(PathBuf::from("o.json")));
        assert!(VerusArgs::parse(&["--module".into()]).is_err());
        assert!(VerusArgs::parse(&["-x".into()]).is_err());
    }
}
