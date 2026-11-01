//! Integration tests: `snp config dump|schema`.
//!
//! Owner: WP-33.

use crate::cli::{LOCAL_CONFIG, assert_code, compat, repo_with, snp, tenv, text};

#[test]
fn config_dump_yaml_json_and_overlay() {
    let env = tenv();
    let r = repo_with(&env, "r", LOCAL_CONFIG);
    let out = snp(&env, &r, &["config", "dump"]);
    assert_code(&out, 0);
    let o = text(&out.stdout);
    assert!(o.contains("repos:\n-   repo: local\n"), "{o}");
    assert!(o.contains("default_install_hook_types:"), "{o}");
    let out = snp(&env, &r, &["config", "dump", "--json"]);
    assert_code(&out, 0);
    assert!(text(&out.stdout).trim_start().starts_with('{'));
    // an untracked local overlay is applied unless --no-local-config
    std::fs::write(
        r.join(".pre-commit-config.local.yaml"),
        "fail_on_changes: never\n",
    )
    .unwrap();
    let with = snp(&env, &r, &["config", "dump"]);
    assert_code(&with, 0);
    let without = snp(&env, &r, &["config", "dump", "--no-local-config"]);
    assert_code(&without, 0);
    assert_ne!(with.stdout, without.stdout);
    // snp-only
    assert_code(&compat(&env, &r, &["config", "dump"]), 2);
}

#[test]
fn config_schema_is_json() {
    let env = tenv();
    let out = snp(&env, env.root(), &["config", "schema"]);
    assert_code(&out, 0);
    let o = text(&out.stdout);
    assert!(
        o.trim_start().starts_with('{') && o.contains("\"repos\""),
        "{o}"
    );
    assert!(o.ends_with("}\n"));
}
