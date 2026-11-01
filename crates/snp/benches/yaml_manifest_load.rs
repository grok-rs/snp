//! Criterion micro-bench: PyYAML-compatible load of the pre-commit-hooks manifest (ARCHITECTURE §1.4.6, §12.3).
//!
//! * `yaml/load_pre_commit_hooks_v6_manifest`: `snp_yaml::load` (`yaml.load(…, Loader=SafeLoader)`) of the
//!   `.pre-commit-hooks.yaml` of pre-commit-hooks v6.0.0 (the embedded snp-hooks-data copy, byte-identical to the
//!   tag);
//! * `yaml/load_b3_config`: the ten-hook `noop` configuration of scenario B3 (the config load on the run's critical
//!   path, ADR-047);
//! * `yaml/compose_pre_commit_hooks_v6_manifest`: the arena composer alone (migrate-config, autoupdate);
//! * `yaml/alias_bomb_10x10_rejected`: the WP-14 acceptance input (10 × 10 `lol` aliases) — a limit error; gated at
//!   < 100 ms by `cargo xtask bench gate --micro` (the perf-smoke test only catches a 3× regression);
//! * `yaml/deep_nesting_10000_rejected`: the other half of that acceptance item (`[` × 10,000) — a clean YAML error,
//!   gated at < 100 ms the same way.
//!
//! Owner: WP-36.
#![expect(
    clippy::expect_used,
    reason = "benchmark setup on fixed inputs: a failure is a broken bench"
)]

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use snp_yaml::{Limits, Schema};

/// The manifest of pre-commit-hooks v6.0.0 (generated, verified against the tag by WP-17's drift check).
const MANIFEST: &str = include_str!("../../snp-hooks-data/src/generated/manifests/v6.0.0.yaml");

/// The B3 configuration: ten local `system` hooks running `true`.
fn b3_config() -> String {
    let mut s = String::from("repos:\n-   repo: local\n    hooks:\n");
    for i in 1..=10 {
        write!(
            s,
            "    -   id: noop-{i:02}\n        name: no-op {i:02}\n        entry: 'true'\n        \
             language: system\n        always_run: true\n        pass_filenames: false\n"
        )
        .expect("writing to a String");
    }
    s
}

/// The `lol` bomb with `levels` levels of 10 aliases each (the WP-14 perf-smoke input).
fn bomb(levels: usize) -> String {
    let mut lines = vec![format!("a0: &a0 [{}]", ["lol"; 10].join(", "))];
    for i in 1..levels {
        let refs = vec![format!("*a{}", i - 1); 10].join(", ");
        lines.push(format!("a{i}: &a{i} [{refs}]"));
    }
    lines.join("\n") + "\n"
}

/// `[` × `depth` + `]` × `depth` (the WP-14 nesting input).
fn deep(depth: usize) -> String {
    let mut s: String = std::iter::repeat_n('[', depth).collect();
    s.extend(std::iter::repeat_n(']', depth));
    s.push('\n');
    s
}

fn bench(c: &mut Criterion) {
    let lim = Limits::default();
    let config = b3_config();
    let bomb = bomb(10);
    let deep = deep(10_000);
    // the inputs behave as the benchmark names say
    assert!(snp_yaml::load(MANIFEST, &lim).is_ok());
    assert!(snp_yaml::load(&config, &lim).is_ok());
    assert!(snp_yaml::load(&bomb, &lim).is_err());
    assert!(snp_yaml::load(&deep, &lim).is_err());

    let mut g = c.benchmark_group("yaml");
    g.bench_function("load_pre_commit_hooks_v6_manifest", |b| {
        b.iter(|| snp_yaml::load(black_box(MANIFEST), &lim).expect("loads"));
    });
    g.bench_function("load_b3_config", |b| {
        b.iter(|| snp_yaml::load(black_box(&config), &lim).expect("loads"));
    });
    g.bench_function("compose_pre_commit_hooks_v6_manifest", |b| {
        b.iter(|| {
            snp_yaml::compose(black_box(MANIFEST), Schema::PyYaml11Safe, &lim).expect("composes")
        });
    });
    g.bench_function("alias_bomb_10x10_rejected", |b| {
        b.iter(|| snp_yaml::load(black_box(&bomb), &lim).is_err());
    });
    g.bench_function("deep_nesting_10000_rejected", |b| {
        b.iter(|| snp_yaml::load(black_box(&deep), &lim).is_err());
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
