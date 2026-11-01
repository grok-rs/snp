//! The divergence-register tooling (COMPATIBILITY §5, ADR-071, WBS "Divergence files").
//!
//! Joins three sources: the register tables of `docs/COMPATIBILITY.md` §5 (one row per `D-…` ID with its class and
//! owning work packages), the per-ID scenario files `tests/differential/divergences/<ID>.yaml` and the per-ID
//! rationale files `docs/divergences/<ID>.md`. [`check`] reports, per owner, which IDs are still stubs (scenario
//! `status: stub`, or a rationale still reading "To be written by"), which are pinned by unit tests instead
//! (`status: unit`, whose listed tests must exist), and hard problems: a register ID without its files, a file
//! without a register ID, a scenario whose `expect_divergence` names another ID, or a file that does not parse.
//!
//! Owner: WP-35.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::scenario::{Scenario, Status};

/// One row of the register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `D-CRASH-04`.
    pub id: String,
    /// The section heading (`5.1 CRASH — …`).
    pub section: String,
    /// The class keyword of the section (`CRASH`, `DATA`, …, `DIAG`, `TIERB`).
    pub class: String,
    /// snp's behaviour (the second-to-last cell).
    pub summary: String,
    /// Work packages, first = file owner (`WP-21`).
    pub owners: Vec<String>,
}

/// Parses the §5 tables of `COMPATIBILITY.md`.
pub fn parse_register(md: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut in_register = false;
    for line in md.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            in_register = h.starts_with("5.");
            continue;
        }
        if let Some(h) = line.strip_prefix("### ") {
            if h.starts_with("5.") {
                in_register = true;
                h.clone_into(&mut section);
            } else {
                in_register = false;
            }
            continue;
        }
        if !in_register || !line.starts_with("| D-") {
            continue;
        }
        let cells: Vec<&str> = line.trim().trim_matches('|').split(" | ").map(str::trim).collect();
        let (Some(id), Some(wp)) = (cells.first(), cells.last()) else { continue };
        let summary = cells.len().checked_sub(2).and_then(|i| cells.get(i)).copied().unwrap_or_default();
        let class = section.split_whitespace().nth(1).unwrap_or_default().to_owned();
        let owners = wp
            .split(',')
            .map(str::trim)
            .filter(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()))
            .map(|w| format!("WP-{w}"))
            .collect();
        out.push(Entry {
            id: (*id).to_owned(),
            section: section.clone(),
            class,
            summary: summary.to_owned(),
            owners,
        });
    }
    out
}

/// The state of one ID's files.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileState {
    /// A runnable scenario.
    Active,
    /// `status: stub`.
    Stub,
    /// `status: unit` with these tests.
    Unit(Vec<String>),
    /// Missing.
    Missing,
}

/// One ID's row of the report.
#[derive(Debug, Clone)]
pub struct IdStatus {
    /// The register row.
    pub entry: Entry,
    /// The scenario file.
    pub scenario: FileState,
    /// The rationale file is filled (not a W0 stub).
    pub doc_filled: bool,
}

/// The result of [`check`].
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Per ID, in register order.
    pub ids: Vec<IdStatus>,
    /// Hard problems (always a failure).
    pub problems: Vec<String>,
}

impl Report {
    /// IDs whose scenario or rationale is still a stub, grouped by owner.
    pub fn stubs_by_owner(&self) -> BTreeMap<String, Vec<String>> {
        let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for s in &self.ids {
            if s.scenario == FileState::Stub || !s.doc_filled {
                let owner = s.entry.owners.first().cloned().unwrap_or_else(|| String::from("?"));
                m.entry(owner).or_default().push(s.entry.id.clone());
            }
        }
        m
    }

    /// No hard problem; with `strict`, also no stub.
    pub fn is_clean(&self, strict: bool) -> bool {
        self.problems.is_empty() && (!strict || self.stubs_by_owner().is_empty())
    }

    /// A text report: counts, stubs per owner, unit-pinned IDs, problems.
    pub fn render(&self) -> String {
        let mut o = String::new();
        let count = |f: &dyn Fn(&IdStatus) -> bool| self.ids.iter().filter(|s| f(s)).count();
        let _ = writeln!(
            o,
            "divergence register: {} IDs; scenarios: {} active, {} unit-pinned, {} stub, {} missing; rationales: {} filled",
            self.ids.len(),
            count(&|s| s.scenario == FileState::Active),
            count(&|s| matches!(s.scenario, FileState::Unit(_))),
            count(&|s| s.scenario == FileState::Stub),
            count(&|s| s.scenario == FileState::Missing),
            count(&|s| s.doc_filled),
        );
        let stubs = self.stubs_by_owner();
        if !stubs.is_empty() {
            let _ = writeln!(o, "unfilled (scenario or rationale still a stub), by owner:");
            for (owner, ids) in &stubs {
                let _ = writeln!(o, "  {owner}: {}", ids.join(", "));
            }
        }
        for s in &self.ids {
            if let FileState::Unit(tests) = &s.scenario {
                let _ = writeln!(o, "  {} pinned by unit tests: {}", s.entry.id, tests.join(", "));
            }
        }
        for p in &self.problems {
            let _ = writeln!(o, "PROBLEM: {p}");
        }
        o
    }
}

/// Whether a rationale file is still the W0 stub.
fn doc_is_stub(text: &str) -> bool {
    text.contains("To be written by") || text.contains("**Status:** stub")
}

/// Whether `path::name` (or `path::mod::name`) names an existing test function.
fn unit_test_exists(root: &Path, spec: &str) -> bool {
    let Some((file, rest)) = spec.split_once("::") else { return false };
    let name = rest.rsplit("::").next().unwrap_or(rest);
    std::fs::read_to_string(root.join(file)).is_ok_and(|t| t.contains(&format!("fn {name}(")))
}

/// Checks the register of the checkout at `root` (the workspace root).
pub fn check(root: &Path) -> Result<Report, String> {
    let compat = root.join("docs/COMPATIBILITY.md");
    let md = std::fs::read_to_string(&compat).map_err(|e| format!("{}: {e}", compat.display()))?;
    let entries = parse_register(&md);
    if entries.is_empty() {
        return Err(format!("{}: no register rows found in §5", compat.display()));
    }
    let scen_dir = root.join("tests/differential/divergences");
    let doc_dir = root.join("docs/divergences");
    let mut report = Report::default();
    let mut seen: Vec<&str> = Vec::new();
    for e in &entries {
        if seen.contains(&e.id.as_str()) {
            report.problems.push(format!("{} appears twice in COMPATIBILITY §5", e.id));
        }
        seen.push(&e.id);
        let yaml = scen_dir.join(format!("{}.yaml", e.id));
        let scenario = match std::fs::read_to_string(&yaml) {
            Err(_) => {
                report.problems.push(format!("{}: missing {}", e.id, yaml.display()));
                FileState::Missing
            }
            Ok(text) => match Scenario::parse(&text, &yaml) {
                Err(err) => {
                    report.problems.push(err);
                    FileState::Missing
                }
                Ok(s) => {
                    if s.expect_divergence.as_deref() != Some(e.id.as_str()) {
                        report.problems.push(format!(
                            "{}: expect_divergence is {:?}, not {}",
                            yaml.display(),
                            s.expect_divergence,
                            e.id
                        ));
                    }
                    match s.status {
                        Status::Stub => FileState::Stub,
                        Status::Unit => {
                            for t in &s.unit_tests {
                                if !unit_test_exists(root, t) {
                                    report.problems.push(format!("{}: unit test {t} not found", e.id));
                                }
                            }
                            FileState::Unit(s.unit_tests)
                        }
                        Status::Active => FileState::Active,
                    }
                }
            },
        };
        let doc = doc_dir.join(format!("{}.md", e.id));
        let doc_filled = match std::fs::read_to_string(&doc) {
            Ok(t) => !doc_is_stub(&t),
            Err(_) => {
                report.problems.push(format!("{}: missing {}", e.id, doc.display()));
                false
            }
        };
        report.ids.push(IdStatus { entry: e.clone(), scenario, doc_filled });
    }
    for (dir, ext) in [(&scen_dir, "yaml"), (&doc_dir, "md")] {
        for f in list(dir, ext)? {
            let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if stem != "README" && !entries.iter().any(|e| e.id == stem) {
                report.problems.push(format!("{}: no register row {stem} in COMPATIBILITY §5", f.display()));
            }
        }
    }
    Ok(report)
}

fn list(dir: &Path, ext: &str) -> Result<Vec<PathBuf>, String> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == ext) {
            v.push(p);
        }
    }
    v.sort();
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "## 5. DIVERGENCES register\n\n### 5.1 CRASH — x\n\n| ID | a | b | WP |\n|---|---|---|---|\n\
| D-CRASH-01 | pc | snp text | 21 |\n| D-CRASH-04 | pc | other | 21, 13 |\n\n### 5.8 DIAG — y\n\n| ID | d | WP |\n\
|---|---|---|\n| D-DIAG-01 | warn | 18 |\n\n## 6. Not divergences\n\n| D-FAKE-01 | x | 1 |\n";

    #[test]
    fn parses_the_register_tables() {
        let r = parse_register(MD);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].id, "D-CRASH-01");
        assert_eq!(r[0].class, "CRASH");
        assert_eq!(r[0].summary, "snp text");
        assert_eq!(r[1].owners, vec![String::from("WP-21"), String::from("WP-13")]);
        assert_eq!(r[2].class, "DIAG");
        assert_eq!(r[2].summary, "warn");
    }

    #[test]
    fn the_workspace_register_is_consistent() {
        let root = crate::workspace_root();
        let rep = check(&root).unwrap();
        assert!(rep.problems.is_empty(), "{}", rep.render());
        assert!(rep.ids.len() >= 80);
        assert!(rep.ids.iter().any(|s| s.entry.id == "D-DIAG-05" && s.scenario == FileState::Active));
        assert!(rep.ids.iter().any(|s| matches!(s.scenario, FileState::Unit(_))));
        assert!(rep.render().starts_with("divergence register: "));
    }

    #[test]
    fn unit_test_lookup() {
        let root = crate::workspace_root();
        assert!(unit_test_exists(&root, "crates/snp-difftest/src/register.rs::tests::unit_test_lookup"));
        assert!(!unit_test_exists(&root, "crates/snp-difftest/src/register.rs::no_such_test"));
        assert!(!unit_test_exists(&root, "no-separator"));
        assert!(doc_is_stub("To be written by WP-32: why"));
    }
}
