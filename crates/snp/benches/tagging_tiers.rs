//! Criterion micro-bench: tiered identify tagging (name / stat / content tiers) and the classifier
//! (ARCHITECTURE §2.3, §12.2 T8, §12.3).
//!
//! * `tagging_tiers/{name,stat,content}_tier_10k`: `FileTable::tags` of 10 k files at one tier on a fresh table
//!   (every call computes): the name tier never touches the disk, the stat tier is one `lstat`, the content tier
//!   opens files whose encoding or interpreter the name tier left open (extension-less files, executables);
//! * `tagging_tiers/prefetch_content_10k`: the same content tier filled by `FileTable::prefetch` on the pool;
//! * `classify/50k_files_10_filters`: `Classifier::new` + `for_hook` for the ten typical filters of the WP-22
//!   perf-smoke test on the same 50 k-file corpus (fresh table per iteration); gated at ≤ 40 ms by
//!   `cargo xtask bench gate --micro` (the exact WP-22 target; the perf-smoke test only catches a 3× regression).
//!
//! Owner: WP-36.
#![expect(
    clippy::expect_used,
    reason = "benchmark setup on fixed inputs: a failure is a broken bench"
)]

#[expect(
    dead_code,
    reason = "the shared corpus module also serves the Unicode benches"
)]
mod common;

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use file_identify::TagSet;
use snp_core::cancel::CancelToken;
use snp_files::{Classifier, FileId, FileTable, HookFilter, Needs, TagOutcome};
use snp_proc::pool::JobPool;
use snp_pyre::{Options, RegexCache};

const TIER_FILES: usize = 10_000;
const CLASSIFY_FILES: usize = 50_000;

/// Writes the corpus under `root`: every 7th file is executable with a shebang, every 11th has no extension
/// (content needed for `text`/`binary`), every 13th is binary.
fn materialise(root: &Path, names: &[String]) {
    for (i, name) in names.iter().enumerate() {
        let full = root.join(name);
        std::fs::create_dir_all(full.parent().expect("a relative path has a parent"))
            .expect("creating a corpus directory");
        let body: &[u8] = if i % 13 == 0 {
            b"\x00\x01\x02binary\xff"
        } else if i % 7 == 0 {
            b"#!/usr/bin/env python3\nprint('x')\n"
        } else {
            b"content\n"
        };
        std::fs::write(&full, body).expect("writing a corpus file");
        #[cfg(unix)]
        if i % 7 == 0 {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&full, std::fs::Permissions::from_mode(0o755))
                .expect("chmod +x");
        }
    }
}

/// The tier corpus: [`common::paths`] with every 11th name stripped of its extension.
fn tier_names(n: usize) -> Vec<String> {
    common::paths(n)
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            if i % 11 == 0 {
                p.rsplit_once('.')
                    .map_or(p.clone(), |(stem, _)| stem.to_owned())
            } else {
                p
            }
        })
        .collect()
}

fn pool() -> Arc<JobPool> {
    let permits = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    JobPool::new(
        NonZeroUsize::new(permits).expect("at least one permit"),
        CancelToken::new(),
    )
}

fn table(names: &[OsString], root: &Path) -> FileTable {
    FileTable::from_args(names.to_vec(), root)
}

fn tag_all(t: &FileTable, n: Needs) -> usize {
    let len = u32::try_from(t.len()).expect("a bench table fits u32");
    (0..len)
        .filter(|&i| matches!(t.tags(FileId(i), n), TagOutcome::Tags(s) if !s.is_empty()))
        .count()
}

fn tiers(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let names = tier_names(TIER_FILES);
    materialise(dir.path(), &names);
    let os: Vec<OsString> = names.iter().map(OsString::from).collect();
    let pool = pool();
    let content = Needs {
        stat: true,
        encoding: true,
        shebang: true,
    };
    let mut g = c.benchmark_group("tagging_tiers");
    g.throughput(Throughput::Elements(
        u64::try_from(TIER_FILES).expect("fits"),
    ));
    for (label, needs) in [
        ("name_tier_10k", Needs::default()),
        (
            "stat_tier_10k",
            Needs {
                stat: true,
                ..Needs::default()
            },
        ),
        ("content_tier_10k", content),
    ] {
        g.bench_function(label, |b| {
            b.iter_batched(
                || table(&os, dir.path()),
                |t| tag_all(&t, needs),
                BatchSize::LargeInput,
            );
        });
    }
    let ids: Vec<FileId> = (0..u32::try_from(TIER_FILES).expect("fits"))
        .map(FileId)
        .collect();
    g.bench_function("prefetch_content_10k", |b| {
        b.iter_batched(
            || table(&os, dir.path()),
            |t| {
                t.prefetch(&ids, content, &pool);
                t
            },
            BatchSize::LargeInput,
        );
    });
    g.finish();
}

fn tags(names: &[&str]) -> TagSet {
    TagSet::from_names(names).expect("known identify tags")
}

/// The WP-22 perf-smoke corpus: 50 k files in 100 packages, one in 50 without an extension.
fn classify_names() -> Vec<String> {
    let exts = [
        "py", "js", "md", "txt", "yaml", "json", "rs", "toml", "pyi", "cfg", "html", "sh",
    ];
    (0..CLASSIFY_FILES)
        .map(|i| {
            let dir = format!("src/pkg{:03}", i % 100);
            if i % 50 == 0 {
                format!("{dir}/NOTES{i}")
            } else {
                format!(
                    "{dir}/module_{i:05}.{}",
                    exts.get(i % exts.len()).copied().unwrap_or("txt")
                )
            }
        })
        .collect()
}

fn classify(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let names = classify_names();
    for name in &names {
        let full = dir.path().join(name);
        std::fs::create_dir_all(full.parent().expect("has a parent")).expect("mkdir");
        std::fs::write(&full, b"content\n").expect("write");
    }
    let os: Vec<OsString> = names.iter().map(OsString::from).collect();
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
        .expect("a valid filter")
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
    let pool = pool();
    let all = cache.get("", Options::CONFIG).expect("valid");
    let none = cache.get("^$", Options::CONFIG).expect("valid");
    let mut g = c.benchmark_group("classify");
    g.sample_size(20);
    g.measurement_time(Duration::from_secs(8));
    g.throughput(Throughput::Elements(
        u64::try_from(CLASSIFY_FILES).expect("fits"),
    ));
    g.bench_function("50k_files_10_filters", |b| {
        b.iter_batched(
            || Arc::new(table(&os, dir.path())),
            |t| {
                let c = Classifier::new(t, &all, &none, &pool).expect("classifier");
                filters
                    .iter()
                    .map(|f| c.for_hook(f, &pool).expect("for_hook").len())
                    .sum::<usize>()
            },
            BatchSize::LargeInput,
        );
    });
    g.finish();
}

criterion_group!(benches, tiers, classify);
criterion_main!(benches);
