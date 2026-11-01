//! Integration tests: xargs partitioning end to end (T10-XRG-17, T10-XRG-18) through the binary. The same rows run
//! through `snp_run::run` in `crates/snp-run/tests/pipeline/run.rs`; these add the binary's entry (WP-33).
//!
//! Owner: WP-32.

use std::path::PathBuf;

use snp_testkit::{Bins, TestEnv};

fn env() -> TestEnv {
    let snp = PathBuf::from(env!("CARGO_BIN_EXE_snp"));
    let probe = Bins::locate().map_or_else(|| snp.clone(), |b| b.probe);
    TestEnv::new(Bins { snp, probe })
}

fn cfg(entry: &str, extra: &str) -> String {
    format!(
        "repos:\n-   repo: local\n    hooks:\n    -   id: e\n        name: e\n        entry: {entry}\n        \
         language: system\n        verbose: true\n{extra}"
    )
}

#[test]
#[ignore = "needs WP-33"]
fn t10_xrg_17_serial_echo_keeps_order() {
    let env = env();
    let mut repo = env.git_repo("x").file(
        ".pre-commit-config.yaml",
        cfg("echo", "        require_serial: true\n").as_bytes(),
    );
    for i in 0..10 {
        repo = repo.file(&i.to_string(), b"");
    }
    let dir = repo.stage_all().build();
    let files: Vec<String> = (0..10).map(|i| i.to_string()).collect();
    let out = env
        .snp_as_pre_commit(&dir)
        .arg("run")
        .arg("--files")
        .args(&files)
        .output();
    assert!(String::from_utf8_lossy(&out.stdout).ends_with("\n\n0 1 2 3 4 5 6 7 8 9\n\n"));
}

#[test]
#[ignore = "needs WP-33"]
fn t10_xrg_18_entry_args_then_files() {
    let env = env();
    let dir = env
        .git_repo("x")
        .file(
            ".pre-commit-config.yaml",
            cfg("echo hi", "        args: [hello]\n").as_bytes(),
        )
        .file("file", b"")
        .stage_all()
        .build();
    let out = env
        .snp_as_pre_commit(&dir)
        .args(["run", "--files", "file", "file", "file"])
        .output();
    assert!(String::from_utf8_lossy(&out.stdout).ends_with("\n\nhi hello file file file\n\n"));
}
