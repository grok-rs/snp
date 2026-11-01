//! Integration tests: `snp self alias install|uninstall|status` and `snp self check` (11 §A.9).
//!
//! Owner: WP-33.

use std::path::PathBuf;

use crate::cli::{assert_code, snp, tenv, text};

fn path_with(dir: &std::path::Path) -> String {
    format!("{}:/usr/bin:/bin", dir.display())
}

#[cfg(unix)]
#[test]
fn alias_install_status_uninstall() {
    let env = tenv();
    let dir = env.root().join("aliases");
    std::fs::create_dir_all(&dir).unwrap();
    let env = env.with_env("PATH", path_with(&dir));
    let d = dir.to_str().unwrap();
    let out = snp(&env, env.root(), &["self", "alias", "install", "--dir", d]);
    assert_code(&out, 0);
    let alias: PathBuf = dir.join("pre-commit");
    assert_eq!(
        text(&out.stdout),
        format!("Created {} -> {}\n", alias.display(), env!("CARGO_BIN_EXE_snp"))
    );
    assert!(std::fs::symlink_metadata(&alias).unwrap().file_type().is_symlink());
    // idempotent
    assert_code(&snp(&env, env.root(), &["self", "alias", "install", "--dir", d]), 0);
    let out = snp(&env, env.root(), &["self", "alias", "status"]);
    assert_code(&out, 0);
    assert_eq!(text(&out.stdout), format!("pre-commit -> {} (snp alias (this snp))\n", alias.display()));
}

#[cfg(unix)]
#[test]
fn alias_force_keeps_a_backup_and_uninstall_restores_it() {
    let env = tenv();
    // the default dir is the parent of the stable snp path: use a copy of snp in its own dir
    let bin = env.root().join("bin2");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_snp"), bin.join("snp")).unwrap();
    crate::install::write_hook(&bin.join("pre-commit"), b"#!/bin/sh\necho foreign\n");
    let mut cmd = std::process::Command::new(bin.join("snp"));
    env.env().apply_to(&mut cmd);
    let run = |args: &[&str]| {
        let mut c = std::process::Command::new(bin.join("snp"));
        env.env().apply_to(&mut c);
        c.env("PATH", path_with(&bin)).args(args).current_dir(env.root()).output().unwrap()
    };
    drop(cmd);
    let o = run(&["self", "alias", "install"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("already exists and is not snp (unknown); use --force"), "{}", text(&o.stderr));
    let o = run(&["self", "alias", "install", "--force"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(std::fs::read(bin.join("pre-commit.snp-backup")).unwrap(), b"#!/bin/sh\necho foreign\n");
    let o = run(&["self", "alias", "uninstall"]);
    assert_eq!(o.status.code(), Some(0));
    let alias = bin.join("pre-commit");
    assert_eq!(
        text(&o.stdout),
        format!("Removed {}\nRestored {} from {}.snp-backup\n", alias.display(), alias.display(), alias.display())
    );
    assert_eq!(std::fs::read(&alias).unwrap(), b"#!/bin/sh\necho foreign\n");
    // a foreign pre-commit is left alone
    let o = run(&["self", "alias", "uninstall"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).ends_with("is not an snp alias; leaving it alone\n"));
}

#[test]
fn self_check_passes() {
    let env = tenv();
    let out = snp(&env, env.root(), &["self", "check"]);
    assert_code(&out, 0);
    let o = text(&out.stdout);
    assert!(o.contains("ok   shlex: "), "{o}");
    assert!(o.contains("ok   hook script: "), "{o}");
    assert!(!o.contains("FAIL"), "{o}");
}
