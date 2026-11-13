//! `cargo xtask proofs <command>`: the machine-checked kernels of ADR-095 (docs/VERIFICATION.md §3, §6).
//!
//! Commands:
//! * `check`: the tool-free guards G1-G10 (syn only, about a second; part of the required `xtask gates` job);
//! * `verus [--module M] [--json F]`: V1-V4: the root verifies with `--no-cheating`, every function of a kernel file is
//!   in Verus's JSON report and the verified count reaches the registry's floor, every registered mutant fails, and
//!   `verusfmt --check` passes on the root and the specs (with `--module`: one mount, no mutants, not a gate);
//! * `lean`: L1-L4: `lake build --wfail`, the axiom audit, `leanchecker`, forbidden tokens;
//! * `oracles [-k ID]`: builds the Lean oracle executables and runs the home-crate oracle tests of every model (or of
//!   model `ID`) with `SNP_LEAN_ORACLES` and `SNP_REQUIRE_LEAN_ORACLES=1`;
//! * `mutants`: V3 alone;
//! * `install`: the pinned Verus release, its rustc and verusfmt (`.github/scripts/proofs-tools.sh install`);
//! * `status`: the kernel and model tables, the installed tools and stale anchors;
//! * `fingerprint <file.rs::item>`: the anchor fingerprint of one item (for a `[[model]]` anchor).
//!
//! `proofs/registry.toml` is the single machine-readable source (§3.1); this file parses it. The guards live in
//! `proofs/guards.rs`, the anchor fingerprint in `proofs/fingerprint.rs`, the tool runs in `proofs/tools.rs`.
//!
//! Owner: WP-00.

mod fingerprint;
mod guards;
mod tools;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};

use crate::workspace;

/// The registry, relative to the workspace root.
pub(crate) const REGISTRY: &str = "proofs/registry.toml";
/// The Verus root.
pub(crate) const ROOT: &str = "proofs/verus/kernels.rs";
/// The spec tree.
pub(crate) const SPEC_DIR: &str = "proofs/verus/spec";
/// The only directory exempt from the cheat-token guard (G5) and the `unsafe_code` rule; it does not exist in M1.
pub(crate) const TRUSTED_DIR: &str = "proofs/verus/trusted";
/// The Lake package.
pub(crate) const LEAN_DIR: &str = "proofs/lean";
/// The tool installer.
pub(crate) const TOOLS_SCRIPT: &str = ".github/scripts/proofs-tools.sh";

const USAGE: &str = "usage: cargo xtask proofs check | verus [--module M] [--json F] | lean | oracles [-k ID] | \
                     mutants | install | status | fingerprint <file.rs::item>";

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    let args: Vec<String> = args
        .iter()
        .map(|a| {
            a.to_str()
                .map(str::to_owned)
                .with_context(|| format!("non-UTF-8 argument {}", a.to_string_lossy()))
        })
        .collect::<anyhow::Result<_>>()?;
    let Some((cmd, rest)) = args.split_first() else {
        bail!("{USAGE}");
    };
    match (cmd.as_str(), rest) {
        ("check", []) => check(&root),
        ("verus", rest) => tools::verus(&root, &tools::VerusArgs::parse(rest)?),
        ("lean", []) => tools::lean(&root),
        ("oracles", rest) => tools::oracles(&root, parse_k(rest)?),
        ("mutants", []) => tools::mutants(&root),
        ("install", []) => tools::install(&root),
        ("status", []) => tools::status(&root),
        ("fingerprint", [item]) => {
            let fp = fingerprint::of_anchor(&Tree::disk(&root), item)?;
            workspace::say(&fp);
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

/// `[-k ID]`.
fn parse_k(rest: &[String]) -> anyhow::Result<Option<&str>> {
    match rest {
        [] => Ok(None),
        [k, id] if k == "-k" => Ok(Some(id.as_str())),
        _ => bail!("{USAGE}"),
    }
}

/// `proofs check`: G1-G10 over the working tree.
fn check(root: &Path) -> anyhow::Result<()> {
    let tree = Tree::disk(root);
    let reg = Registry::load(&tree)?;
    let problems = guards::check(&tree, &reg)?;
    if problems.is_empty() {
        let verified = reg
            .kernels
            .iter()
            .filter(|k| k.status == Status::Verified)
            .count();
        workspace::say(&format!(
            "proofs check: ok (G1-G10; {verified} verified kernel(s), {} pending, {} model(s), {} trusted item(s))",
            reg.kernels.len().saturating_sub(verified),
            reg.models.len(),
            reg.trusted.len()
        ));
        return Ok(());
    }
    for p in &problems {
        workspace::say(&format!("proofs check: {p}"));
    }
    bail!(
        "{} guard violation(s) (docs/VERIFICATION.md §3.2)",
        problems.len()
    )
}

/// The files the guards read: the working tree, optionally overlaid (unit tests edit fixture trees in memory).
#[derive(Debug)]
pub(crate) struct Tree {
    root: PathBuf,
    /// Path (relative, `/`-separated) → replacement text, or `None` for a deleted file.
    overlay: BTreeMap<String, Option<String>>,
}

impl Tree {
    /// The tree on disk at `root`.
    pub(crate) fn disk(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            overlay: BTreeMap::new(),
        }
    }

    /// The text of `rel`, if the file exists.
    pub(crate) fn read(&self, rel: &str) -> Option<String> {
        if let Some(o) = self.overlay.get(rel) {
            return o.clone();
        }
        std::fs::read_to_string(self.root.join(rel)).ok()
    }

    /// Whether `rel` is an existing file.
    pub(crate) fn is_file(&self, rel: &str) -> bool {
        match self.overlay.get(rel) {
            Some(o) => o.is_some(),
            None => self.root.join(rel).is_file(),
        }
    }

    /// Every `*.rs` file below `dir` (relative paths, sorted), overlay included.
    pub(crate) fn rust_files(&self, dir: &str) -> anyhow::Result<Vec<String>> {
        let mut out: Vec<String> = workspace::rust_files(&self.root.join(dir))?
            .iter()
            .map(|p| workspace::rel(&self.root, p))
            .collect();
        let prefix = format!("{}/", dir.trim_end_matches('/'));
        for (path, text) in &self.overlay {
            if !path.starts_with(&prefix) || Path::new(path).extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let present = out.iter().any(|p| p == path);
            match (text.is_some(), present) {
                (true, false) => out.push(path.clone()),
                (false, true) => out.retain(|p| p != path),
                _ => {}
            }
        }
        out.sort();
        Ok(out)
    }

    /// Replaces (or creates) `rel` with `text` in memory.
    #[cfg(test)]
    pub(crate) fn put(&mut self, rel: &str, text: &str) {
        self.overlay.insert(rel.to_owned(), Some(text.to_owned()));
    }

    /// Deletes `rel` in memory.
    #[cfg(test)]
    pub(crate) fn remove(&mut self, rel: &str) {
        self.overlay.insert(rel.to_owned(), None);
    }
}

/// `status` of a `[[kernel]]` entry (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    /// Mounted and verified.
    Verified,
    /// Proof pending: unmounted, marker line says so.
    Pending,
}

/// One registered negative check (V3).
#[derive(Debug, Clone)]
pub(crate) struct Mutant {
    /// Text whose first occurrence in the kernel file is replaced.
    pub(crate) find: String,
    /// The replacement.
    pub(crate) replace: String,
}

/// One `[[kernel]]` entry.
#[derive(Debug, Clone)]
pub(crate) struct Kernel {
    pub(crate) id: String,
    pub(crate) status: Status,
    /// The production file (repo-relative, under `crates/`).
    pub(crate) file: String,
    /// The module name of its mount in the root.
    pub(crate) mount: String,
    /// The spec file.
    pub(crate) spec: String,
    /// `file.rs::fn` of every production caller.
    pub(crate) callers: Vec<String>,
    /// The floor this kernel adds to the whole-root verified count.
    pub(crate) verified_min: u64,
    pub(crate) mutants: Vec<Mutant>,
}

/// One fingerprinted link from a Lean model to production items.
#[derive(Debug, Clone)]
pub(crate) struct Anchor {
    /// `file.rs::item` (`item` may be `Type::method` or `mod::item`).
    pub(crate) item: String,
    /// `fnv1a64` of the item's normalised tokens, 16 hex digits.
    pub(crate) fingerprint: String,
}

/// One `[[model]]` entry.
#[derive(Debug, Clone)]
pub(crate) struct Model {
    pub(crate) id: String,
    /// The model file.
    pub(crate) lean: String,
    /// Fully qualified theorem names (axiom audit).
    pub(crate) theorems: Vec<String>,
    /// The oracle executable (a `lean_exe` of the Lake package).
    pub(crate) oracle: String,
    /// The home-crate oracle test file.
    pub(crate) oracle_test: String,
    pub(crate) anchors: Vec<Anchor>,
}

/// One `[[trusted]]` entry (§10 row 8; empty in M1).
#[derive(Debug, Clone)]
pub(crate) struct Trusted {
    /// `Type: Trait`.
    pub(crate) item: String,
    /// The kernel id that declares the trait.
    pub(crate) kernel: String,
    /// `file.rs::test_fn` of the differential test against the verified reference implementation.
    pub(crate) test: String,
}

/// The `[verus]` pins.
#[derive(Debug, Clone)]
pub(crate) struct VerusPin {
    pub(crate) version: String,
    pub(crate) rustc: String,
}

/// `proofs/registry.toml`.
#[derive(Debug, Clone)]
pub(crate) struct Registry {
    pub(crate) verus: VerusPin,
    pub(crate) kernels: Vec<Kernel>,
    pub(crate) models: Vec<Model>,
    pub(crate) trusted: Vec<Trusted>,
}

impl Registry {
    /// Reads and validates the shape of the registry (unknown keys are errors: a typo must not drop a check).
    pub(crate) fn load(tree: &Tree) -> anyhow::Result<Self> {
        let text = tree
            .read(REGISTRY)
            .with_context(|| format!("{REGISTRY} is missing"))?;
        Self::parse(&text).with_context(|| format!("parsing {REGISTRY}"))
    }

    pub(crate) fn parse(text: &str) -> anyhow::Result<Self> {
        let t: toml::Table = text.parse()?;
        only_keys(
            &t,
            &["verus", "kernel", "model", "trusted"],
            "the top level",
        )?;
        let v = t
            .get("verus")
            .and_then(toml::Value::as_table)
            .context("missing [verus]")?;
        only_keys(v, &["version", "rustc", "sha256", "verusfmt"], "[verus]")?;
        let verus = VerusPin {
            version: req_str(v, "version", "[verus]")?,
            rustc: req_str(v, "rustc", "[verus]")?,
        };
        let sha = v
            .get("sha256")
            .and_then(toml::Value::as_table)
            .context("[verus] sha256 must be a table of platform = hash")?;
        for (plat, h) in sha {
            check_sha(h, &format!("[verus] sha256.{plat}"))?;
        }
        let fmt = v
            .get("verusfmt")
            .and_then(toml::Value::as_table)
            .context("[verus] verusfmt must be { version, sha256 }")?;
        only_keys(fmt, &["version", "sha256"], "[verus] verusfmt")?;
        req_str(fmt, "version", "[verus] verusfmt")?;
        for (plat, h) in fmt
            .get("sha256")
            .and_then(toml::Value::as_table)
            .context("[verus] verusfmt.sha256 must be a table")?
        {
            check_sha(h, &format!("[verus] verusfmt.sha256.{plat}"))?;
        }

        let mut kernels = Vec::new();
        for (i, k) in tables(&t, "kernel")?.into_iter().enumerate() {
            let ctx = format!("[[kernel]] #{}", i.saturating_add(1));
            only_keys(
                k,
                &[
                    "id",
                    "status",
                    "file",
                    "mount",
                    "spec",
                    "callers",
                    "verified_min",
                    "mutants",
                ],
                &ctx,
            )?;
            let status = match req_str(k, "status", &ctx)?.as_str() {
                "verified" => Status::Verified,
                "pending" => Status::Pending,
                other => bail!("{ctx}: status must be \"verified\" or \"pending\", not {other:?}"),
            };
            let verified_min = k
                .get("verified_min")
                .and_then(toml::Value::as_integer)
                .and_then(|n| u64::try_from(n).ok())
                .with_context(|| format!("{ctx}: verified_min must be a non-negative integer"))?;
            let mut mutants = Vec::new();
            for m in k
                .get("mutants")
                .and_then(toml::Value::as_array)
                .with_context(|| format!("{ctx}: mutants must be an array"))?
            {
                let m = m
                    .as_table()
                    .with_context(|| format!("{ctx}: a mutant must be {{ find, replace }}"))?;
                only_keys(m, &["find", "replace"], &format!("{ctx} mutant"))?;
                mutants.push(Mutant {
                    find: req_str(m, "find", &ctx)?,
                    replace: req_str(m, "replace", &ctx)?,
                });
            }
            kernels.push(Kernel {
                id: req_str(k, "id", &ctx)?,
                status,
                file: req_str(k, "file", &ctx)?,
                mount: req_str(k, "mount", &ctx)?,
                spec: req_str(k, "spec", &ctx)?,
                callers: str_list(k, "callers", &ctx)?,
                verified_min,
                mutants,
            });
        }

        let mut models = Vec::new();
        for (i, m) in tables(&t, "model")?.into_iter().enumerate() {
            let ctx = format!("[[model]] #{}", i.saturating_add(1));
            only_keys(
                m,
                &["id", "lean", "theorems", "oracle", "oracle_test", "anchors"],
                &ctx,
            )?;
            let mut anchors = Vec::new();
            for a in m
                .get("anchors")
                .and_then(toml::Value::as_array)
                .with_context(|| format!("{ctx}: anchors must be an array"))?
            {
                let a = a
                    .as_table()
                    .with_context(|| format!("{ctx}: an anchor must be {{ item, fingerprint }}"))?;
                only_keys(a, &["item", "fingerprint"], &format!("{ctx} anchor"))?;
                anchors.push(Anchor {
                    item: req_str(a, "item", &ctx)?,
                    fingerprint: req_str(a, "fingerprint", &ctx)?,
                });
            }
            models.push(Model {
                id: req_str(m, "id", &ctx)?,
                lean: req_str(m, "lean", &ctx)?,
                theorems: str_list(m, "theorems", &ctx)?,
                oracle: req_str(m, "oracle", &ctx)?,
                oracle_test: req_str(m, "oracle_test", &ctx)?,
                anchors,
            });
        }

        let mut trusted = Vec::new();
        for (i, e) in tables(&t, "trusted")?.into_iter().enumerate() {
            let ctx = format!("[[trusted]] #{}", i.saturating_add(1));
            only_keys(e, &["item", "kernel", "test"], &ctx)?;
            trusted.push(Trusted {
                item: req_str(e, "item", &ctx)?,
                kernel: req_str(e, "kernel", &ctx)?,
                test: req_str(e, "test", &ctx)?,
            });
        }
        Ok(Self {
            verus,
            kernels,
            models,
            trusted,
        })
    }

    /// The kernel whose file is `rel`.
    pub(crate) fn kernel_of_file(&self, rel: &str) -> Option<&Kernel> {
        self.kernels.iter().find(|k| k.file == rel)
    }
}

fn only_keys(t: &toml::Table, allowed: &[&str], ctx: &str) -> anyhow::Result<()> {
    for k in t.keys() {
        if !allowed.contains(&k.as_str()) {
            bail!("{ctx}: unknown key {k:?} (allowed: {})", allowed.join(", "));
        }
    }
    Ok(())
}

fn req_str(t: &toml::Table, key: &str, ctx: &str) -> anyhow::Result<String> {
    t.get(key)
        .and_then(toml::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("{ctx}: `{key}` must be a non-empty string"))
}

fn str_list(t: &toml::Table, key: &str, ctx: &str) -> anyhow::Result<Vec<String>> {
    t.get(key)
        .and_then(toml::Value::as_array)
        .with_context(|| format!("{ctx}: `{key}` must be an array of strings"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .with_context(|| format!("{ctx}: `{key}` must be an array of strings"))
        })
        .collect()
}

fn tables<'a>(t: &'a toml::Table, key: &str) -> anyhow::Result<Vec<&'a toml::Table>> {
    match t.get(key) {
        None => Ok(Vec::new()),
        Some(v) => v
            .as_array()
            .with_context(|| format!("`{key}` must be an array of tables ([[{key}]])"))?
            .iter()
            .map(|e| {
                e.as_table()
                    .with_context(|| format!("`{key}` must be an array of tables ([[{key}]])"))
            })
            .collect(),
    }
}

fn check_sha(v: &toml::Value, ctx: &str) -> anyhow::Result<()> {
    let ok = v
        .as_str()
        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()));
    if !ok {
        bail!("{ctx} must be 64 hex digits");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_registry_parses_and_passes_the_guards() {
        let root = workspace::root().unwrap();
        let tree = Tree::disk(&root);
        let reg = Registry::load(&tree).unwrap();
        assert!(reg.kernels.iter().any(|k| k.id == "K1-xargs-partition"));
        let problems = guards::check(&tree, &reg).unwrap();
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn unknown_registry_keys_are_errors() {
        let base = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/tasks/proofs/fixtures/good/proofs/registry.toml"),
        )
        .unwrap();
        Registry::parse(&base).unwrap();
        let typo = base.replace("verified_min", "verified_minimum");
        let err = format!("{:#}", Registry::parse(&typo).unwrap_err());
        assert!(err.contains("verified_minimum"), "{err}");
        let bad_status = base.replace("status = \"verified\"", "status = \"done\"");
        assert!(Registry::parse(&bad_status).is_err());
        let bad_sha = base.replace("f3a0119292df", "zz");
        assert!(Registry::parse(&bad_sha).is_err());
    }
}
