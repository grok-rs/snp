//! Timing thresholds of WP-12 (WBS `DoD`: release-mode `#[ignore = "perf"]` tests with a 3× margin, run by the
//! `perf-smoke` CI job; WP-36's bench gate enforces the exact 5 µs target of 12 §A.17 item 8).
//!
//! Owner: WP-12.

#[cfg(test)]
mod perf {
    use std::ffi::OsString;
    use std::hint::black_box;
    use std::time::{Duration, Instant};

    use snp_cli::{Cli, CliOutcome};
    use snp_core::brand::Brand;
    use snp_core::env::Env;

    /// The argv every `git commit` passes through the installed hook script (`hook-tmpl`: `hook-impl
    /// --config=… --hook-type=… --hook-dir "$HERE" -- "$@"`).
    fn hook_impl_argv() -> Vec<OsString> {
        [
            "hook-impl",
            "--config=.pre-commit-config.yaml",
            "--hook-type=pre-push",
            "--hook-dir",
            "/home/user/src/project/.git/hooks",
            "--",
            "origin",
            "git@github.com:org/project.git",
        ]
        .iter()
        .map(OsString::from)
        .collect()
    }

    /// The median of `rounds` timings of `iters` parses each, per parse.
    fn median_per_parse(brand: Brand, env: &Env, argv: &[OsString]) -> Duration {
        let iters = 2_000u32;
        let mut samples: Vec<Duration> = (0..15)
            .map(|_| {
                let start = Instant::now();
                for _ in 0..iters {
                    let out = snp_cli::parse(brand, black_box(argv), env, None);
                    assert!(matches!(out, CliOutcome::Cli(Cli::HookImpl(_))));
                    black_box(out);
                }
                start.elapsed() / iters
            })
            .collect();
        samples.sort();
        samples[samples.len() / 2]
    }

    #[test]
    #[ignore = "perf"]
    fn perf_hook_impl_parse_under_15us() {
        let env = Env::from_pairs([
            ("PATH", "/usr/local/bin:/usr/bin:/bin"),
            ("HOME", "/home/user"),
            ("GIT_DIR", "/home/user/src/project/.git"),
        ]);
        let argv = hook_impl_argv();
        for brand in [Brand::PreCommit, Brand::Snp] {
            let t = median_per_parse(brand, &env, &argv);
            assert!(
                t < Duration::from_micros(15),
                "{brand:?}: parsing the hook-impl argv took {t:?} (budget 15 µs = 3 × 5 µs)"
            );
        }
    }
}
