//! Criterion micro-bench: argparse parse of a `hook-impl` argv (ARCHITECTURE §12.2 T1, §12.3).
//!
//! Every `git commit` through an installed hook parses `hook-impl --config=… --hook-type=… --hook-dir "$HERE" -- "$@"`
//! before anything else happens, so this parse is on the B9 critical path.
//!
//! * `argparse/hook_impl_cli_{compat,snp}`: the full `snp_cli::parse` (spec tree, namespace, typed facade) in both
//!   brands; gated at < 5 µs by `cargo xtask bench gate --micro` (the exact 12 §A.17 item 8 target of WP-12; the
//!   perf-smoke test only catches a 3× regression);
//! * `argparse/hook_impl_engine_compat`: the `snp_argparse::parse` engine alone on the pre-commit spec;
//! * `argparse/run_all_files_cli_compat`: `run --all-files --show-diff-on-failure` for comparison.
//!
//! Owner: WP-36.

use std::ffi::OsString;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use snp_argparse::{Outcome, ParseCfg};
use snp_cli::{Cli, CliOutcome, PRE_COMMIT_SPEC};
use snp_core::brand::Brand;
use snp_core::env::Env;

/// The argv the pre-push hook script passes (`hook-tmpl`).
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
    .map(OsString::from)
    .to_vec()
}

fn bench(c: &mut Criterion) {
    let env = Env::from_pairs([
        ("PATH", "/usr/local/bin:/usr/bin:/bin"),
        ("HOME", "/home/user"),
        ("GIT_DIR", "/home/user/src/project/.git"),
    ]);
    let argv = hook_impl_argv();
    let run_argv: Vec<OsString> = ["run", "--all-files", "--show-diff-on-failure"]
        .map(OsString::from)
        .to_vec();
    for brand in [Brand::PreCommit, Brand::Snp] {
        assert!(matches!(
            snp_cli::parse(brand, &argv, &env, None),
            CliOutcome::Cli(Cli::HookImpl(_))
        ));
    }
    let cfg = ParseCfg {
        prog: "pre-commit",
        env: &env,
        columns: 80,
    };
    assert!(matches!(
        snp_argparse::parse(PRE_COMMIT_SPEC, &argv, &cfg),
        Outcome::Parsed(_)
    ));

    let mut g = c.benchmark_group("argparse");
    for (label, brand) in [("compat", Brand::PreCommit), ("snp", Brand::Snp)] {
        g.bench_function(format!("hook_impl_cli_{label}"), |b| {
            b.iter(|| snp_cli::parse(brand, black_box(&argv), &env, None));
        });
    }
    g.bench_function("hook_impl_engine_compat", |b| {
        b.iter(|| snp_argparse::parse(PRE_COMMIT_SPEC, black_box(&argv), &cfg));
    });
    g.bench_function("run_all_files_cli_compat", |b| {
        b.iter(|| snp_cli::parse(Brand::PreCommit, black_box(&run_argv), &env, None));
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
