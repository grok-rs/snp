//! Release-mode threshold tests (`perf-smoke` CI job: `--cargo-profile perf --run-ignored only -E
//! 'test(/perf_/)'`), with the 3× margin of ADR-071; WP-36's bench gate enforces the exact targets.
//!
//! * tier-1 path search throughput ≥ 330,000 searches/s per core on typical config patterns (target 1,000,000);
//! * the lazy-DFA cache of a large-state pattern stays within 2 MiB per thread (ADR-079);
//! * a catastrophic tier-2 pattern returns `MatchError::Backtrack` in < 10 s (D-SEC-10);
//! * parsing is linear in the pattern length.
//!
//! Owner: WP-13.

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "test inputs built with fixed, non-zero repeat counts and steps"
)]
mod perf {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use snp_pyre::{MatchError, MatchKind, Mode, Options, PyRegex};

    fn real_paths() -> Vec<String> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/haystacks.json");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        v["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_owned())
            .collect()
    }

    /// Tier-1 `files`/`exclude` patterns of the shapes the 01 Addendum corpus is made of.
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

    #[test]
    #[ignore = "perf"]
    fn perf_tier1_path_search_throughput() {
        let paths = real_paths();
        let patterns: Vec<PyRegex> = TYPICAL
            .iter()
            .map(|p| PyRegex::compile(p, Options::CONFIG).unwrap())
            .collect();
        for (p, re) in TYPICAL.iter().zip(&patterns) {
            assert_eq!(re.kind(), MatchKind::Tier1, "{p}");
        }
        let mut searches = 0_u64;
        let mut hits = 0_u64;
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(2) {
            for re in &patterns {
                for h in &paths {
                    hits += u64::from(re.search(h).unwrap());
                    searches += 1;
                }
            }
        }
        let rate = f64::from(u32::try_from(searches).unwrap()) / t.elapsed().as_secs_f64();
        eprintln!("tier-1 path searches: {rate:.0}/s ({searches} searches, {hits} hits)");
        assert!(
            rate >= 330_000.0,
            "{rate:.0} searches/s < 330,000 (target 1,000,000)"
        );
    }

    /// The process's resident set (Linux).
    #[cfg(target_os = "linux")]
    fn rss_bytes() -> u64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let line = status.lines().find(|l| l.starts_with("VmRSS:")).unwrap();
        let kb: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        kb * 1024
    }

    #[test]
    #[ignore = "perf"]
    #[cfg(target_os = "linux")]
    fn perf_dfa_cache_bounded_per_thread() {
        // Unicode word classes over non-ASCII text: a large lazy-DFA state space. The cache is capped by
        // `dfa_size_limit` (the crate default 2 MiB) per regex per thread; a full cache is cleared and refilled.
        let re =
            PyRegex::compile(r"(?:\w+\s+){3,6}(?:[^\W\d]\w*-){2}\d{2,}", Options::CONFIG).unwrap();
        assert_eq!(re.kind(), MatchKind::Tier1);
        let alphabet: Vec<char> = "aéßΣσκжЖ中文٣३ 1-_\u{2028}\u{a0}xyz".chars().collect();
        let haystack = |seed: usize| -> String {
            (0..400)
                .map(|i| alphabet[(i * 7 + seed * 13 + (i * i + seed) % 11) % alphabet.len()])
                .collect()
        };
        // Warm up allocator and pools once, then measure.
        assert!(re.search(&haystack(0)).is_ok());
        for threads in [1_usize, 8] {
            let before = rss_bytes();
            std::thread::scope(|s| {
                let handles: Vec<_> = (0..threads)
                    .map(|t| {
                        let re = &re;
                        std::thread::Builder::new()
                            .stack_size(256 << 10)
                            .spawn_scoped(s, move || {
                                for i in 0..3_000 {
                                    assert!(re.search(&haystack(i * 31 + t)).is_ok());
                                }
                            })
                            .unwrap()
                    })
                    .collect();
                for h in handles {
                    h.join().unwrap();
                }
            });
            let grown = rss_bytes().saturating_sub(before);
            let bound = u64::try_from(threads).unwrap() * (2 << 20) + (6 << 20);
            eprintln!(
                "{threads} threads: RSS grew {} KiB (bound {} KiB)",
                grown / 1024,
                bound / 1024
            );
            assert!(
                grown <= bound,
                "{threads} threads grew RSS by {grown} bytes"
            );
        }
    }

    #[test]
    #[ignore = "perf"]
    fn perf_catastrophic_tier2_pattern_hits_the_backtrack_limit() {
        // `(?:(?=a)a+)+` explores every partition of the run of `a`s before the look-behind fails (the look-ahead
        // keeps the loop inside the backtracking VM instead of a linear delegate); the relaxed prefilter
        // `(?:a+)+b` matches, so the backtracking engine must run.
        let haystack = format!("{}b", "a".repeat(40));
        for mode in [Mode::Str, Mode::Bytes] {
            let re = PyRegex::compile(
                "(?:(?=a)a+)+(?<!a)b",
                Options {
                    mode,
                    ..Options::CONFIG
                },
            )
            .unwrap();
            assert_eq!(re.kind(), MatchKind::Tier2);
            let t = Instant::now();
            assert_eq!(re.search(&haystack), Err(MatchError::Backtrack));
            let search = t.elapsed();
            let t = Instant::now();
            assert_eq!(
                re.search_bytes(haystack.as_bytes()),
                Err(MatchError::Backtrack)
            );
            let spans = t.elapsed();
            eprintln!(
                "{mode:?}: search stopped at the backtrack limit in {search:?}, search_bytes in {spans:?}"
            );
            // The acceptance bound of WBS WP-13, per search (measured ~3.3 s, i.e. a 3× margin).
            for elapsed in [search, spans] {
                assert!(
                    elapsed < Duration::from_secs(10),
                    "{elapsed:?} for one search (acceptance: < 10 s)"
                );
            }
        }
    }

    #[test]
    #[ignore = "perf"]
    fn perf_parse_is_linear() {
        type Shape = (&'static str, fn(usize) -> String);
        let shapes: [Shape; 6] = [
            ("literals", |n| "a".repeat(n)),
            ("groups", |n| "(?:a)".repeat(n)),
            ("classes", |n| "[a-c\\d]".repeat(n)),
            ("alternation", |n| "ab|".repeat(n)),
            ("quantified", |n| "(a)*".repeat(n)),
            ("verbose", |n| format!("(?x){}", "a # c\n".repeat(n))),
        ];
        for (name, make) in shapes {
            let time = |n: usize| {
                let p = make(n);
                (0..5)
                    .map(|_| {
                        let t = Instant::now();
                        assert!(snp_pyre::validate(&p, Mode::Str).is_ok());
                        t.elapsed()
                    })
                    .min()
                    .unwrap()
            };
            let small = time(2_000);
            let large = time(16_000);
            let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
            eprintln!("{name}: 2,000 → {small:?}, 16,000 → {large:?} (×{ratio:.1})");
            assert!(
                ratio < 24.0,
                "{name}: parsing 8× the input took ×{ratio:.1} (linear: ×8, 3× margin)"
            );
        }
    }
}
