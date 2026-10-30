//! Timing thresholds of the WP-14 acceptance list as release-mode tests with a 3× margin (ADR-071; run by the
//! `perf-smoke` CI job with `cargo test --profile perf -- --ignored`; WP-36's bench gate enforces the exact
//! targets).
//!
//! Owner: WP-14.

#[cfg(test)]
mod perf {
    use std::time::{Duration, Instant};

    use snp_yaml::Limits;

    /// `s` repeated `n` times (`str::repeat` is a disallowed method).
    fn rep(s: &str, n: usize) -> String {
        std::iter::repeat_n(s, n).collect()
    }

    /// The `lol` bomb with `levels` levels of 10 aliases each.
    fn bomb(levels: usize) -> String {
        let mut lines = vec![format!("a0: &a0 [{}]", ["lol"; 10].join(", "))];
        for i in 1..levels {
            let refs = vec![format!("*a{}", i - 1); 10].join(", ");
            lines.push(format!("a{i}: &a{i} [{refs}]"));
        }
        lines.join("\n") + "\n"
    }

    /// Target: the 10 × 10 alias bomb is a limit error in < 100 ms (3× margin: 300 ms).
    #[test]
    #[ignore = "perf"]
    fn alias_bomb_is_rejected_within_300_ms() {
        let text = bomb(10);
        let t = Instant::now();
        let r = snp_yaml::load(&text, &Limits::default());
        let elapsed = t.elapsed();
        assert!(r.is_err());
        assert!(elapsed < Duration::from_millis(300), "{elapsed:?}");
    }

    /// The 10,000-deep document of the same acceptance item: a limit error well within the same budget.
    #[test]
    #[ignore = "perf"]
    fn deep_nesting_is_rejected_within_300_ms() {
        let text = format!("{}{}\n", rep("[", 10_000), rep("]", 10_000));
        let t = Instant::now();
        let r = snp_yaml::load(&text, &Limits::default());
        let elapsed = t.elapsed();
        assert!(r.is_err());
        assert!(elapsed < Duration::from_millis(300), "{elapsed:?}");
    }
}
