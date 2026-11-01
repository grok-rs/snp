//! Integration tests: run output through the binary — colours, columns, the pty (T10-OUT; the byte-level rows run as
//! unit tests of `snp_run::render`).
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

const CFG: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: p\n        name: passing hook\n        entry: 'true'\n        language: system\n        always_run: true\n";

#[test]
fn colour_always_and_never() {
    let env = env();
    let dir = repo(&env, "o", CFG);
    let out = env
        .snp_as_pre_commit(&dir)
        .args(["run", "--color", "always"])
        .output();
    assert_eq!(
        text(&out.stdout),
        format!("passing hook{}\x1b[42mPassed\x1b[m\n", rep(".", 61))
    );
    let out = env
        .snp_as_pre_commit(&dir)
        .args(["run", "--color", "never"])
        .output();
    assert_eq!(text(&out.stdout), line("passing hook", "Passed"));
}

#[test]
fn auto_colour_follows_stderr() {
    let env = env();
    let dir = repo(&env, "o", CFG);
    // stderr is a pipe: no colour, whatever stdout is
    let out = env.snp_as_pre_commit(&dir).arg("run").output();
    assert_eq!(text(&out.stdout), line("passing hook", "Passed"));
    let out = env
        .snp_as_pre_commit(&dir)
        .env("PRE_COMMIT_COLOR", "always")
        .arg("run")
        .output();
    assert!(text(&out.stdout).contains("\x1b[42mPassed"));
}
