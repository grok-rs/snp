//! Integration tests: `migrate-config` (the command; the conversion rules are WP-21's T10-MIG rows).
//!
//! Owner: WP-33.

use crate::cli::{assert_code, compat, repo_with, tenv, text};

#[test]
fn migrate_config_rewrites_and_reports() {
    let env = tenv();
    let r = repo_with(&env, "r", "-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: system\n        stages: [commit]\n");
    let out = compat(&env, &r, &["migrate-config"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), "Configuration has been migrated.\n");
    let cfg = std::fs::read_to_string(r.join(".pre-commit-config.yaml")).unwrap();
    assert!(cfg.starts_with("repos:\n"), "{cfg}");
    assert!(cfg.contains("stages: [pre-commit]"), "{cfg}");
    let out = compat(&env, &r, &["migrate-config"]);
    assert_eq!(text(&out.stdout), "Configuration is already migrated.\n");
    // from a subdirectory: `-c` is relative to the cwd before the chdir
    std::fs::create_dir_all(r.join("sub")).unwrap();
    let out = compat(&env, &r.join("sub"), &["migrate-config", "-c", "../.pre-commit-config.yaml"]);
    assert_eq!(text(&out.stdout), "Configuration is already migrated.\n");
}

#[test]
fn migrate_config_invalid_yaml_and_missing_file() {
    let env = tenv();
    let r = repo_with(&env, "r", "{\n");
    let out = compat(&env, &r, &["migrate-config"]);
    assert_code(&out, 1);
    assert!(text(&out.stdout).starts_with("An error has occurred: InvalidConfigError: \n==> File .pre-commit-config.yaml\n=====> "));
    let out = compat(&env, &r, &["migrate-config", "-c", "missing.yaml"]);
    assert_code(&out, 1);
    assert!(text(&out.stdout).contains("=====> missing.yaml is not a file"), "{}", text(&out.stdout));
}

/// The file mode survives the rewrite and a symlinked config is written through to its target.
#[cfg(unix)]
#[test]
fn migrate_config_keeps_mode_and_symlink() {
    use std::os::unix::fs::PermissionsExt;
    let env = tenv();
    let r = repo_with(&env, "r", "x: 1\n");
    std::fs::write(r.join("real.yaml"), "-   repo: meta\n    hooks:\n    -   id: identity\n").unwrap();
    std::fs::set_permissions(r.join("real.yaml"), std::fs::Permissions::from_mode(0o640)).unwrap();
    std::fs::remove_file(r.join(".pre-commit-config.yaml")).unwrap();
    std::os::unix::fs::symlink("real.yaml", r.join(".pre-commit-config.yaml")).unwrap();
    assert_code(&compat(&env, &r, &["migrate-config"]), 0);
    assert!(std::fs::symlink_metadata(r.join(".pre-commit-config.yaml")).unwrap().file_type().is_symlink());
    assert!(std::fs::read_to_string(r.join("real.yaml")).unwrap().starts_with("repos:\n"));
    assert_eq!(std::fs::metadata(r.join("real.yaml")).unwrap().permissions().mode() & 0o777, 0o640);
}
