//! Integration tests: snp's config extension keys through the binary (ARCHITECTURE §8). The semantics are tested
//! through `snp_run::run` in `crates/snp-run/tests/pipeline/extensions.rs`; these pin the CLI surface (`--group`,
//! `-j`, `--summary`) and are `#[ignore = "needs WP-33"]` until the entry merges.
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

const CFG: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: lint:js\n        name: lint js\n        entry: 'true'\n        language: system\n        always_run: true\n        groups: [lint]\n        priority: 0\n    -   id: slow\n        name: slow\n        entry: 'true'\n        language: system\n        always_run: true\n        groups: [slow]\n        priority: 0\n";

#[test]
#[ignore = "needs WP-33"]
fn selectors_and_groups_on_the_command_line() {
    let env = env();
    let dir = repo(&env, "e", CFG);
    let out = env.snp(&dir).args(["run", "lint:js"]).output();
    assert_eq!(text(&out.stdout), line("lint js", "Passed"));
    let out = env.snp(&dir).args(["run", "--no-group", "lint"]).output();
    assert_eq!(text(&out.stdout), line("slow", "Passed"));
}

#[test]
#[ignore = "needs WP-33"]
fn jobs_do_not_change_the_output() {
    let env = env();
    let dir = repo(&env, "e", CFG);
    let a = env.snp(&dir).args(["run", "-j", "1"]).output();
    let b = env.snp(&dir).args(["run", "-j", "32"]).output();
    assert_eq!(a.stdout, b.stdout);
}

#[test]
#[ignore = "needs WP-33"]
fn summary_footer() {
    let env = env();
    let dir = repo(&env, "e", CFG);
    let out = env.snp(&dir).args(["run", "--summary"]).output();
    assert!(text(&out.stdout).contains("Summary: 2 passed, 0 failed, 0 skipped in "));
}
