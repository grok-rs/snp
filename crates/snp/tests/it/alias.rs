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
        format!(
            "Created {} -> {}\n",
            alias.display(),
            env!("CARGO_BIN_EXE_snp")
        )
    );
    assert!(
        std::fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // idempotent
    assert_code(
        &snp(&env, env.root(), &["self", "alias", "install", "--dir", d]),
        0,
    );
    let out = snp(&env, env.root(), &["self", "alias", "status"]);
    assert_code(&out, 0);
    assert_eq!(
        text(&out.stdout),
        format!("pre-commit -> {} (snp alias (this snp))\n", alias.display())
    );
}

#[cfg(unix)]
#[test]
fn alias_force_keeps_a_backup_and_uninstall_restores_it() {
    let env = tenv();
    // the default dir is the parent of the stable snp path: use a copy of snp in its own dir
    let bin = env.root().join("bin2");
    std::fs::create_dir_all(&bin).unwrap();
    crate::install::copy_snp(&bin.join("snp"));
    crate::install::write_hook(&bin.join("pre-commit"), b"#!/bin/sh\necho foreign\n");
    let run = |args: &[&str]| {
        let mut c = std::process::Command::new(bin.join("snp"));
        env.env().apply_to(&mut c);
        c.env("PATH", path_with(&bin))
            .args(args)
            .current_dir(env.root())
            .output()
            .unwrap()
    };
    let o = run(&["self", "alias", "install"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stderr).contains("already exists and is not snp (unknown); use --force"),
        "{}",
        text(&o.stderr)
    );
    let o = run(&["self", "alias", "install", "--force"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(
        std::fs::read(bin.join("pre-commit.snp-backup")).unwrap(),
        b"#!/bin/sh\necho foreign\n"
    );
    let o = run(&["self", "alias", "uninstall"]);
    assert_eq!(o.status.code(), Some(0));
    let alias = bin.join("pre-commit");
    assert_eq!(
        text(&o.stdout),
        format!(
            "Removed {}\nRestored {} from {}.snp-backup\n",
            alias.display(),
            alias.display(),
            alias.display()
        )
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

/// A different snp build (another version, a stale Windows hard link) named `pre-commit` is recognised as snp
/// although its compat-brand `--version` prints only `pre-commit 4.6.2`: the probe sets `SNP_VERSION_PROBE=1`
/// (11 §A.19.4). Python pre-commit (which ignores the variable) is still reported as such.
#[cfg(unix)]
#[test]
fn alias_status_recognises_another_snp_build() {
    let env = tenv();
    let dir = env.root().join("other");
    let mut bytes = std::fs::read(env!("CARGO_BIN_EXE_snp")).unwrap();
    bytes.extend_from_slice(b"\0not the same build\0");
    crate::install::write_hook(&dir.join("pre-commit"), &bytes);
    let env2 = env.with_env("PATH", path_with(&dir));
    let out = snp(&env2, env.root(), &["self", "alias", "status"]);
    assert_code(&out, 0);
    assert_eq!(
        text(&out.stdout),
        format!(
            "pre-commit -> {} (snp alias (snp {}))\n",
            dir.join("pre-commit").display(),
            env!("CARGO_PKG_VERSION")
        )
    );
    // `install --dir` over it is not refused as "not snp" (it replaces the other build's alias or keeps it).
    let d = dir.to_str().unwrap();
    let out = snp(&env2, env.root(), &["self", "alias", "install", "--dir", d]);
    assert!(
        !text(&out.stderr).contains("is not snp"),
        "{}",
        text(&out.stderr)
    );
    assert_code(&out, 0);
    let py = env.root().join("py");
    crate::install::write_hook(
        &py.join("pre-commit"),
        b"#!/bin/sh\necho 'pre-commit 4.6.2'\n",
    );
    let env3 = env.with_env("PATH", path_with(&py));
    let out = snp(&env3, env.root(), &["self", "alias", "status"]);
    assert_code(&out, 0);
    assert_eq!(
        text(&out.stdout),
        format!(
            "pre-commit -> {} (Python pre-commit 4.6.2)\n",
            py.join("pre-commit").display()
        )
    );
}

/// A `pre-commit` on PATH whose background grandchild keeps the stdout pipe open cannot hang the `--version`
/// probe (11 §A.19.4): at the 10 s deadline its whole process group is killed and the kind is `unknown`. Before,
/// `doctor` and `self alias status` waited for the grandchild (25 s here; forever with `sleep infinity`).
#[cfg(unix)]
#[test]
fn version_probe_is_bounded_when_a_grandchild_holds_the_pipe() {
    let env = tenv();
    let dir = env.root().join("slow");
    crate::install::write_hook(
        &dir.join("pre-commit"),
        b"#!/bin/sh\nsleep 25 &\necho 'pre-commit 4.6.2'\n",
    );
    let env = env.with_env("PATH", path_with(&dir));
    let r = crate::cli::repo_with(&env, "r", crate::cli::LOCAL_CONFIG);
    for args in [&["self", "alias", "status"][..], &["doctor"][..]] {
        let t = std::time::Instant::now();
        let out = snp(&env, &r, args);
        let took = t.elapsed();
        // 10 s deadline + SIGTERM (the group dies at once) + scheduling slack on a loaded host.
        assert!(
            took < std::time::Duration::from_secs(14),
            "{args:?}: {took:?}"
        );
        let o = text(&out.stdout);
        assert!(
            o.contains("pre-commit") && o.contains("(unknown)"),
            "{args:?}: {o}"
        );
    }
}
