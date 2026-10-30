//! D-SEC-10 / 01 Addendum §A.6.13 D7, D8: the relaxed prefilter answers before the backtracking engine runs,
//! and shapes that are exponential in CPython stay linear in tier 1. (The 10⁸-step backtrack limit itself is
//! exercised by `tests/perf.rs::perf_catastrophic_tier2_pattern_hits_the_backtrack_limit`: unoptimised builds
//! need ~20 s to take 10⁸ steps.)
//!
//! Owner: WP-13.

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "test inputs built with fixed, non-zero repeat counts and steps"
)]
mod backtrack {
    use std::time::{Duration, Instant};

    use snp_pyre::{MatchKind, Mode, Options, PyRegex};

    /// See `tests/perf.rs`.
    const CATASTROPHIC: &str = "(?:(?=a)a+)+(?<!a)b";

    #[test]
    fn the_prefilter_answers_without_backtracking() {
        // No `b`: the relaxed prefilter `(?:a+)+b` rejects the haystack before the backtracking engine runs.
        for mode in [Mode::Str, Mode::Bytes] {
            let re = PyRegex::compile(
                CATASTROPHIC,
                Options {
                    mode,
                    ..Options::CONFIG
                },
            )
            .unwrap();
            assert_eq!(re.kind(), MatchKind::Tier2);
            let t = Instant::now();
            assert_eq!(re.search(&"a".repeat(10_000)), Ok(false));
            assert_eq!(re.search_bytes("a".repeat(10_000).as_bytes()), Ok(None));
            assert!(
                t.elapsed() < Duration::from_secs(5),
                "took {:?}",
                t.elapsed()
            );
        }
    }

    #[test]
    fn exponential_in_cpython_is_linear_in_tier1() {
        // D8: `^(.+/)*x$` is exponential in CPython on deep paths without a match; tier 1 answers in linear time.
        let re = PyRegex::compile(r"^(.+/)*\.pre-commit-config\.yaml$", Options::CONFIG).unwrap();
        assert_eq!(re.kind(), MatchKind::Tier1);
        let deep = format!("{}x", "d/".repeat(5_000));
        let t = Instant::now();
        assert_eq!(re.search(&deep), Ok(false));
        assert_eq!(
            re.search(&format!("{deep}/.pre-commit-config.yaml")),
            Ok(true)
        );
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "took {:?}",
            t.elapsed()
        );
    }
}
