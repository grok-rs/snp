//! Release-mode timing smoke tests (`#[ignore = "perf"]`, run by the `perf-smoke` CI job; WP-36's bench gate
//! enforces the real budgets). Thresholds carry a 3× margin over the measured release-mode times.
//!
//! Owner: WP-21.

#[cfg(test)]
mod perf {
    use std::fmt::Write as _;
    use std::time::Instant;

    use snp_config::{ConfigSource, ExtMode, LoadOpts, RepoConfig, finalize_local};
    use snp_core::brand::Brand;
    use snp_core::output::CollectWarnings;
    use snp_pyre::RegexCache;

    /// A config of 50 repos × 10 hooks with regexes, stages and extension keys.
    fn big_config() -> String {
        let mut s =
            String::from("default_stages: [pre-commit, pre-push]\nexclude: '^vendor/'\nrepos:\n");
        for r in 0..25 {
            writeln!(
                s,
                "-   repo: https://example.com/r{r}\n    rev: v1.{r}.0\n    hooks:"
            )
            .expect("w");
            for h in 0..10 {
                writeln!(s, "    -   id: h{h}\n        files: '\\.(py|pyi)$'\n        args: [--a, --b={h}]\n        priority: {h}").expect("w");
            }
        }
        for r in 0..25 {
            writeln!(s, "-   repo: local\n    hooks:").expect("w");
            for h in 0..10 {
                writeln!(s, "    -   id: l{r}-{h}\n        name: local {h}\n        entry: echo {h}\n        language: system\n        types_or: [python, rust]\n        exclude: '^tests/fixtures/{r}'\n        groups: [g{h}]").expect("w");
            }
        }
        s
    }

    #[test]
    #[ignore = "perf"]
    fn load_and_merge_500_hooks_under_150ms() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join(".pre-commit-config.yaml");
        std::fs::write(&path, big_config()).expect("write");
        let src = ConfigSource {
            path,
            display: ".pre-commit-config.yaml".into(),
            project_root: dir.path().to_path_buf(),
        };
        let regexes = RegexCache::new();
        let sink = CollectWarnings::new();
        let o = LoadOpts {
            brand: Brand::PreCommit,
            ext: ExtMode::Enabled,
            regexes: &regexes,
            warn: &sink,
            diag: &sink,
        };
        let start = Instant::now();
        let cfg = snp_config::load_config(&src, &o).expect("load");
        let mut n = 0_usize;
        for repo in &cfg.repos {
            if let RepoConfig::Local { hooks } = repo {
                for h in hooks {
                    finalize_local(h, &cfg, "local", None, &sink).expect("merge");
                    n += 1;
                }
            }
        }
        let elapsed = start.elapsed();
        assert_eq!(n, 250);
        assert!(elapsed.as_millis() < 150, "load + merge took {elapsed:?}");
    }
}
