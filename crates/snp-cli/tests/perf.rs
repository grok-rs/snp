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

    /// The slowest of `rounds` parses of `argv` in both brands (the parse is deterministic, so the outcome only has
    /// to be produced; its kind is checked by `check`).
    fn worst_parse(argv: &[OsString], check: fn(&CliOutcome) -> bool) -> Duration {
        let env = Env::from_pairs::<&str, &str>([]);
        let mut worst = Duration::ZERO;
        for brand in [Brand::PreCommit, Brand::Snp] {
            for _ in 0..3 {
                let start = Instant::now();
                let out = snp_cli::parse(brand, black_box(argv), &env, None);
                let t = start.elapsed();
                assert!(check(&out), "{brand:?}: unexpected outcome kind");
                black_box(out);
                worst = worst.max(t);
            }
        }
        worst
    }

    /// A named argv and the outcome kind its parse must produce.
    type Case = (&'static str, Vec<OsString>, fn(&CliOutcome) -> bool);

    fn os(parts: impl IntoIterator<Item = String>) -> Vec<OsString> {
        parts.into_iter().map(OsString::from).collect()
    }

    /// `parse` is documented as linear in `argv`: large repetitive argvs (repeated `append` options, a huge short
    /// cluster, unknown options, interleaved positionals, many `--files`) must stay far from quadratic. CPython 3.13
    /// takes about 0.5 s for the first two shapes; a quadratic regression takes seconds to minutes.
    #[test]
    #[ignore = "perf"]
    fn perf_large_argvs_parse_linearly() {
        let budget = Duration::from_millis(150);
        let n = 20_000;
        let cases: Vec<Case> = vec![
            (
                "install + 20k `-t pre-push`",
                os(std::iter::once("install".to_owned())
                    .chain((0..n).flat_map(|_| ["-t".to_owned(), "pre-push".to_owned()]))),
                |o| matches!(o, CliOutcome::Cli(Cli::Install(_))),
            ),
            (
                "autoupdate + 20k `--repo x`",
                os(std::iter::once("autoupdate".to_owned())
                    .chain((0..n).flat_map(|_| ["--repo".to_owned(), "x".to_owned()]))),
                |o| matches!(o, CliOutcome::Cli(Cli::Autoupdate(_))),
            ),
            (
                "run -<100k v>",
                os([
                    "run".to_owned(),
                    std::iter::once('-')
                        .chain(std::iter::repeat_n('v', 100_000))
                        .collect(),
                ]),
                |o| matches!(o, CliOutcome::Cli(Cli::Run(_))),
            ),
            (
                "run + 20k `x -v`",
                os(std::iter::once("run".to_owned())
                    .chain((0..n).flat_map(|_| ["x".to_owned(), "-v".to_owned()]))),
                |o| matches!(o, CliOutcome::Usage(_)),
            ),
            (
                "run + 20k `--unknown`",
                os(std::iter::once("run".to_owned())
                    .chain((0..n).map(|i| format!("--unknown{i}")))),
                |o| matches!(o, CliOutcome::Usage(_)),
            ),
            (
                "run --files + 100k paths",
                os(["run".to_owned(), "--files".to_owned()]
                    .into_iter()
                    .chain((0..100_000).map(|i| format!("src/file{i}.rs")))),
                |o| matches!(o, CliOutcome::Cli(Cli::Run(_))),
            ),
        ];
        for (name, argv, check) in &cases {
            let t = worst_parse(argv, *check);
            eprintln!("{name}: {t:?}");
            assert!(t < budget, "{name}: parse took {t:?} (budget {budget:?})");
        }
    }
}
