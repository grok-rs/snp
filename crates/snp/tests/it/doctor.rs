//! Integration tests: `snp doctor` (11 §A.15).
//!
//! Owner: WP-33.

use crate::cli::{
    LOCAL_CONFIG, assert_code, compat, repo_with, slashed, snp, tenv, text as raw_text,
};
use crate::install::{pre_commit_script, write_hook};

/// [`crate::cli::text`] with the path separator read as `/` (doctor names hooks by their joined paths).
fn text(b: &[u8]) -> String {
    slashed(&raw_text(b))
}

#[test]
fn doctor_clean_repository() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    assert_code(&snp(&env, &r, &["install"]), 0);
    let out = snp(&env, &r, &["doctor"]);
    assert_code(&out, 0);
    let o = text(&out.stdout);
    assert!(
        o.starts_with("info  snp: 2.0.1 (pre-commit 4.7.0 compatible), "),
        "{o}"
    );
    assert!(o.contains("ok    .git/hooks/pre-commit: runs snp\n"), "{o}");
    assert!(o.contains("ok    stash: no unrestored stash\n"), "{o}");
    assert!(
        o.lines()
            .last()
            .unwrap()
            .ends_with("error(s), 0 warning(s).")
            || o.contains(" 0 error(s), "),
        "{o}"
    );
    // compat brand has no doctor
    assert_code(&compat(&env, &r, &["doctor"]), 2);
}

#[test]
fn doctor_outside_a_repository() {
    let env = tenv();
    let out = snp(&env, env.root(), &["doctor"]);
    assert!(text(&out.stdout).contains("info  repository: not inside a git repository\n"));
}

#[test]
fn doctor_predicts_failing_commits_and_fixes_them() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    // Case C: INSTALL_PYTHON missing and nothing named pre-commit on PATH.
    write_hook(
        &r.join(".git/hooks/pre-commit"),
        pre_commit_script("/nonexistent/python", " --skip-on-missing-config").as_bytes(),
    );
    let env2 = env.with_env("PATH", crate::cli::path_without_snp(&env));
    let out = env2.snp(&r).arg("doctor").output();
    let o = text(&out.stdout);
    if !o.contains("pre-commit on PATH: not found") {
        return; // a system pre-commit answers (Case B)
    }
    assert_code(&out, 1);
    assert!(
        o.contains("error .git/hooks/pre-commit: commits fail: nothing named pre-commit on PATH"),
        "{o}"
    );
    let out = snp(&env, &r, &["doctor", "--fix"]);
    let script = std::fs::read_to_string(r.join(".git/hooks/pre-commit")).unwrap();
    assert!(
        script.contains("--skip-on-missing-config"),
        "parsed options kept: {script}"
    );
    assert!(text(&out.stdout).contains("ok    .git/hooks/pre-commit: runs snp"));
}

#[test]
fn doctor_fast_path_table() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    let out = snp(&env, &r, &["doctor", "--fast-path"]);
    assert!(text(&out.stdout).contains("fast path: "));
}

/// `snp install -c $'my\ncfg.yaml'`: the quoted config spans two lines of the script; doctor reads it (no false
/// "`SNP_EXE` is not executable") and `--fix` has nothing to rewrite, so the config path survives.
#[cfg(unix)]
#[test]
fn doctor_reads_a_multi_line_config_token() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    std::fs::write(r.join("my\ncfg.yaml"), LOCAL_CONFIG).unwrap();
    assert_code(&snp(&env, &r, &["install", "-c", "my\ncfg.yaml"]), 0);
    let before = std::fs::read(r.join(".git/hooks/pre-commit")).unwrap();
    let out = snp(&env, &r, &["doctor"]);
    let o = text(&out.stdout);
    assert!(o.contains("ok    .git/hooks/pre-commit: runs snp\n"), "{o}");
    let _fixed = snp(&env, &r, &["doctor", "--fix"]);
    assert_eq!(
        std::fs::read(r.join(".git/hooks/pre-commit")).unwrap(),
        before
    );
}
