//! Criterion micro-bench: MT19937 shuffle + xargs partition of 50 k file names (ARCHITECTURE §12.2 T7, §12.3).
//!
//! * `shuffle_partition/shuffle_50k`: `lang_base._shuffled` — `precommit_shuffled_indices(50_000)` (a fresh
//!   `random.Random(1542676187)` per hook) applied to `u32` file ids;
//! * `shuffle_partition/partition_50k`: `xargs.partition` of the shuffled names for `jobs = 8` within the POSIX
//!   128 KiB budget;
//! * `shuffle_partition/shuffle_and_partition_50k`: both, the per-hook cost the executor pays.
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

use std::ffi::{OsStr, OsString};
use std::hint::black_box;
use std::num::NonZeroUsize;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use snp_proc::xargs;
use snp_pycompat::random::precommit_shuffled_indices;

const FILES: usize = 50_000;
/// `_get_platform_max_length` on a typical Linux host (clamped to 128 KiB).
const MAX_LEN: usize = 131_072;

fn shuffled_ids(n: usize) -> Vec<u32> {
    precommit_shuffled_indices(n)
}

/// The name of file id `i`.
fn name(names: &[OsString], i: u32) -> &OsStr {
    names
        .get(usize::try_from(i).expect("u32 ids fit usize"))
        .expect("a permutation of 0..n")
}

fn bench(c: &mut Criterion) {
    let names: Vec<OsString> = common::paths(FILES)
        .into_iter()
        .map(OsString::from)
        .collect();
    let cmd: Vec<OsString> = ["/usr/bin/true", "--quiet"].map(OsString::from).to_vec();
    let jobs = NonZeroUsize::new(8).expect("8 > 0");
    let order = shuffled_ids(FILES);
    let shuffled: Vec<&OsStr> = order.iter().map(|&i| name(&names, i)).collect();
    // sanity: the permutation keeps every file exactly once and the partitions cover them all
    let ranges = xargs::partition(&cmd, &shuffled, jobs, MAX_LEN).expect("short names fit");
    assert_eq!(
        ranges.iter().map(ExactSizeIterator::len).sum::<usize>(),
        FILES
    );

    let mut g = c.benchmark_group("shuffle_partition");
    g.throughput(Throughput::Elements(u64::try_from(FILES).expect("fits")));
    g.bench_function("shuffle_50k", |b| {
        b.iter(|| shuffled_ids(black_box(FILES)));
    });
    g.bench_function("partition_50k", |b| {
        b.iter(|| xargs::partition(&cmd, black_box(&shuffled), jobs, MAX_LEN).expect("fits"));
    });
    g.bench_function("shuffle_and_partition_50k", |b| {
        b.iter(|| {
            let order = shuffled_ids(black_box(FILES));
            let files: Vec<&OsStr> = order.iter().map(|&i| name(&names, i)).collect();
            xargs::partition(&cmd, &files, jobs, MAX_LEN).expect("fits")
        });
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
