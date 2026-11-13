//! D-SEC-10 / 01 Addendum §A.6.13 D7, D8: the relaxed prefilter answers before the backtracking engine runs
//! (tier 2, and the tier-1 exact fallback, ADR-097), and shapes that are exponential in CPython stay linear in
//! tier 1. (The 10⁸-step backtrack limit itself is
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

    #[test]
    fn the_tier1_exact_fallback_is_prefiltered() {
        // ADR-097: a str `\b` sends non-ASCII haystacks to the backtracking exact fallback, where these shapes
        // took 2.7-4.7 s per haystack and ended in `MatchError::Backtrack`. The relaxed prefilter rejects them
        // first: no match, CPython's answer.
        let cases = [
            (
                r"^(.+/)*\bconfig\.yaml$",
                format!("{}x", "\u{e9}/".repeat(24)),
            ),
            (r"\b(\w+\s?)*\b\.py", format!("{}\u{e9}!", "a".repeat(30))),
        ];
        for (pattern, hay) in &cases {
            let re = PyRegex::compile(pattern, Options::CONFIG).unwrap();
            assert_eq!(re.kind(), MatchKind::Tier1, "{pattern}");
            let t = Instant::now();
            assert_eq!(re.search(hay), Ok(false), "{pattern}");
            assert_eq!(re.match_prefix(hay), Ok(false), "{pattern}");
            assert_eq!(re.fullmatch(hay), Ok(false), "{pattern}");
            assert_eq!(re.search_bytes(hay.as_bytes()), Ok(None), "{pattern}");
            assert!(
                t.elapsed() < Duration::from_secs(1),
                "{pattern}: took {:?}",
                t.elapsed()
            );
        }
        // A haystack the prefilter admits still gets the exact answer.
        let re = PyRegex::compile(r"^(.+/)*\bconfig\.yaml$", Options::CONFIG).unwrap();
        assert_eq!(re.search("\u{e9}/config.yaml"), Ok(true));
        assert_eq!(re.search("\u{e9}config.yaml"), Ok(false));
    }
}
