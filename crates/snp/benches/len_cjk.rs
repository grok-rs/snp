//! Criterion micro-bench: `len_cjk` column width of hook names (ARCHITECTURE §12.3).
//!
//! `output._compute_cols` sums `len_cjk(hook.name)` over every selected hook on each run; the ASCII fast path is
//! what nearly every configuration takes, the per-character East-Asian-width lookup is the fallback.
//!
//! * `len_cjk/ascii_100_names`: 100 typical ASCII hook names;
//! * `len_cjk/cjk_100_names`: 100 names mixing CJK, accented Latin, Hangul and emoji;
//! * `len_cjk/long_cjk_4k_chars`: one 4,096-character CJK string (worst case per call).
//!
//! Owner: WP-36.

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use snp_pycompat::eaw::len_cjk;

const ASCII: &[&str] = &[
    "trim trailing whitespace",
    "fix end of files",
    "check yaml",
    "check for added large files",
    "black",
    "flake8",
    "mypy",
    "ruff",
    "ruff-format",
    "check json",
];

const CJK: &[&str] = &[
    "检查中文名称の長いフック",
    "末尾の空白を削除",
    "проверка yaml",
    "vérifier les fichiers ajoutés",
    "한국어 훅 이름",
    "✨ format 🚀",
    "全角ＡＢＣ",
    "混合 mixed 名前",
    "Ελληνικά έλεγχος",
    "检查",
];

fn widths(names: &[&str]) -> usize {
    names.iter().map(|n| len_cjk(black_box(n))).sum()
}

fn bench(c: &mut Criterion) {
    let ascii: Vec<&str> = ASCII.iter().copied().cycle().take(100).collect();
    let cjk: Vec<&str> = CJK.iter().copied().cycle().take(100).collect();
    let long: String = "检查中文名称の長いフック"
        .chars()
        .cycle()
        .take(4096)
        .collect();
    assert_eq!(len_cjk("检查中文名称の長いフック"), 24);

    let mut g = c.benchmark_group("len_cjk");
    g.throughput(Throughput::Elements(100));
    g.bench_function("ascii_100_names", |b| b.iter(|| widths(&ascii)));
    g.bench_function("cjk_100_names", |b| b.iter(|| widths(&cjk)));
    g.throughput(Throughput::Bytes(
        u64::try_from(long.len()).unwrap_or(u64::MAX),
    ));
    g.bench_function("long_cjk_4k_chars", |b| {
        b.iter(|| len_cjk(black_box(&long)));
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
