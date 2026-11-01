//! Integration tests: `uninstall` (the T10-INS uninstall rows live in `install.rs`; these cover 11 §A.5.2).
//!
//! Owner: WP-33.

use crate::cli::{LOCAL_CONFIG, assert_code, compat, repo_with, snp, tenv, text};
use crate::install::{prek_script, write_hook};

/// A prek script is not ours: left alone silently; snp's own script in `.legacy` is deleted with M12.
#[test]
fn uninstall_manager_rules() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    let hook = r.join(".git/hooks/pre-commit");
    write_hook(&hook, prek_script("").as_bytes());
    let out = compat(&env, &r, &["uninstall"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), "");
    assert!(hook.exists());
    // snp script with snp's own script as .legacy
    assert_code(&snp(&env, &r, &["install"]), 0);
    let script = std::fs::read(&hook).unwrap();
    write_hook(&r.join(".git/hooks/pre-commit.legacy"), &script);
    let out = snp(&env, &r, &["uninstall"]);
    assert_code(&out, 0);
    // `{hook_type} uninstalled` names the hook type, never the tool (ADR-084): not branded
    assert_eq!(text(&out.stdout), "pre-commit uninstalled\n");
    assert_eq!(
        text(&out.stderr),
        "Removed .git/hooks/pre-commit.legacy: it is a hook script from snp, not a user hook\n"
    );
    assert!(!hook.exists());
}

/// A prek `.legacy` is restored (rollback to that tool, D-BRAND-02).
#[test]
fn uninstall_restores_a_prek_legacy() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    assert_code(&snp(&env, &r, &["install"]), 0);
    write_hook(
        &r.join(".git/hooks/pre-commit.legacy"),
        prek_script("").as_bytes(),
    );
    let out = compat(&env, &r, &["uninstall"]);
    assert_eq!(
        text(&out.stdout),
        "pre-commit uninstalled\nRestored previous hooks to .git/hooks/pre-commit\n"
    );
}

/// ADR-084: a multi-type uninstall names each hook type; `pre-commit uninstalled` is not branded into `snp
/// uninstalled` (which read as if snp itself had been removed).
#[test]
fn uninstall_names_every_hook_type() {
    let env = tenv();
    let r = repo_with(&env, "multi", LOCAL_CONFIG);
    assert_code(
        &snp(&env, &r, &["install", "-t", "pre-commit", "-t", "pre-push"]),
        0,
    );
    let out = snp(
        &env,
        &r,
        &["uninstall", "-t", "pre-commit", "-t", "pre-push"],
    );
    assert_code(&out, 0);
    assert_eq!(
        text(&out.stdout),
        "pre-commit uninstalled\npre-push uninstalled\n"
    );
}
