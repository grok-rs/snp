//! `cargo xtask catalog-status [--owner WP-xx]… [--wave W1|W2|W3]… [--check] [--all]`: ported vs total catalog IDs
//! from `tests/catalog/index.tsv` (ADR-062, ARCHITECTURE §13.8).
//!
//! A row is **ported** when some Rust function in the workspace (`crates/`, `xtask/`, `fuzz/`; `target/` skipped) is
//! named after its namespaced ID: `T10-XRG-03` ↔ `fn t10_xrg_03` or `fn t10_xrg_03_<description>` (the WBS naming
//! rule). The report lists, per owner, ported/total for all rows and for the hermetic rows, then every unported row
//! (hermetic first). Integrity checks that always fail the task: an ID listed twice in the index, a hermetic row
//! without an owning work package, and a test name that resolves to two IDs (e.g. a malformed `t10_run_1_…` next to
//! `T10-RUN-1` and `T10-RUN-10`). `--check` additionally fails when a hermetic row of the selected owners (all owners
//! when none is selected; `--wave` selects the packages of a wave from the WBS overview table) is unported — the
//! wave exit gate of docs/WBS.md.
//!
//! Owner: WP-35.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;

use anyhow::{Context as _, bail};

use crate::workspace::{self, say};

const INDEX: &str = "tests/catalog/index.tsv";

/// One index row.
#[derive(Debug, Clone)]
struct Row {
    id: String,
    title: String,
    owner: String,
    hermetic: bool,
    test_file: String,
}

fn parse_index(text: &str) -> anyhow::Result<Vec<Row>> {
    let mut rows = Vec::new();
    for (n, line) in text.lines().enumerate().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let cells: Vec<&str> = line.split('\t').collect();
        let [id, _source, title, owner, hermetic, test_file] = cells.as_slice() else {
            bail!(
                "{INDEX}:{}: expected 6 tab-separated columns",
                n.saturating_add(1)
            );
        };
        rows.push(Row {
            id: (*id).to_owned(),
            title: (*title).to_owned(),
            owner: (*owner).to_owned(),
            hermetic: *hermetic == "yes",
            test_file: (*test_file).to_owned(),
        });
    }
    Ok(rows)
}

/// `T10-XRG-03` → `t10_xrg_03`.
fn id_prefix(id: &str) -> String {
    id.to_ascii_lowercase().replace('-', "_")
}

/// Every `fn <name>` in `text` whose name starts with a catalog namespace.
fn catalog_fns(text: &str, out: &mut BTreeMap<String, String>, file: &str) {
    for (i, _) in text.match_indices("fn ") {
        let before = i.checked_sub(1).and_then(|p| text.as_bytes().get(p));
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
            continue;
        }
        let rest = text.get(i.saturating_add(3)..).unwrap_or_default();
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if ["t10_", "x13_", "m11_"].iter().any(|p| name.starts_with(p)) {
            out.entry(name).or_insert_with(|| file.to_owned());
        }
    }
}

/// The work packages of each wave (`| W2 | WP-21 | …` rows of the WBS overview table).
fn waves(wbs: &str) -> BTreeMap<String, Vec<String>> {
    let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in wbs.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if let [_, wave, wp, ..] = cells.as_slice()
            && wave.len() == 2
            && wave.starts_with('W')
            && wp.starts_with("WP-")
        {
            m.entry((*wave).to_owned())
                .or_default()
                .push((*wp).to_owned());
        }
    }
    m
}

/// The result of joining the index with the test names.
#[derive(Debug, Default)]
struct Status {
    /// Row ID → the test function porting it.
    ported: BTreeMap<String, (String, String)>,
    /// Integrity problems.
    problems: Vec<String>,
}

fn join(rows: &[Row], fns: &BTreeMap<String, String>) -> Status {
    let mut st = Status::default();
    let mut seen = BTreeSet::new();
    for r in rows {
        if !seen.insert(r.id.as_str()) {
            st.problems
                .push(format!("{} is listed twice in {INDEX}", r.id));
        }
        if r.hermetic && !r.owner.starts_with("WP-") {
            st.problems
                .push(format!("hermetic row {} has no owning work package", r.id));
        }
    }
    let prefixes: Vec<(String, &str)> = rows
        .iter()
        .map(|r| (id_prefix(&r.id), r.id.as_str()))
        .collect();
    for (name, file) in fns {
        // A set: an ID listed twice is reported once above, not again as an ambiguous test name.
        let hits: BTreeSet<&str> = prefixes
            .iter()
            .filter(|(p, _)| {
                name == p
                    || name
                        .strip_prefix(p.as_str())
                        .is_some_and(|rest| rest.starts_with('_'))
            })
            .map(|(_, id)| *id)
            .collect();
        let hits: Vec<&str> = hits.into_iter().collect();
        match hits.as_slice() {
            [] => {}
            [id] => {
                st.ported
                    .entry((*id).to_owned())
                    .or_insert_with(|| (name.clone(), file.clone()));
            }
            many => st.problems.push(format!(
                "test {name} ({file}) resolves to several rows: {}",
                many.join(", ")
            )),
        }
    }
    st
}

fn pct(a: usize, b: usize) -> String {
    if b == 0 {
        return String::from("—");
    }
    let tenths = a.saturating_mul(1000).checked_div(b).unwrap_or(0);
    format!("{}.{} %", tenths / 10, tenths % 10)
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let mut owners: Vec<String> = Vec::new();
    let mut wave_sel: Vec<String> = Vec::new();
    let mut check = false;
    let mut all = false;
    let mut it = args.iter().map(|a| a.to_string_lossy().into_owned());
    while let Some(a) = it.next() {
        match a.as_str() {
            "--owner" => owners.push(it.next().context("--owner needs WP-xx")?),
            "--wave" => wave_sel.push(it.next().context("--wave needs W0…W3")?),
            "--check" => check = true,
            "--all" => all = true,
            other => bail!(
                "usage: cargo xtask catalog-status [--owner WP-xx]… [--wave Wn]… [--check] [--all] (got {other:?})"
            ),
        }
    }
    let root = workspace::root()?;
    let text =
        std::fs::read_to_string(root.join(INDEX)).with_context(|| format!("reading {INDEX}"))?;
    let rows = parse_index(&text)?;
    if !wave_sel.is_empty() {
        let wbs =
            std::fs::read_to_string(root.join("docs/WBS.md")).context("reading docs/WBS.md")?;
        let w = waves(&wbs);
        for sel in &wave_sel {
            let wps = w
                .get(sel)
                .with_context(|| format!("no wave {sel:?} in the WBS overview table"))?;
            owners.extend(wps.iter().cloned());
        }
    }
    let mut fns = BTreeMap::new();
    for dir in ["crates", "xtask", "fuzz", "tests"] {
        for f in workspace::rust_files(&root.join(dir))? {
            let t =
                std::fs::read_to_string(&f).with_context(|| format!("reading {}", f.display()))?;
            catalog_fns(&t, &mut fns, &workspace::rel(&root, &f));
        }
    }
    let st = join(&rows, &fns);
    report(&rows, &st, &owners, all);
    let selected = |r: &Row| owners.is_empty() || owners.contains(&r.owner);
    let missing: Vec<&Row> = rows
        .iter()
        .filter(|r| selected(r) && r.hermetic && !st.ported.contains_key(&r.id))
        .collect();
    if !st.problems.is_empty() {
        bail!("{} catalog integrity problem(s)", st.problems.len());
    }
    if check && !missing.is_empty() {
        bail!(
            "{} hermetic catalog row(s) of the selected owners are not ported",
            missing.len()
        );
    }
    Ok(())
}

fn report(rows: &[Row], st: &Status, owners: &[String], all: bool) {
    let mut by_owner: BTreeMap<&str, Vec<&Row>> = BTreeMap::new();
    for r in rows {
        by_owner.entry(r.owner.as_str()).or_default().push(r);
    }
    let herm_total = rows.iter().filter(|r| r.hermetic).count();
    let herm_owned = rows
        .iter()
        .filter(|r| r.hermetic && r.owner.starts_with("WP-"))
        .count();
    let ported_total = rows
        .iter()
        .filter(|r| st.ported.contains_key(&r.id))
        .count();
    say(&format!(
        "catalog: {} rows ({herm_total} hermetic, {} owned = {}); ported {ported_total}/{} ({})",
        rows.len(),
        herm_owned,
        pct(herm_owned, herm_total),
        rows.len(),
        pct(ported_total, rows.len())
    ));
    say(&format!(
        "{:<8} {:>15} {:>19}",
        "owner", "ported/total", "hermetic ported"
    ));
    for (owner, rs) in &by_owner {
        if !owners.is_empty() && !owners.iter().any(|o| o == owner) {
            continue;
        }
        let p = rs.iter().filter(|r| st.ported.contains_key(&r.id)).count();
        let h = rs.iter().filter(|r| r.hermetic).count();
        let hp = rs
            .iter()
            .filter(|r| r.hermetic && st.ported.contains_key(&r.id))
            .count();
        say(&format!(
            "{owner:<8} {:>15} {:>19}",
            format!("{p}/{}", rs.len()),
            format!("{hp}/{h} ({})", pct(hp, h))
        ));
    }
    for (owner, rs) in &by_owner {
        if !owners.is_empty() && !owners.iter().any(|o| o == owner) {
            continue;
        }
        let mut unported: Vec<&&Row> = rs
            .iter()
            .filter(|r| !st.ported.contains_key(&r.id))
            .collect();
        if unported.is_empty() {
            continue;
        }
        unported.sort_by_key(|r| (!r.hermetic, r.id.clone()));
        say(&format!("\nunported rows of {owner} ({}):", unported.len()));
        for r in unported {
            let tag = if r.hermetic {
                "hermetic"
            } else {
                "non-hermetic"
            };
            say(&format!(
                "  {} [{tag}] {} -> {}",
                r.id, r.title, r.test_file
            ));
        }
    }
    if all {
        say("\nported rows:");
        for (id, (name, file)) in &st.ported {
            say(&format!("  {id}: {file}::{name}"));
        }
    }
    for p in &st.problems {
        say(&format!("PROBLEM: {p}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TSV: &str = "id\tsource\ttitle\towner\thermetic\ttest_file\n\
T10-RUN-01\t10 §5\ta\tWP-32\tyes\tx.rs\nT10-RUN-10\t10 §5\tb\tWP-32\tyes\tx.rs\n\
X13-CFG-01\t13\tc\tWP-21\tno\ty.rs\n";

    #[test]
    fn joins_rows_and_test_names() {
        let rows = parse_index(TSV).unwrap();
        assert_eq!(rows.len(), 3);
        let mut fns = BTreeMap::new();
        catalog_fns(
            "fn t10_run_01_basic() {}\nfn t10_run_010() {}\n  pub fn x13_cfg_01() {}\nfn nott10_run_10_x() {}", // spellchecker:disable-line
            &mut fns,
            "a.rs",
        );
        let st = join(&rows, &fns);
        assert!(st.ported.contains_key("T10-RUN-01"));
        assert!(st.ported.contains_key("X13-CFG-01"));
        assert!(
            !st.ported.contains_key("T10-RUN-10"),
            "t10_run_010 must not match T10-RUN-01 or -10"
        );
        assert!(st.problems.is_empty(), "{:?}", st.problems);
        let dup = parse_index(&format!("{TSV}T10-RUN-01\t10\tz\t—\tyes\tz.rs\n")).unwrap();
        let st = join(&dup, &fns);
        assert_eq!(st.problems.len(), 2, "{:?}", st.problems);
    }

    #[test]
    fn waves_and_percentages() {
        let w = waves(
            "| W1 | WP-11 | x | y |\n| W1 | WP-12 | x | y |\n| W3 | WP-35 | z | q |\n| Wave | x |\n",
        );
        assert_eq!(w["W1"], vec![String::from("WP-11"), String::from("WP-12")]);
        assert_eq!(pct(1, 3), "33.3 %");
        assert_eq!(pct(0, 0), "—");
        assert_eq!(id_prefix("M11-MIG-07"), "m11_mig_07");
    }

    #[test]
    fn the_workspace_index_has_no_integrity_problems() {
        let root = workspace::root().unwrap();
        let rows = parse_index(&std::fs::read_to_string(root.join(INDEX)).unwrap()).unwrap();
        let mut fns = BTreeMap::new();
        for f in workspace::rust_files(&root.join("crates")).unwrap() {
            catalog_fns(
                &std::fs::read_to_string(&f).unwrap(),
                &mut fns,
                &workspace::rel(&root, &f),
            );
        }
        let st = join(&rows, &fns);
        assert!(st.problems.is_empty(), "{:?}", st.problems);
        assert!(!st.ported.is_empty());
    }
}
