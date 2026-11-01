//! Criterion micro-bench: Python-`re` fast kinds vs the regex / fancy-regex engines on 100 k real-shaped paths
//! (ARCHITECTURE §12.2 T9, §12.3).
//!
//! * `pyre_kinds/<shape>/<MatchKind>`: one `files`/`exclude` shape (`''`, `^$`, prefix, suffix, contains) written
//!   three ways — the form the front end lowers to a fast kind, an equivalent form only the tier-1 DFA engine accepts,
//!   and an equivalent form needing tier 2 (look-around) — each searched over 100 k paths. The id carries the kind
//!   [`PyRegex::kind`] actually chose, so a lowering change shows up as a renamed benchmark, never as a mislabelled
//!   number.
//! * `pyre_tier1/typical_12x100k`: the twelve typical tier-1 config patterns of the WP-13 perf-smoke test over the
//!   same 100 k paths; its throughput is gated by `cargo xtask bench gate --micro` (≥ 1,000,000 searches/s, the exact
//!   WP-13 target; the perf-smoke test only catches a 3× regression).
//! * `pyre_tier1/unicode_32threads_x20`: the ADR-079 regex-cache case — 20 Unicode-class patterns searched by 32
//!   threads over non-ASCII paths (each thread owns its lazy-DFA caches).
//!
//! Owner: WP-36.
#![expect(
    clippy::expect_used,
    reason = "benchmark setup on fixed inputs: a failure is a broken bench"
)]

mod common;

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, SamplingMode, Throughput, criterion_group, criterion_main};
use snp_pyre::{Options, PyRegex};

/// The corpus size of ARCHITECTURE §12.3.
const PATHS: usize = 100_000;

/// `(shape, [fast form, tier-1 form, tier-2 form])`: the three forms match exactly the same paths.
const SHAPES: &[(&str, [&str; 3])] = &[
    ("match_all", ["", "(?:|x)", "(?=)"]),
    ("never", ["^$", "^(?:|$)$", "^(?!.)$"]),
    ("prefix", ["^docs/", "^(?:docs|docs)/", "^(?=d)docs/"]),
    ("suffix", [r"\.py$", r"\.(?:py|py)$", r"(?<=\.)py$"]),
    (
        "contains",
        [
            "migrations/",
            "(?:migrations|migrations)/",
            "(?=m)migrations/",
        ],
    ),
];

/// The typical tier-1 `files`/`exclude` patterns (the WP-13 perf-smoke set).
const TYPICAL: &[&str] = &[
    "^(docs|tests)/",
    "(^|/)migrations/",
    r"\.(js|jsx|ts|tsx)$",
    r"^src/.*\.rs$",
    r".*\.min\.(js|css)$",
    r"\.ya?ml$",
    "(^|/)node_modules/",
    r"\.(png|jpe?g|gif|ico|svg)$",
    r"^setup\.(py|cfg)$",
    "(?x)^(\n    docs/.*|\n    tests/fixtures/.*\n)$",
    "^(pre_commit|testing)/resources/",
    r"(?i)readme\.(md|rst)$",
];

/// Twenty Unicode-aware tier-1 patterns (word classes, case folding, non-ASCII literals): large lazy-DFA state
/// spaces, the ADR-079 cache-memory case.
const UNICODE_PATTERNS: &[&str] = &[
    "(?i)^données/",
    r"^\w+/\w+_\d+\.py$",
    "(?i)straße|strasse",
    r"[^\W\d_]{6,}_\d+\.",
    "^(?:日本語|中文资料)/",
    r"\b\w{3,8}\b/\w+\.md$",
    "(?i)ünïcödé",
    r"^[\w\-]+(?:/[\w\-]+){2,}\.json$",
    r"\d{3,}\.(?:ts|tsx|js)$",
    "(?i)^(?:ελληνικά|русский)/",
    "[αβγδεζηθ]",
    r"(?:\w+/){3}\w+_\d{2}\.",
    r"[\u4e00-\u9fff]{2,}",
    "(?i)caf[ée]",
    r"^\w*\d\w*/",
    "(?:^|/)測試|测试/",
    r"\W\w+_\d+\.min\.js$",
    r"(?i)naïve/.+\.ya?ml$",
    "[ऀ-ॿ]+",
    r"^\w+(?:/\w+)*/\w+_1\d*\.\w+$",
];

fn elements(n: usize) -> u64 {
    u64::try_from(n).expect("a corpus size fits u64")
}

fn compile(src: &str) -> PyRegex {
    PyRegex::compile(src, Options::CONFIG).expect("a valid bench pattern")
}

fn count_hits(re: &PyRegex, paths: &[String]) -> u64 {
    let mut hits = 0_u64;
    for p in paths {
        hits += u64::from(
            re.search(black_box(p))
                .expect("no backtrack limit on paths"),
        );
    }
    hits
}

fn kinds(c: &mut Criterion) {
    let paths = common::paths(PATHS);
    let mut g = c.benchmark_group("pyre_kinds");
    g.throughput(Throughput::Elements(elements(PATHS)));
    g.sampling_mode(SamplingMode::Flat);
    g.sample_size(20);
    g.measurement_time(Duration::from_secs(4));
    for (shape, forms) in SHAPES {
        let mut expected = None;
        for src in forms {
            let re = compile(src);
            // the three forms are the same predicate: equal hit counts or the bench is wrong
            let hits = count_hits(&re, &paths);
            assert_eq!(*expected.get_or_insert(hits), hits, "{shape}: {src:?}");
            g.bench_function(format!("{shape}/{:?}", re.kind()), |b| {
                b.iter(|| count_hits(&re, &paths));
            });
        }
    }
    g.finish();
}

fn tier1(c: &mut Criterion) {
    let paths = common::paths(PATHS);
    let typical: Vec<PyRegex> = TYPICAL.iter().map(|p| compile(p)).collect();
    let mut g = c.benchmark_group("pyre_tier1");
    g.throughput(Throughput::Elements(elements(PATHS * TYPICAL.len())));
    g.sampling_mode(SamplingMode::Flat);
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(12));
    g.bench_function("typical_12x100k", |b| {
        b.iter(|| typical.iter().map(|re| count_hits(re, &paths)).sum::<u64>());
    });

    // ADR-079: 32 threads × 20 Unicode patterns on non-ASCII paths (each thread its own caches).
    let unicode = common::unicode_paths(10_000);
    let patterns: Vec<PyRegex> = UNICODE_PATTERNS.iter().map(|p| compile(p)).collect();
    let threads = 32_usize;
    g.throughput(Throughput::Elements(elements(
        threads * unicode.len() * patterns.len(),
    )));
    g.bench_function("unicode_32threads_x20", |b| {
        b.iter(|| {
            std::thread::scope(|s| {
                #[expect(
                    clippy::needless_collect,
                    reason = "every thread must be running before the first join"
                )]
                let handles: Vec<_> = (0..threads)
                    .map(|_| {
                        std::thread::Builder::new()
                            .stack_size(256 << 10)
                            .spawn_scoped(s, || {
                                patterns
                                    .iter()
                                    .map(|re| count_hits(re, &unicode))
                                    .sum::<u64>()
                            })
                            .expect("spawning a bench thread")
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("a bench thread"))
                    .sum::<u64>()
            })
        });
    });
    g.finish();
}

criterion_group!(benches, kinds, tier1);
criterion_main!(benches);
