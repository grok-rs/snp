//! Micro-benchmarks without an external harness:
//! `cargo bench --bench identify`.
//!
//! Prints the median time per operation over 15 samples. Run by
//! `cargo test --benches` / `--all-targets` (without `--bench`), every case
//! is executed once as a smoke test and nothing is timed.
#![expect(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::indexing_slicing,
    reason = "a harness-less bench: set-up on fixed inputs fails loudly, results go to stdout, and the median index is in bounds of the 15 samples"
)]

use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use file_identify::{
    ExecPolicy, Meta, Tag, TagSet, classify, parse_shebang_bytes, tags_from_filename,
    tags_from_interpreter, tags_from_meta, tags_from_metadata, tags_from_path,
};

const NAMES: &[&str] = &[
    "src/lib.rs",
    "README.md",
    "Makefile",
    "Dockerfile.dev",
    "setup.cfg",
    "x.unknownext",
    "pre_commit/commands/run.py",
    ".pre-commit-config.yaml",
    "image.PNG",
    "noext",
];

/// Whether to measure: `cargo bench` passes `--bench` to a harness-less
/// bench target, `cargo test` does not.
fn measuring() -> bool {
    static MEASURE: OnceLock<bool> = OnceLock::new();
    *MEASURE.get_or_init(|| std::env::args().any(|arg| arg == "--bench"))
}

/// Median time per call of `f` (called `batch` times per sample); without
/// `--bench`, call `f` once.
fn bench(name: &str, batch: u32, mut f: impl FnMut()) {
    if !measuring() {
        f();
        println!("{name:<48} ok (not measured: run `cargo bench`)");
        return;
    }
    for _ in 0..batch {
        f();
    }
    let mut samples: Vec<f64> = (0..15)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..batch {
                f();
            }
            start.elapsed().as_secs_f64() * 1e9 / f64::from(batch)
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    let median = samples[samples.len() / 2];
    println!("{name:<48} {median:>10.1} ns/op");
}

fn main() {
    bench("tags_from_filename (10 mixed names, total)", 20_000, || {
        for name in NAMES {
            black_box(tags_from_filename(black_box(name)));
        }
    });
    bench("tags_from_filename(\"pre_commit/run.py\")", 200_000, || {
        black_box(tags_from_filename(black_box("pre_commit/run.py")));
    });
    bench(
        "tags_from_interpreter(\"/usr/bin/python3.12.1\")",
        200_000,
        || {
            black_box(tags_from_interpreter(black_box("/usr/bin/python3.12.1")));
        },
    );
    bench("Tag::from_name(\"pyproject\")", 200_000, || {
        black_box(Tag::from_name(black_box("pyproject")));
    });
    let tags: TagSet = "file, non-executable, python, text".parse().expect("tags");
    let types: TagSet = "file".parse().expect("tags");
    let types_or: TagSet = "python, pyi".parse().expect("tags");
    let exclude: TagSet = "binary".parse().expect("tags");
    bench("TagSet::matches_types", 1_000_000, || {
        black_box(black_box(tags).matches_types(&types, &types_or, &exclude));
    });
    bench("classify(regular, \"src/main.py\")", 200_000, || {
        black_box(classify(Meta::regular(false), black_box("src/main.py")));
    });
    bench("parse_shebang_bytes(env -S python3 -u)", 200_000, || {
        black_box(parse_shebang_bytes(black_box(
            b"#!/usr/bin/env -S python3 -u\n",
        )));
    });

    // File-system benchmarks (warm page cache).
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();
    let mut files: Vec<(PathBuf, &str)> = Vec::new();
    let mut add = |name: &str, content: &[u8], exec: bool, label: &'static str| {
        let path = d.join(name);
        fs::write(&path, content).expect("write");
        #[cfg(unix)]
        if exec {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        #[cfg(not(unix))]
        let _ = exec;
        files.push((path, label));
    };
    add("a.py", b"print()\n", false, "known extension");
    add(
        "data.unknownext",
        b"some text\n",
        false,
        "unknown extension",
    );
    add(
        "script",
        b"#!/usr/bin/env python3\nprint()\n",
        true,
        "executable script",
    );
    add(
        "f.plist",
        b"bplist00\xd1\x01",
        false,
        "binary-check extension",
    );
    for (path, label) in &files {
        bench(&format!("tags_from_path: {label}"), 5_000, || {
            black_box(tags_from_path(black_box(path)).expect("identify"));
        });
    }
    let unknown = d.join("data.unknownext");
    let md = fs::symlink_metadata(&unknown).expect("lstat");
    bench(
        "tags_from_metadata (own lstat, unknown extension)",
        5_000,
        || {
            black_box(
                tags_from_metadata(black_box(&unknown), &md, ExecPolicy::PythonCompat)
                    .expect("identify"),
            );
        },
    );
    let known = d.join("a.py");
    let meta = Meta::from_git_mode(0o100_644).expect("mode");
    bench(
        "tags_from_meta(git 100644, known extension)",
        200_000,
        || {
            black_box(tags_from_meta(black_box(&known), meta).expect("identify"));
        },
    );
}
