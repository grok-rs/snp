//! The content cache of a window of consecutive in-process hooks in the scheduler (ADR-131): a run over files older
//! than the racy window (cached) prints, exits and leaves files exactly like the same run over files written just
//! before it (racy: nothing is kept), with `SNP_JOBS=1` and `SNP_JOBS=2`, under `SNP_VERIFY_FILE_CACHE=1`; the cache
//! is used in a window of two or more readers, never in a concurrent priority bucket, and a process hook ends the
//! window. Hits are counted with `snp_hooks::content_cache::process_hits` (nextest runs each test in its own
//! process).

use std::sync::Arc;
use std::time::Duration;

use snp_hooks::content_cache::{RACY_WINDOW, process_hits};
use snp_repo::StoreResolver;

use crate::harness::{H, Out, args, sys};

/// The staged files of every case.
const FILES: [(&str, &[u8]); 7] = [
    ("a.yaml", b"a: 1  \nb: [2]\r\n"),
    ("b.yaml", b"k: v\n"),
    ("c.json", b"{\"a\": 1}   \n\n\n"),
    ("d.json", b"{\"a\": }\n"),
    ("e.txt", b"no newline at end # TODO"),
    ("f.txt", b"mixed\r\nendings\nhere\r\n"),
    ("g.txt", b"clean\n"),
];

/// pre-commit-hooks v6.0.0 hooks (the native fast path).
fn pch(hooks: &str) -> String {
    format!(
        "-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v6.0.0\n    hooks:\n{hooks}"
    )
}

/// A local `pygrep` hook.
fn pygrep(extra: &str) -> String {
    format!(
        "-   repo: local\n    hooks:\n    -   id: todo\n        name: todo\n        entry: TODO\n        \
         language: pygrep\n{extra}"
    )
}

/// A repository with `config` committed and [`FILES`] staged.
fn repo(config: &str, jobs: usize) -> H {
    let mut h = H::new();
    h.jobs = jobs;
    h.resolver = Some(Arc::new(StoreResolver::default()));
    h.env_set("GIT_ALLOW_PROTOCOL", "file");
    h.env_set("SNP_VERIFY_FILE_CACHE", "1");
    h.config(config);
    for (p, b) in FILES {
        h.write(p, b);
    }
    h.stage(".");
    h
}

/// The bytes of [`FILES`] after a run.
fn files(h: &H) -> Vec<Option<Vec<u8>>> {
    FILES
        .iter()
        .map(|(p, _)| std::fs::read(h.path(p)).ok())
        .collect()
}

/// Whether the test's temp dir can hold a content cache at all (`stat_oracle_safe`).
fn cache_possible(h: &H) -> bool {
    snp_os::fsinfo::stat_oracle_safe(&h.root).unwrap_or(false)
}

/// For `SNP_JOBS` 1 and 2: runs `config` over aged files and over fresh ones; asserts the runs agree and no verified
/// hit differed; returns the aged runs' hit counts. (The fresh repository is made right before its run, so its files
/// are racy and nothing is kept; on a machine slow enough to take the racy window over the run, some are, and every
/// such hit is still checked by `SNP_VERIFY_FILE_CACHE`.)
fn cached_vs_fresh(config: &str) -> Vec<u64> {
    let aged: Vec<H> = [1, 2].map(|jobs| repo(config, jobs)).into();
    std::thread::sleep(RACY_WINDOW + Duration::from_millis(300));
    let mut hits = Vec::new();
    for h in &aged {
        let before = process_hits();
        let got = h.run(args());
        let mid = process_hits();
        let fresh = repo(config, h.jobs);
        let want = fresh.run(args());
        let what = format!("SNP_JOBS={}", h.jobs);
        assert!(got.failure.is_none(), "{what}: {:?}", got.failure);
        assert_same(&got, &want, &what);
        assert_eq!(files(h), files(&fresh), "{what}");
        hits.push(if cache_possible(h) {
            mid.saturating_sub(before)
        } else {
            u64::MAX
        });
    }
    hits
}

fn assert_same(got: &Out, want: &Out, what: &str) {
    assert_eq!(
        (got.code, &got.stdout, &got.stderr),
        (want.code, &want.stdout, &want.stderr),
        "{what}"
    );
    assert!(
        !got.stderr.contains("SNP_VERIFY_FILE_CACHE"),
        "{what}: {}",
        got.stderr
    );
}

/// Fixers, then checkers reading what they wrote, and a pygrep hook: one window, served from the cache where the
/// files did not change, and the same output and files as without it.
#[test]
fn cached_run_equals_uncached_run() {
    let config = format!(
        "repos:\n{}{}",
        pch(
            "    -   id: check-yaml\n    -   id: trailing-whitespace\n    -   id: end-of-file-fixer\n    \
             -   id: mixed-line-ending\n    -   id: check-yaml\n    -   id: check-json\n    \
             -   id: detect-private-key\n"
        ),
        pygrep(""),
    );
    for (jobs, hits) in [1, 2].iter().zip(cached_vs_fresh(&config)) {
        assert!(hits > 0, "SNP_JOBS={jobs}: no hit, the test proves nothing");
    }
}

/// Two natives with the same priority run concurrently: a bucket of several hooks gets no cache.
#[test]
fn a_concurrent_bucket_has_no_cache() {
    let config = format!(
        "repos:\n{}{}",
        pch(
            "    -   id: trailing-whitespace\n        priority: 0\n    -   id: check-yaml\n        priority: 0\n    \
             -   id: detect-private-key\n        priority: 0\n"
        ),
        pygrep("        priority: 0\n"),
    );
    for hits in cached_vs_fresh(&config) {
        assert!(
            hits == 0 || hits == u64::MAX,
            "{hits} hits in a concurrent bucket"
        );
    }
}

/// A process hook ends the window: with one reader on each side there is no cache; a process hook that rewrites a
/// file (same size) between two windows is seen by the natives after it.
#[test]
fn a_process_hook_ends_the_window() {
    let one_each = format!(
        "repos:\n{}-   repo: local\n    hooks:\n{}{}",
        pch("    -   id: detect-private-key\n"),
        sys(
            "proc",
            "proc",
            "'true'",
            "pass_filenames: false\nalways_run: true"
        ),
        pch("    -   id: trailing-whitespace\n"),
    );
    for hits in cached_vs_fresh(&one_each) {
        assert!(
            hits == 0 || hits == u64::MAX,
            "{hits} hits across a process hook"
        );
    }
    let rewrite = format!(
        "repos:\n{}-   repo: local\n    hooks:\n{}{}",
        pch("    -   id: detect-private-key\n    -   id: check-json\n"),
        sys(
            "proc",
            "proc",
            "sh -c 'printf \"k=vv\\n\" > b.yaml' --",
            "pass_filenames: false\nalways_run: true"
        ),
        pch(
            "    -   id: check-yaml\n    -   id: trailing-whitespace\n    -   id: end-of-file-fixer\n"
        ),
    );
    for (jobs, hits) in [1, 2].iter().zip(cached_vs_fresh(&rewrite)) {
        assert!(hits > 0, "SNP_JOBS={jobs}: no hit, the test proves nothing");
    }
}
