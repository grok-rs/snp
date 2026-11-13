//! `cargo xtask catalog-index [--check]`: generate `tests/catalog/index.tsv` from 10 §5, 13 X.4/X.5 and 11 §A.20
//! (ADR-062, ARCHITECTURE §13.8).
//!
//! Every catalog row gets its namespaced ID (`T10-…`, `X13-…`, `M11-…`), its source section, a title (the row's
//! first descriptive cell, shortened), its owner from the WBS "Catalog ownership" table (family owner plus row
//! exceptions, parsed from docs/WBS.md), a hermetic flag and the planned test file (the owner's catalog file; the
//! rows WP-00 ported in W0 point at their files). Tests are named after the ID (`t10_xrg_03_…`). The family counts
//! are checked against the WBS WP-00 acceptance list. `--check` fails when the committed index differs from a fresh
//! generation (only a change of the research documents or of the WBS table changes it).
//!
//! Hermetic = runnable without a language toolchain, the network, the pre-commit oracle or a Windows-only host:
//! T10 rows whose "Needs" cell names `tc:…`, `net` or `win` (or that say "win only") are not hermetic (10 Appendix A);
//! X13 rows that need a language toolchain ([`X13_TOOLCHAIN_ROWS`]) are not; M11 rows that run the 4.7.0 oracle or
//! need the network (`interop_*`, `net_*`, 11 §A.20) are not (except [`M11_HERMETIC_DESPITE_ORACLE`]).
//!
//! Owner: WP-00.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context as _, bail, ensure};

use crate::workspace;

/// Where the index lives.
const INDEX: &str = "tests/catalog/index.tsv";
/// Longest title (characters, before the ellipsis).
const TITLE_MAX: usize = 100;
/// X13 rows that need a language toolchain (X.4.5 EXE-08: lua without luarocks; X.4.7 ENV-03: language backends).
const X13_TOOLCHAIN_ROWS: [&str; 2] = ["EXE-08", "ENV-03"];

/// M11 rows that mention the oracle only as the comparison of a registered deviation (MIG-12: "the oracle skips
/// it"), so they run without it.
const M11_HERMETIC_DESPITE_ORACLE: [&str; 1] = ["MIG-12"];

/// Row counts per family (WBS WP-00 acceptance).
const EXPECTED: &[(&str, &str, usize)] = &[
    ("T10", "CFG", 29),
    ("T10", "VAL", 4),
    ("T10", "RUN", 46),
    ("T10", "SFO", 17),
    ("T10", "XRG", 22),
    ("T10", "INS", 34),
    ("T10", "ITD", 5),
    ("T10", "HKI", 9),
    ("T10", "GIT", 14),
    ("T10", "STO", 13),
    ("T10", "REP", 16),
    ("T10", "AUP", 17),
    ("T10", "MIG", 9),
    ("T10", "TRY", 7),
    ("T10", "GC", 8),
    ("T10", "MET", 8),
    ("T10", "OUT", 5),
    ("T10", "ERR", 5),
    ("T10", "CLI", 7),
    ("T10", "HAZ", 5),
    ("T10", "UTL", 7),
    ("X13", "CFG", 14),
    ("X13", "ST", 12),
    ("X13", "GIT", 10),
    ("X13", "CLS", 5),
    ("X13", "EXE", 8),
    ("X13", "STASH", 3),
    ("X13", "ENV", 4),
    ("X13", "HI", 8),
    ("X13", "HZ", 6),
    ("X13", "CMD", 12),
    ("X13", "EH", 5),
    ("X13", "SM", 10),
    ("M11", "MIG", 24),
];

/// One research document and the part of it that holds catalog rows.
struct Source {
    /// The namespace prefix (`T10`, `X13`, `M11`).
    prefix: &'static str,
    /// The document number used in section names (`10`, `13`, `11`).
    doc: &'static str,
    path: &'static str,
    /// First line of the catalog part (prefix match).
    start: &'static str,
    /// First line after it (prefix match).
    end: &'static str,
}

const SOURCES: [Source; 3] = [
    Source {
        prefix: "T10",
        doc: "10",
        path: "docs/research/10-test-strategy.md",
        start: "## 5. ",
        end: "## 6. ",
    },
    Source {
        prefix: "X13",
        doc: "13",
        path: "docs/research/13-crash-and-message-catalog.md",
        start: "### X.4 ",
        end: "### X.6 ",
    },
    Source {
        prefix: "M11",
        doc: "11",
        path: "docs/research/11-dropin-migration.md",
        start: "### A.20 ",
        end: "### A.21 ",
    },
];

/// One generated index line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) title: String,
    pub(crate) owner: String,
    pub(crate) hermetic: bool,
    pub(crate) test_file: String,
}

/// A parsed catalog row before ownership.
#[derive(Debug)]
struct RawRow {
    prefix: &'static str,
    family: String,
    number: u32,
    id: String,
    section: String,
    title: String,
    hermetic: bool,
}

/// Splits a Markdown table line into trimmed cells (`\|` is an escaped pipe).
fn cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|');
    let inner = inner.trim_suffix('|');
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    for ch in inner.chars() {
        if escaped {
            if ch != '|' {
                cur.push('\\');
            }
            cur.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '|' {
            out.push(cur.trim().to_owned());
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    if escaped {
        cur.push('\\');
    }
    out.push(cur.trim().to_owned());
    out
}

/// `FAM-NN` → (`FAM`, NN).
fn split_id(s: &str) -> Option<(String, u32)> {
    let (fam, num) = s.split_once('-')?;
    let ok_fam = !fam.is_empty() && fam.bytes().all(|b| b.is_ascii_uppercase());
    let ok_num = !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit());
    if !(ok_fam && ok_num) {
        return None;
    }
    Some((fam.to_owned(), num.parse().ok()?))
}

/// A single-line, tab-free, shortened title.
fn clean_title(s: &str) -> String {
    let s = s.replace("**", "").replace('`', "");
    let words: Vec<&str> = s.split_whitespace().collect();
    let joined = words.join(" ");
    if joined.chars().count() <= TITLE_MAX {
        return joined;
    }
    // Cut at the last word boundary inside the limit (no half words).
    let t: String = joined.chars().take(TITLE_MAX).collect();
    let cut = t
        .rfind(' ')
        .map_or(t.as_str(), |i| t.get(..i).unwrap_or(&t));
    let mut t = cut.trim_end().to_owned();
    t.push('…');
    t
}

/// Whether a word occurs in `s` delimited by non-alphanumerics.
fn has_word(s: &str, word: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|w| w == word)
}

/// The catalog rows of one source document.
fn parse_source(src: &Source, text: &str) -> anyhow::Result<Vec<RawRow>> {
    let mut rows = Vec::new();
    let mut inside = false;
    let mut section = String::new();
    let mut header: Vec<String> = Vec::new();
    for line in text.lines() {
        if !inside {
            inside = line.starts_with(src.start);
            if inside {
                section = section_name(src, line);
            }
            continue;
        }
        if line.starts_with(src.end) {
            break;
        }
        if line.starts_with('#') {
            section = section_name(src, line);
            header.clear();
            continue;
        }
        if !line.starts_with('|') {
            continue;
        }
        let c = cells(line);
        let Some(first) = c.first() else { continue };
        if first == "ID" || first == "Id" {
            header = c.iter().map(|h| h.to_ascii_lowercase()).collect();
            continue;
        }
        let Some((family, number)) = split_id(first) else {
            continue; // separator row or a non-catalog table
        };
        ensure!(
            !header.is_empty(),
            "{}: row {first} outside a table",
            src.path
        );
        let title = c
            .iter()
            .zip(&header)
            .skip(1)
            .find(|(v, h)| {
                h.as_str() != "source"
                    && h.as_str() != "where"
                    && !matches!(v.as_str(), "" | "—" | "-")
            })
            .map_or_default(|(v, _)| clean_title(v));
        let hermetic = match src.prefix {
            "T10" => {
                let needs = header
                    .iter()
                    .position(|h| h.starts_with("needs"))
                    .and_then(|i| c.get(i))
                    .map_or_default(String::as_str);
                let row_text = c.join(" ");
                !(needs.contains("tc:")
                    || has_word(needs, "net")
                    || has_word(needs, "win")
                    || row_text.contains("win only"))
            }
            "X13" => !X13_TOOLCHAIN_ROWS.contains(&first.as_str()),
            _ => {
                let row_text = c.join(" ");
                M11_HERMETIC_DESPITE_ORACLE.contains(&first.as_str())
                    || !(row_text.contains("oracle") || row_text.contains("net_"))
            }
        };
        rows.push(RawRow {
            prefix: src.prefix,
            id: format!("{}-{first}", src.prefix),
            family,
            number,
            section: section.clone(),
            title,
            hermetic,
        });
    }
    ensure!(
        inside,
        "{}: catalog start `{}` not found",
        src.path,
        src.start
    );
    Ok(rows)
}

/// `10 §5.4` / `13 X.4.1` / `13 X.5` / `11 §A.20` from a heading line.
fn section_name(src: &Source, heading: &str) -> String {
    let title = heading.trim_start_matches('#').trim();
    let num = title.split_whitespace().next().unwrap_or_default();
    let num = num.trim_end_matches('.');
    if src.prefix == "X13" {
        format!("{} {num}", src.doc)
    } else {
        format!("{} §{num}", src.doc)
    }
}

/// Ownership from the WBS table: family owners and row exceptions, keyed by (prefix, family[, number]).
#[derive(Debug, Default)]
pub(crate) struct Ownership {
    families: BTreeMap<(String, String), String>,
    rows: BTreeMap<(String, String, u32), String>,
}

/// The first `WP-NN` in `s`.
fn find_wp(s: &str) -> Option<String> {
    let i = s.find("WP-")?;
    let digits: String = s
        .get(i.saturating_add(3)..)?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (!digits.is_empty()).then(|| format!("WP-{digits}"))
}

/// Parses the "Catalog ownership" table of docs/WBS.md.
pub(crate) fn parse_ownership(wbs: &str) -> anyhow::Result<Ownership> {
    let mut own = Ownership::default();
    let mut inside = false;
    for line in wbs.lines() {
        if line.starts_with("### Catalog ownership") {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line.starts_with('#') {
            break;
        }
        if !line.starts_with('|') {
            continue;
        }
        let c = cells(line);
        let (Some(fams), Some(owner), Some(exc)) = (c.get(1), c.get(2), c.get(3)) else {
            continue;
        };
        let mut families = Vec::new();
        for prefix in ["T10", "X13", "M11"] {
            let mut rest = fams.as_str();
            let pat = format!("{prefix}-");
            while let Some(i) = rest.find(&pat) {
                let after = rest.get(i.saturating_add(pat.len())..).unwrap_or_default();
                let fam: String = after.chars().take_while(char::is_ascii_uppercase).collect();
                if !fam.is_empty() {
                    families.push((prefix, fam));
                }
                rest = after;
            }
        }
        let Some(&(prefix, _)) = families.first() else {
            continue; // e.g. "language rows (no IDs)"
        };
        let owner = find_wp(owner).with_context(|| format!("no owner in WBS row `{line}`"))?;
        for (p, fam) in &families {
            let prev = own
                .families
                .insert(((*p).to_owned(), fam.clone()), owner.clone());
            ensure!(
                prev.is_none(),
                "family {p}-{fam} has two owners in the WBS table"
            );
        }
        if exc.starts_with('—') || exc.is_empty() {
            continue;
        }
        for part in exc.split(';') {
            let (lhs, rhs) = part
                .split_once('→')
                .with_context(|| format!("bad exception `{part}`"))?;
            let wp = find_wp(rhs).with_context(|| format!("no owner in exception `{part}`"))?;
            let mut fam = String::new();
            for tok in lhs.split(',') {
                let tok = tok.trim();
                let num = if let Some((f, n)) = split_id(tok) {
                    fam = f;
                    n
                } else {
                    tok.parse::<u32>()
                        .with_context(|| format!("bad row number `{tok}` in `{part}`"))?
                };
                ensure!(!fam.is_empty(), "exception without a family: `{part}`");
                own.rows
                    .insert((prefix.to_owned(), fam.clone(), num), wp.clone());
            }
        }
    }
    ensure!(
        !own.families.is_empty(),
        "docs/WBS.md: no \"Catalog ownership\" table found"
    );
    Ok(own)
}

impl Ownership {
    fn owner(&self, r: &RawRow) -> Option<&String> {
        self.rows
            .get(&(r.prefix.to_owned(), r.family.clone(), r.number))
            .or_else(|| self.families.get(&(r.prefix.to_owned(), r.family.clone())))
    }
}

/// The owner's planned catalog test file.
fn default_test_file(owner: &str, prefix: &str, family: &str, number: u32) -> String {
    let it = |m: &str| format!("crates/snp/tests/it/{m}.rs");
    let krate = |c: &str| format!("crates/{c}/tests/catalog.rs");
    match owner {
        "WP-00" => krate("snp-core"),
        "WP-11" => krate("snp-pycompat"),
        "WP-12" => krate("snp-cli"),
        "WP-13" => krate("snp-pyre"),
        "WP-14" => krate("snp-yaml"),
        "WP-15" => krate("snp-proc"),
        "WP-16" => krate("snp-lang"),
        "WP-17" => krate("snp-hooks-data"),
        "WP-18" => krate("snp-git"),
        "WP-21" => krate("snp-config"),
        "WP-22" if family == "CLS" || (prefix == "X13" && family == "GIT") => krate("snp-files"),
        "WP-22" => krate("snp-store"),
        "WP-23" => krate("snp-stash"),
        "WP-26" | "WP-27" => krate("snp-hooks"),
        "WP-31" => it("repository"),
        "WP-32" => match family {
            "HKI" | "HI" => it("hook_impl"),
            "TRY" => it("try_repo"),
            "MET" => it("meta"),
            "OUT" => it("output"),
            "XRG" => it("xargs"),
            _ => it("run"),
        },
        "WP-33" => match family {
            "VAL" => it("validate"),
            "INS" | "MIG" => it("install"),
            "ITD" => it("init_templatedir"),
            "AUP" | "GIT" => it("autoupdate"),
            "GC" if number == 8 => it("clean"),
            "GC" => it("gc"),
            "CLI" => it("cli"),
            "HZ" => it("hazmat_cli"),
            _ => it("errors"),
        },
        "WP-34" => it("migrate_lefthook"),
        "WP-35" => krate("snp-difftest"),
        "WP-36" => it("budgets"),
        _ => String::new(),
    }
}

/// Rows ported in W0 by WP-00 (the baselines and snp-core): their test files at generation time. Later owners
/// may move a test inside their crate; `catalog-status` (WP-35) finds tests by name, so this column is the planned
/// (initial) home and never needs regenerating when a row is ported.
const W0_PORTED: [(&str, &str); 8] = [
    ("T10-UTL-01", "crates/snp-core/src/failure.rs"),
    ("T10-UTL-06", "crates/snp-core/src/env.rs"),
    ("T10-XRG-03", "crates/snp-proc/src/xargs.rs"),
    ("T10-XRG-04", "crates/snp-proc/src/xargs.rs"),
    ("T10-XRG-05", "crates/snp-proc/src/xargs.rs"),
    ("T10-XRG-06", "crates/snp-proc/src/xargs.rs"),
    ("T10-XRG-07", "crates/snp-proc/src/xargs.rs"),
    ("T10-XRG-15", "crates/snp-proc/src/xargs.rs"),
];

/// Generates every index row (in document order) after validating counts and ownership.
pub(crate) fn generate(root: &Path) -> anyhow::Result<Vec<Row>> {
    let wbs = std::fs::read_to_string(root.join("docs/WBS.md")).context("reading docs/WBS.md")?;
    let own = parse_ownership(&wbs)?;
    let mut raw = Vec::new();
    for src in &SOURCES {
        let text = std::fs::read_to_string(root.join(src.path))
            .with_context(|| format!("reading {}", src.path))?;
        raw.extend(parse_source(src, &text)?);
    }

    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    let mut counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for r in &raw {
        if !seen.insert(r.id.clone()) {
            problems.push(format!("duplicate catalog ID {}", r.id));
        }
        let e = counts.entry((r.prefix, r.family.as_str())).or_default();
        *e = e.saturating_add(1);
    }
    for (prefix, fam, n) in EXPECTED {
        let got = counts.get(&(*prefix, *fam)).copied().unwrap_or_default();
        if got != *n {
            problems.push(format!(
                "{prefix}-{fam}: {got} rows, the WBS acceptance expects {n}"
            ));
        }
    }
    for (prefix, fam) in counts.keys() {
        if !EXPECTED.iter().any(|(p, f, _)| p == prefix && f == fam) {
            problems.push(format!(
                "family {prefix}-{fam} is not in the WBS acceptance list"
            ));
        }
    }
    for (prefix, fam, num) in own.rows.keys() {
        if !raw
            .iter()
            .any(|r| r.prefix == prefix && &r.family == fam && r.number == *num)
        {
            problems.push(format!(
                "WBS exception {prefix}-{fam}-{num:02} names no catalog row"
            ));
        }
    }

    let mut rows = Vec::new();
    for r in raw {
        let Some(owner) = own.owner(&r) else {
            problems.push(format!(
                "{} has no owner in the WBS \"Catalog ownership\" table",
                r.id
            ));
            continue;
        };
        let test_file = W0_PORTED.iter().find(|(id, _)| *id == r.id).map_or_else(
            || default_test_file(owner, r.prefix, &r.family, r.number),
            |(_, f)| (*f).to_owned(),
        );
        rows.push(Row {
            id: r.id,
            source: r.section,
            title: r.title,
            owner: owner.clone(),
            hermetic: r.hermetic,
            test_file,
        });
    }
    if !problems.is_empty() {
        bail!("catalog problems:\n  {}", problems.join("\n  "));
    }
    Ok(rows)
}

/// The TSV text of `rows` (header first, `\n` line ends).
pub(crate) fn render(rows: &[Row]) -> String {
    let mut out = String::from("id\tsource\ttitle\towner\thermetic\ttest_file\n");
    for r in rows {
        writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}",
            r.id,
            r.source,
            r.title,
            r.owner,
            if r.hermetic { "yes" } else { "no" },
            r.test_file
        )
        .unwrap_or_default();
    }
    out
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let check = match args {
        [] => false,
        [a] if a == "--check" => true,
        _ => bail!("usage: cargo xtask catalog-index [--check]"),
    };
    let root = workspace::root()?;
    let text = render(&generate(&root)?);
    let path = root.join(INDEX);
    if check {
        let current = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        ensure!(
            current == text,
            "{INDEX} is stale: run `cargo xtask catalog-index` and commit the result"
        );
        workspace::say(&format!("catalog-index: {INDEX} is up to date"));
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, &text).with_context(|| format!("writing {INDEX}"))?;
    let rows = text.lines().count().saturating_sub(1);
    workspace::say(&format!("catalog-index: wrote {INDEX} ({rows} rows)"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_and_ids() {
        assert_eq!(cells("| A-01 | `a\\|b` | c |"), ["A-01", "`a|b`", "c"]);
        assert_eq!(split_id("XRG-07"), Some(("XRG".to_owned(), 7)));
        assert_eq!(split_id("---"), None);
        assert_eq!(split_id("Id"), None);
    }

    #[test]
    fn ownership_table_parses() {
        let wbs = "### Catalog ownership\n\n| Source | Family | Owner | Row exceptions |\n|---|---|---|---|\n\
                   | 10 §5 | T10-XRG-01…22 | WP-15 | XRG-16 → WP-11; XRG-17, 18 → WP-32 |\n\
                   | 13 X.4 | X13-SM-01…10 | WP-31 | SM-03, 10 → WP-21 |\n### Next\n";
        let own = parse_ownership(wbs).unwrap();
        assert_eq!(own.families.len(), 2);
        assert_eq!(
            own.rows.get(&("T10".into(), "XRG".into(), 18)),
            Some(&"WP-32".to_owned())
        );
        assert_eq!(
            own.rows.get(&("X13".into(), "SM".into(), 10)),
            Some(&"WP-21".to_owned())
        );
    }

    #[test]
    fn the_index_is_complete_and_current() {
        let root = workspace::root().unwrap();
        let rows = generate(&root).unwrap();
        let total: usize = EXPECTED.iter().map(|(_, _, n)| n).sum();
        assert_eq!(rows.len(), total);
        assert!(
            rows.iter()
                .all(|r| r.owner.starts_with("WP-") && !r.test_file.is_empty())
        );
        let xrg14 = rows.iter().find(|r| r.id == "T10-XRG-14").unwrap();
        assert!(!xrg14.hermetic);
        let xrg03 = rows.iter().find(|r| r.id == "T10-XRG-03").unwrap();
        assert_eq!(xrg03.test_file, "crates/snp-proc/src/xargs.rs");
        assert_eq!(
            rows.iter().find(|r| r.id == "T10-UTL-06").unwrap().owner,
            "WP-00"
        );
        let committed = std::fs::read_to_string(root.join(INDEX))
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(committed, render(&rows), "run `cargo xtask catalog-index`");
    }
}
