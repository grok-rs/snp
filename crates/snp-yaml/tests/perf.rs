//! Timing thresholds of the WP-14 acceptance list and the D-SEC-11 hostile inputs as release-mode tests with the
//! 3x margin of the definition of done (ADR-071): `#[ignore = "perf"]` tests named `perf_*`, the filter of the `perf-smoke` CI job
//! (`--cargo-profile perf --run-ignored only -E 'test(/perf_/)'`); WP-36's bench gate enforces the exact targets.
//! The alias bomb and the deep nesting measure about 1 ms against their 100 ms target.
//!
//! Owner: WP-14.

#[cfg(test)]
mod perf {
    use std::fmt::Write as _;
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

    /// The definition of done's 3x margin over a timing target (WP-36's bench gate enforces the targets themselves).
    fn budget(target_ms: u64) -> Duration {
        Duration::from_millis(target_ms.saturating_mul(3))
    }

    /// Runs `load` and checks its outcome and time.
    fn timed(text: &str, ok: bool, ms: u64) {
        let t = Instant::now();
        let r = snp_yaml::load(text, &Limits::default());
        let elapsed = t.elapsed();
        assert_eq!(r.is_ok(), ok, "{:.200?}", r.map(|_| ()));
        assert!(elapsed < budget(ms), "{elapsed:?}");
    }

    /// Acceptance: the 10 × 10 alias bomb is a limit error in < 100 ms.
    #[test]
    #[ignore = "perf"]
    fn perf_alias_bomb_is_rejected_within_100_ms() {
        timed(&bomb(10), false, 100);
    }

    /// The 10,000-deep document of the same acceptance item: a limit error within the same budget.
    #[test]
    #[ignore = "perf"]
    fn perf_deep_nesting_is_rejected_within_100_ms() {
        timed(
            &format!("{}{}\n", rep("[", 10_000), rep("]", 10_000)),
            false,
            100,
        );
    }

    /// A merge bomb of empty mappings (D-SEC-11): 20,000 mappings each merging a list of 20,000 `{}` copy
    /// nothing, but every nested flatten is charged, so it is a limit error, not 4·10⁸ steps (33 s before).
    #[test]
    #[ignore = "perf"]
    fn perf_empty_merge_bomb_is_rejected_within_100_ms() {
        let text = format!("l: &L [{}]\n", vec!["{}"; 20_000].join(", "))
            + &(0..20_000).fold(String::new(), |mut out, i| {
                writeln!(out, "m{i}: {{<<: *L}}").unwrap();
                out
            });
        timed(&text, false, 100);
    }

    /// 300,000 `<<` keys in one mapping load in linear time (deletion is O(1); 34.6 s before, PyYAML 13 s).
    #[test]
    #[ignore = "perf"]
    fn perf_many_merge_keys_load_within_500_ms() {
        timed(
            &format!("{{{}}}\n", vec!["<<: {}"; 300_000].join(", ")),
            true,
            500,
        );
        timed(
            &format!("a: &a {{}}\nb:\n{}", rep("  <<: *a\n", 400_000)),
            true,
            1000,
        );
    }

    /// Hex, octal and binary ints have no digit limit in CPython: a 4 MiB literal parses in linear time.
    #[test]
    #[ignore = "perf"]
    fn perf_huge_power_of_two_ints_parse_within_100_ms() {
        for (prefix, digit) in [("0x", "f"), ("0", "7"), ("0b", "1")] {
            timed(&format!("a: {prefix}{}\n", rep(digit, 4 << 20)), true, 100);
        }
    }

    /// A sexagesimal `!!int` with 400,000 parts (quadratic in CPython as well) is charged to the node budget.
    #[test]
    #[ignore = "perf"]
    fn perf_huge_sexagesimal_int_is_rejected_within_100_ms() {
        let parts = vec!["1"; 400_000].join(":");
        timed(&format!("a: !!int \"{parts}\"\n"), false, 100);
    }
}
