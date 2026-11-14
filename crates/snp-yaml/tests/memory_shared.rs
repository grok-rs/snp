//! The memory gate of a document inside every limit whose shared collection nests 500 levels deep (D-SEC-11,
//! ADR-112): it loads with a peak heap linear in its size, never a copy per nesting level. Measured with dhat's
//! counting allocator, so this test binary holds exactly one test.
//!
//! Owner: WP-14.

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[cfg(test)]
mod memory_shared {
    use snp_yaml::Limits;

    /// `s` repeated `n` times (`str::repeat` is a disallowed method).
    fn rep(s: &str, n: usize) -> String {
        std::iter::repeat_n(s, n).collect()
    }

    /// `repos: &s [[…[x0, …, x3999]…]]` (500 levels) and `x: *s`: about 9,000 values in the loaded tree. A finished
    /// copy kept for every collection below the shared one would hold about 500 × 4,000 values.
    #[test]
    fn nested_shared_collection_peak_heap_is_below_16_mb() {
        let leaves: Vec<String> = (0..4_000).map(|i| format!("x{i}")).collect();
        let text = format!(
            "repos: &s {}[{}]{}\nx: *s\n",
            rep("[", 499),
            leaves.join(", "),
            rep("]", 499)
        );

        let _profiler = dhat::Profiler::builder().testing().build();
        let value = snp_yaml::load(&text, &Limits::default()).expect("inside every limit");
        drop(value);
        let stats = dhat::HeapStats::get();
        assert!(
            stats.max_bytes < 16 << 20,
            "peak heap {} bytes",
            stats.max_bytes
        );
    }
}
