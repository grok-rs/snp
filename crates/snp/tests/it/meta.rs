//! Integration tests: meta hooks through the binary (T10-MET; the rows themselves run in
//! `crates/snp-run/tests/pipeline/meta.rs`). `#[ignore = "needs WP-33"]` until the entry merges.
//!
//! Owner: WP-32.

use std::path::PathBuf;

use snp_testkit::{Bins, TestEnv};

/// `s` repeated `n` times (`str::repeat` is a disallowed method).
fn rep(s: &str, n: usize) -> String {
    std::iter::repeat_n(s, n).collect()
}

fn env() -> TestEnv {
    let snp = PathBuf::from(env!("CARGO_BIN_EXE_snp"));
    let probe = Bins::locate().map_or_else(|| snp.clone(), |b| b.probe);
    TestEnv::new(Bins { snp, probe })
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `{name}{dots}{status}` at 80 columns.
fn line(name: &str, status: &str) -> String {
    format!(
        "{name}{}{status}\n",
        rep(".", 79 - name.chars().count() - status.chars().count())
    )
}

/// A committed repository with `config` as `.pre-commit-config.yaml`.
fn repo(env: &TestEnv, name: &str, config: &str) -> PathBuf {
    env.git_repo(name)
        .file(".pre-commit-config.yaml", config.as_bytes())
        .stage(".pre-commit-config.yaml")
        .commit("config")
        .build()
}

#[test]
#[ignore = "needs WP-33"]
fn check_useless_excludes_through_the_binary() {
    let env = env();
    let dir = repo(
        &env,
        "m",
        "exclude: nomatch\nrepos:\n-   repo: meta\n    hooks:\n    -   id: check-useless-excludes\n",
    );
    let out = env
        .snp_as_pre_commit(&dir)
        .args(["run", "--all-files"])
        .output();
    assert_eq!(out.code, 1);
    assert!(text(&out.stdout).starts_with(&line("Check for useless excludes", "Failed")));
    assert!(
        text(&out.stdout).contains("The global exclude pattern 'nomatch' does not match any files")
    );
}

#[test]
#[ignore = "needs WP-33"]
fn check_hooks_apply_with_one_job() {
    let env = env();
    let dir = repo(
        &env,
        "m",
        "repos:\n-   repo: meta\n    hooks:\n    -   id: check-hooks-apply\n",
    );
    let out = env
        .snp_as_pre_commit(&dir)
        .env("PRE_COMMIT_NO_CONCURRENCY", "1")
        .args(["run", "--all-files"])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stdout));
}
