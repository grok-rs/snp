//! Acceptance: a 100k-file table that only ever uses the name tier stays within 10 MB of heap (dhat-measured; the
//! name column is two bytes per file, ARCHITECTURE §2.3, ADR-079).
//!
//! One test per binary: dhat's global allocator and profiler are process-wide.
//!
//! Owner: WP-22.

#[cfg(test)]
mod memory {
    use std::path::Path;

    use snp_files::{FileId, FileTable, Needs, TagOutcome};

    #[global_allocator]
    static ALLOC: dhat::Alloc = dhat::Alloc;

    const FILES: u32 = 100_000;
    const LIMIT: usize = 10 * 1024 * 1024;

    #[test]
    fn name_tier_of_100k_files_fits_in_10_mb() {
        let _profiler = dhat::Profiler::builder().testing().build();
        let exts = ["py", "js", "md", "txt", "yaml", "rs", "json", "unknownext"];
        let mut buf = Vec::new();
        let mut spans = Vec::new();
        for i in 0..FILES {
            let start = u32::try_from(buf.len()).unwrap();
            let ext = exts[usize::try_from(i).unwrap() % exts.len()];
            buf.extend_from_slice(format!("src/pkg{:03}/module_{i:06}.{ext}", i % 500).as_bytes());
            spans.push((start, u32::try_from(buf.len()).unwrap()));
            buf.push(0);
        }
        buf.shrink_to_fit();
        spans.shrink_to_fit();
        let table = FileTable::from_zbuf(buf, spans, Path::new("/repo"));
        let mut python = 0_u32;
        for i in 0..FILES {
            let TagOutcome::Tags(tags) = table.tags(FileId(i), Needs::default()) else {
                panic!("the name tier never fails")
            };
            python += u32::from(tags.contains("python"));
        }
        assert_eq!(python, FILES / 8);
        let stats = dhat::HeapStats::get();
        assert!(
            stats.curr_bytes <= LIMIT,
            "table + name tier hold {} bytes (limit {LIMIT})",
            stats.curr_bytes
        );
        drop(table);
    }
}
