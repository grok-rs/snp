//! Oracle-backed acceptance tests of the harness itself (WP-35): they need the pre-commit 4.7.0 oracle venvs of
//! `cargo xtask oracle install` and are `#[ignore]`d otherwise; the `difftest` CI workflow runs them with
//! `cargo test -p snp-difftest --test oracle -- --ignored`.
//!
//! * oracle-vs-oracle runs of the 20 seed scenarios show zero differences (self-consistency of the materialiser,
//!   the hermetic environment and the Tier-C normalisers);
//! * a divergence planted in an otherwise oracle-identical "snp" (`tests/differential/harness/planted-divergence.sh`)
//!   is found, shrunk to at most 3 hooks and 3 files, and the written `repro.sh` reproduces it.
//!
//! Owner: WP-35.

#[cfg(test)]
#[cfg(unix)]
mod oracle {
    use std::fmt::Write as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use snp_difftest::shrink::{self, hook_count};
    use snp_difftest::{
        Brand, Options, Runner, Scenario, StepKind, Verdict, load_scenarios, workspace_root,
    };

    const NEEDS: &str = "needs the pre-commit 4.7.0 oracle (cargo xtask oracle install)";

    fn oracle_dir() -> PathBuf {
        workspace_root().join("target/oracle/pc470-py312")
    }

    fn runner(snp_bin: PathBuf, root: &Path) -> Runner {
        let o = oracle_dir();
        let cli = workspace_root().join("target/oracle/pc470-py313");
        let cli = if cli.exists() { cli } else { o.clone() };
        Runner {
            snp_bin,
            oracle_bin: o.join("bin/pre-commit"),
            oracle_snp_prog: cli.join("bin/pre-commit-snp-prog"),
            root: root.to_path_buf(),
        }
    }

    #[test]
    #[ignore = "needs the pre-commit 4.7.0 oracle (cargo xtask oracle install)"]
    fn seed_scenarios_are_self_consistent() {
        assert!(oracle_dir().exists(), "{NEEDS}");
        let seeds =
            load_scenarios(&workspace_root().join("tests/differential/scenarios/seed")).unwrap();
        assert!(seeds.len() >= 20, "only {} seed scenarios", seeds.len());
        let tmp = tempfile::tempdir().unwrap();
        // The oracle against itself: no snp binary is involved.
        let r = runner(PathBuf::from("/nonexistent/snp"), tmp.path());
        let mut o = Options::detect(&r).unwrap();
        o.self_check = true;
        let mut failures = String::new();
        for s in &seeds {
            let out = r.run_with(s, &o);
            if !matches!(out.verdict, Verdict::Same) || out.error.is_some() {
                writeln!(failures, "{}", out.report()).unwrap();
            }
        }
        assert!(
            failures.is_empty(),
            "oracle-vs-oracle differences:\n{failures}"
        );
    }

    /// A noisy scenario around the planted difference: 12 hooks, 30 files, 3 steps, an environment entry.
    fn noisy() -> Scenario {
        let mut cfg = String::from("repos:\n-   repo: local\n    hooks:\n");
        for i in 0..12 {
            writeln!(
                cfg,
                "    -   {{id: h{i}, name: hook {i}, entry: 'true', language: system}}"
            )
            .unwrap();
        }
        let mut text = String::from("name: planted\nbrands: [compat]\nwork:\n  files:\n");
        for i in 0..29 {
            writeln!(
                text,
                "    f{i}.txt: \"line one\\nline {i}\\nline three\\n\""
            )
            .unwrap();
        }
        text.push_str("    planted.txt: \"before\\nPLANT\\nafter\\n\"\n  config: |\n");
        for l in cfg.lines() {
            writeln!(text, "    {l}").unwrap();
        }
        text.push_str("  commit: [.pre-commit-config.yaml]\n  stage: [");
        let names: Vec<String> = (0..29)
            .map(|i| format!("f{i}.txt"))
            .chain([String::from("planted.txt")])
            .collect();
        text.push_str(&names.join(", "));
        text.push_str("]\nsteps:\n  - tool: [run]\n    env: {SKIP: h3}\n  - tool: [run, --all-files]\n  - tool: [validate-config, .pre-commit-config.yaml]\n");
        Scenario::parse(&text, Path::new("planted.yaml")).unwrap()
    }

    #[test]
    #[ignore = "needs the pre-commit 4.7.0 oracle (cargo xtask oracle install)"]
    fn shrinker_reduces_a_planted_divergence_and_writes_a_working_repro() {
        assert!(oracle_dir().exists(), "{NEEDS}");
        let tmp = tempfile::tempdir().unwrap();
        let template = std::fs::read_to_string(
            workspace_root().join("tests/differential/harness/planted-divergence.sh"),
        )
        .unwrap();
        let oracle = oracle_dir().join("bin/pre-commit");
        let fake = tmp.path().join("fake-snp");
        std::fs::write(
            &fake,
            template.replace("@ORACLE@", &oracle.to_string_lossy()),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let r = runner(fake, &tmp.path().join("roots"));
        let mut o = Options::detect(&r).unwrap();
        o.brands = vec![Brand::Compat];

        let s = noisy();
        assert_eq!(hook_count(&s), 12);
        assert_eq!(s.work.files.len(), 30);
        let first = r.run_with(&s, &o);
        assert!(
            matches!(first.verdict, Verdict::Diverged { id: None, .. }),
            "the planted difference is not seen: {}",
            first.report()
        );

        let mut runs = 0_usize;
        let mut pred = |c: &Scenario| {
            runs += 1;
            let out = r.run_with(c, &o);
            out.error.is_none() && matches!(out.verdict, Verdict::Diverged { id: None, .. })
        };
        let shrunk = shrink::shrink(&s, &mut pred, 400);
        let small = &shrunk.scenario;
        assert!(
            hook_count(small) <= 3,
            "{} hooks left:\n{}",
            hook_count(small),
            small.to_yaml()
        );
        assert!(
            small.work.files.len() <= 3,
            "{} files left:\n{}",
            small.work.files.len(),
            small.to_yaml()
        );
        assert!(small.work.files.iter().any(|(p, _)| p == b"planted.txt"));
        assert!(
            small
                .steps
                .iter()
                .all(|st| matches!(st.kind, StepKind::Tool(_)))
        );
        assert!(runs > 0);

        // repro.sh: materialises the shrunk scenario, runs both sides, exits 1 because they differ.
        let root = r.scenario_root(small).unwrap();
        let script = shrink::repro_script(small, &r, &root);
        let path = tmp.path().join("repro.sh");
        std::fs::write(&path, &script).unwrap();
        let out = Command::new("sh")
            .arg(&path)
            .env("OUT", tmp.path().join("repro-out"))
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            out.status.code(),
            Some(1),
            "repro.sh must report the difference:\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.contains("planted divergence"), "{stdout}");
        // The same script against the plain oracle as "snp" finds nothing.
        let same = Command::new("sh")
            .arg(&path)
            .env("SNP_BIN", &oracle)
            .env("OUT", tmp.path().join("repro-same"))
            .output()
            .unwrap();
        assert_eq!(
            same.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&same.stdout)
        );
    }
}
