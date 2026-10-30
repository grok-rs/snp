//! perf-smoke (release, `#[ignore = "perf"]`, run by CI's `perf-smoke` job): classification of 50k files with 10
//! typical filters within 120 ms — 3× the 40 ms target that WP-36's bench gate enforces (ARCHITECTURE §12.1).
//!
//! `cargo test -p snp-files --release --test perf -- --ignored`
//!
//! Owner: WP-22.

#[cfg(test)]
mod perf {
    use std::ffi::OsString;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use file_identify::TagSet;
    use snp_core::cancel::CancelToken;
    use snp_files::{Classifier, FileTable, HookFilter};
    use snp_proc::pool::JobPool;
    use snp_pyre::{Options, RegexCache};

    const FILES: usize = 50_000;
    const BUDGET: Duration = Duration::from_millis(120);

    fn tags(names: &[&str]) -> TagSet {
        TagSet::from_names(names).unwrap()
    }

    #[test]
    #[ignore = "perf"]
    fn perf_classify_50k_files_with_10_typical_filters() {
        let d = tempfile::tempdir().unwrap();
        let exts = [
            "py", "js", "md", "txt", "yaml", "json", "rs", "toml", "pyi", "cfg", "html", "sh",
        ];
        let mut names = Vec::new();
        for i in 0..FILES {
            let dir = format!("src/pkg{:03}", i % 100);
            let name = if i % 50 == 0 {
                format!("{dir}/NOTES{i}")
            } else {
                format!("{dir}/module_{i:05}.{}", exts[i % exts.len()])
            };
            let full = d.path().join(&name);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, b"content\n").unwrap();
            names.push(OsString::from(name));
        }
        let cache = RegexCache::new();
        let f = |files: &str, types: &[&str], types_or: &[&str]| {
            HookFilter::compile(
                files,
                "^$",
                tags(types),
                tags(types_or),
                TagSet::EMPTY,
                None,
                &cache,
            )
            .unwrap()
        };
        let filters = [
            f("", &["text"], &[]),                // trailing-whitespace
            f("", &["text"], &[]),                // end-of-file-fixer
            f("", &["yaml"], &[]),                // check-yaml
            f("", &["json"], &[]),                // check-json
            f("", &["file"], &[]),                // check-added-large-files
            f("", &["text"], &[]),                // check-merge-conflict
            f("", &["file"], &["python", "pyi"]), // black
            f(r"\.py$", &["python"], &[]),        // flake8
            f("", &["file"], &["javascript", "markdown", "json", "yaml"]), // prettier
            f(r"^src/.*\.py$", &["file"], &[]),   // a local hook
        ];
        let permits = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
        let pool = JobPool::new(
            std::num::NonZeroUsize::new(permits).unwrap(),
            CancelToken::new(),
        );
        let all = cache.get("", Options::CONFIG).unwrap();
        let none = cache.get("^$", Options::CONFIG).unwrap();
        // Best of three fresh tables (the machine is shared; WP-36's bench gate measures properly).
        let mut best = Duration::MAX;
        for _ in 0..3 {
            let table = Arc::new(FileTable::from_args(names.clone(), d.path()));
            let start = Instant::now();
            let c = Classifier::new(table, &all, &none, &pool).unwrap();
            let mut total = 0;
            for filter in &filters {
                total += c.for_hook(filter, &pool).unwrap().len();
            }
            let took = start.elapsed();
            assert!(total > FILES);
            best = best.min(took);
        }
        eprintln!("classify 50k files x 10 filters: best {best:?}");
        assert!(
            best <= BUDGET,
            "classification took {best:?} (budget {BUDGET:?})"
        );
    }
}
