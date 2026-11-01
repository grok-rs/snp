//! Integration tests: the staged-files stash in the run pipeline, end to end (ARCHITECTURE §4): the restore on the
//! interrupt and panic paths through the binary (`--features testing`: fault injection, ADR-066). The pipeline-level
//! stash tests (conflict, rollback, EPIPE, interrupt) run through `snp_run::run` in
//! `crates/snp-run/tests/pipeline/run.rs`; these need WP-33's entry.
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

/// A committed `a.txt` with a staged and an unstaged change, and `config`.
fn dirty(env: &TestEnv, config: &str) -> PathBuf {
    let dir = env
        .git_repo("s")
        .file(".pre-commit-config.yaml", config.as_bytes())
        .file("a.txt", b"a\n")
        .stage_all()
        .commit("init")
        .file("a.txt", b"a\nstaged\n")
        .stage("a.txt")
        .build();
    std::fs::write(dir.join("a.txt"), b"a\nstaged\nunstaged\n").unwrap();
    dir
}

const CFG: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: p\n        name: printing\n        entry: echo out\n        language: system\n        verbose: true\n";

#[test]
#[cfg_attr(
    not(feature = "testing"),
    ignore = "needs a `--features testing` build"
)]
fn a_panic_after_a_hook_still_restores_the_stash() {
    let env = env();
    let dir = dirty(&env, CFG);
    // the 3rd Printer write panics: after the hook's start segment, inside the run
    let out = env
        .snp_as_pre_commit(&dir)
        .env("SNP_FAULT", "PrinterWrite@3")
        .arg("run")
        .output();
    assert_eq!(out.code, 3, "{}", text(&out.stdout));
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "a\nstaged\nunstaged\n"
    );
}

#[test]
fn sigint_during_a_hook_restores_the_stash() {
    let env = env();
    let dir = dirty(
        &env,
        "repos:\n-   repo: local\n    hooks:\n    -   id: slow\n        name: slow\n        entry: sleep 30\n        language: system\n        pass_filenames: false\n",
    );
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_snp"))
        .arg("run")
        .current_dir(&dir)
        .env_clear()
        .envs(env.env().iter())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130));
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "a\nstaged\nunstaged\n"
    );
}
