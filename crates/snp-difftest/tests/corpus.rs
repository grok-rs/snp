//! Integrity of the committed scenario corpus (no oracle needed; WP-35): every file parses, names are unique, every
//! `covers:` entry is a catalog ID of `tests/catalog/index.tsv`, every `expect_divergence` is a register ID of
//! COMPATIBILITY §5, and the corpus contains the sources WBS WP-35 lists — 20 seed scenarios, the crash recipes of
//! 13 X.10, the platform scenarios W01–W27 / M01–M07, the migration scenarios M11-MIG-01…24 and fast-path
//! transcripts — plus 2,000 generated cases that render and re-parse.
//!
//! Owner: WP-35.

#[cfg(test)]
mod corpus {
    use std::collections::BTreeSet;
    use std::path::Path;

    use snp_difftest::{Scenario, Status, generate, load_scenarios, register, workspace_root};

    fn corpus() -> Vec<Scenario> {
        load_scenarios(&workspace_root().join("tests/differential/scenarios")).unwrap()
    }

    fn catalog_ids() -> BTreeSet<String> {
        let tsv =
            std::fs::read_to_string(workspace_root().join("tests/catalog/index.tsv")).unwrap();
        tsv.lines()
            .skip(1)
            .filter_map(|l| l.split('\t').next())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn every_scenario_parses_and_is_active() {
        let all = corpus();
        assert!(all.len() >= 200, "only {} curated scenarios", all.len());
        for s in &all {
            assert_eq!(
                s.status,
                Status::Active,
                "{}: the curated corpus holds no stubs",
                s.path.display()
            );
            let rel = s
                .path
                .strip_prefix(workspace_root().join("tests/differential/scenarios"))
                .unwrap();
            let dir = rel.parent().and_then(Path::to_str).unwrap();
            assert!(
                s.name.starts_with(&format!("{dir}/")),
                "{}: name {:?} must start with {dir}/",
                rel.display(),
                s.name
            );
            // The YAML renderer round-trips every curated scenario (the shrinker relies on it).
            let back = Scenario::parse(&s.to_yaml(), &s.path).unwrap();
            assert_eq!(back.steps, s.steps, "{}", s.name);
            assert_eq!(back.work, s.work, "{}", s.name);
        }
    }

    #[test]
    fn covers_name_catalog_rows_and_divergences_name_register_ids() {
        let ids = catalog_ids();
        let md = std::fs::read_to_string(workspace_root().join("docs/COMPATIBILITY.md")).unwrap();
        let reg: BTreeSet<String> = register::parse_register(&md)
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert!(reg.len() >= 80);
        for s in corpus() {
            for c in &s.covers {
                assert!(
                    ids.contains(c),
                    "{}: covers {c}, which is not in tests/catalog/index.tsv",
                    s.name
                );
            }
            if let Some(id) = &s.expect_divergence {
                assert!(
                    reg.contains(id),
                    "{}: expect_divergence {id} is not in COMPATIBILITY §5",
                    s.name
                );
            }
        }
    }

    #[test]
    fn the_corpus_holds_every_source_of_the_brief() {
        let all = corpus();
        let tagged = |t: &str| all.iter().filter(|s| s.tags.iter().any(|x| x == t)).count();
        let under = |d: &str| all.iter().filter(|s| s.name.starts_with(d)).count();
        assert!(under("seed/") >= 20);
        // 13 X.10 recipes c01…c24 (c07 does not exist; c14 and c23 are folded into c06 and c18; c06/c17's python
        // environments are in repo/x13-sm-01-02-python-env).
        for c in [
            "c01", "c02", "c03", "c04", "c05", "c06", "c08", "c09", "c10", "c11", "c12", "c13",
            "c15", "c16", "c18", "c19", "c20", "c21", "c22", "c24",
        ] {
            assert!(
                all.iter()
                    .any(|s| s.name.starts_with(&format!("crash/{c}-"))),
                "crash recipe {c} missing"
            );
        }
        assert!(tagged("crash-recipe") >= 25);
        for i in 1..=27 {
            assert!(
                all.iter()
                    .any(|s| s.name.starts_with(&format!("platform/w{i:02}-"))),
                "platform W{i:02} missing"
            );
        }
        for i in 1..=7 {
            assert!(
                all.iter()
                    .any(|s| s.name.starts_with(&format!("platform/m{i:02}-"))),
                "platform M{i:02} missing"
            );
        }
        let covered: BTreeSet<&str> = all
            .iter()
            .flat_map(|s| s.covers.iter().map(String::as_str))
            .collect();
        for i in 1..=24 {
            let id = format!("M11-MIG-{i:02}");
            assert!(
                covered.contains(id.as_str()),
                "{id} has no interop scenario"
            );
        }
        assert!(tagged("fast-path") >= 3);
        // Every catalog family of 10 §5 that is reachable through the CLI has curated rows.
        for fam in [
            "CFG", "VAL", "RUN", "SFO", "XRG", "INS", "ITD", "HKI", "GIT", "STO", "REP", "AUP",
            "MIG", "TRY", "GC", "MET", "OUT", "ERR", "CLI", "HAZ", "UTL",
        ] {
            let p = format!("T10-{fam}-");
            assert!(
                covered.iter().any(|c| c.starts_with(&p)),
                "no scenario covers a {p}* row"
            );
        }
        assert!(covered.iter().filter(|c| c.starts_with("T10-")).count() >= 150);
    }

    #[test]
    fn divergence_files_parse() {
        let d = load_scenarios(&workspace_root().join("tests/differential/divergences")).unwrap();
        assert!(d.len() >= 80);
        for s in &d {
            assert!(s.expect_divergence.is_some(), "{}", s.path.display());
        }
    }

    #[test]
    fn two_thousand_generated_cases_render_and_reparse() {
        let mut names = BTreeSet::new();
        for i in 0..2000 {
            let s = generate::generate(20_261_101, i, i % 2 == 0);
            let back = Scenario::parse(&s.to_yaml(), Path::new("generated.yaml")).unwrap();
            assert_eq!(back.steps, s.steps, "case {i}");
            assert!(
                back.unsupported.is_empty(),
                "case {i}: {:?}",
                back.unsupported
            );
            assert!(names.insert(s.name.clone()));
        }
    }
}
