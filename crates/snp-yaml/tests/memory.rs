//! The memory half of the alias-bomb acceptance item: the 10 × 10 `lol` bomb is rejected with a peak heap below
//! 64 MB (D-SEC-11). Measured with dhat's counting allocator, so this test binary holds exactly one test.
//!
//! Owner: WP-14.

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[cfg(test)]
mod memory {
    use snp_yaml::Limits;

    /// `s` repeated `n` times (`str::repeat` is a disallowed method).
    fn rep(s: &str, n: usize) -> String {
        std::iter::repeat_n(s, n).collect()
    }

    #[test]
    fn alias_bomb_peak_heap_is_below_64_mb() {
        let mut lines = vec![format!("a0: &a0 [{}]", ["lol"; 10].join(", "))];
        for i in 1..10 {
            let refs = vec![format!("*a{}", i - 1); 10].join(", ");
            lines.push(format!("a{i}: &a{i} [{refs}]"));
        }
        let text = lines.join("\n") + "\n";
        let deep = format!("{}{}\n", rep("[", 10_000), rep("]", 10_000));

        let _profiler = dhat::Profiler::builder().testing().build();
        assert!(snp_yaml::load(&text, &Limits::default()).is_err());
        assert!(snp_yaml::load(&deep, &Limits::default()).is_err());
        let stats = dhat::HeapStats::get();
        assert!(
            stats.max_bytes < 64 << 20,
            "peak heap {} bytes",
            stats.max_bytes
        );
    }
}
