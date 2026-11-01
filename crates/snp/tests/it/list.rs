//! Integration tests: `snp list` through the binary (the listing is tested through `snp_run::list` in
//! `crates/snp-run/tests/pipeline/extensions.rs`).
//!
//! Owner: WP-32.

use std::path::PathBuf;

use snp_testkit::{Bins, TestEnv};

fn env() -> TestEnv {
    let snp = PathBuf::from(env!("CARGO_BIN_EXE_snp"));
    let probe = Bins::locate().map_or_else(|| snp.clone(), |b| b.probe);
    TestEnv::new(Bins { snp, probe })
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
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
fn list_text_json_and_filters() {
    let env = env();
    let dir = repo(
        &env,
        "l",
        "repos:\n-   repo: local\n    hooks:\n    -   id: a\n        name: A\n        entry: 'true'\n        language: system\n        groups: [g]\n    -   id: b\n        name: B\n        entry: 'true'\n        language: system\n        stages: [pre-push]\n",
    );
    let out = env.snp(&dir).arg("list").output();
    assert_eq!(
        (out.code, text(&out.stdout)),
        (0, String::from("a  A [g]\nb  B\n"))
    );
    let out = env
        .snp(&dir)
        .args([
            "list",
            "--hook-stage",
            "pre-push",
            "--group",
            "@ungrouped",
            "--json",
        ])
        .output();
    assert!(text(&out.stdout).starts_with("[\n  {\"id\": \"b\""));
}
