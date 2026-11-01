//! Integration tests: `hook-impl` through installed git hooks (T10-HKI, X13-HI). Every row also runs through
//! `snp_run::hook_impl` in `crates/snp-run/tests/pipeline/hook_impl.rs`; these add `install` (WP-33) and real git
//! triggers.
//!
//! Owner: WP-32.

use std::fmt::Write as _;
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

fn git(dir: &std::path::Path, env: &TestEnv, args: &[&str]) -> std::process::Output {
    let mut c = std::process::Command::new("git");
    c.args(args).current_dir(dir).env_clear();
    for (k, v) in env.env().iter() {
        c.env(k, v);
    }
    c.output().unwrap()
}

const SAMPLE: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: do_not_commit\n        name: Block if \"DO NOT COMMIT\" is found\n        entry: DO NOT COMMIT\n        language: pygrep\n";

#[test]
fn t10_hki_08_git_commit_with_nothing_staged() {
    let env = env();
    let dir = repo(&env, "h", SAMPLE);
    let out = env
        .snp_as_pre_commit(&dir)
        .args([
            "hook-impl",
            "--config=.pre-commit-config.yaml",
            "--hook-type=pre-commit",
            "--",
        ])
        .output();
    assert_eq!(out.code, 0);
    assert_eq!(
        text(&out.stdout),
        "Block if \"DO NOT COMMIT\" is found....................(no files to check)Skipped\n"
    );
}

#[test]
fn hook_type_matrix_through_git() {
    let env = env();
    let mut cfg = String::from("repos:\n-   repo: local\n    hooks:\n");
    for t in [
        "pre-commit",
        "commit-msg",
        "prepare-commit-msg",
        "post-commit",
        "post-checkout",
    ] {
        writeln!(
            cfg,
            "    -   id: {t}\n        name: {t}\n        entry: 'true'\n        language: system\n        stages: [{t}]\n        always_run: true"
        )
        .unwrap();
    }
    let dir = repo(&env, "m", &cfg);
    let out = env
        .snp_as_pre_commit(&dir)
        .args([
            "install",
            "-t",
            "pre-commit",
            "-t",
            "commit-msg",
            "-t",
            "prepare-commit-msg",
            "-t",
            "post-commit",
            "-t",
            "post-checkout",
        ])
        .output();
    assert_eq!(out.code, 0, "{}", text(&out.stdout));
    std::fs::write(dir.join("a"), b"a").unwrap();
    git(&dir, &env, &["add", "a"]);
    let c = git(&dir, &env, &["commit", "-m", "msg"]);
    let all = text(&c.stdout) + &text(&c.stderr);
    for t in [
        "pre-commit",
        "commit-msg",
        "prepare-commit-msg",
        "post-commit",
    ] {
        assert!(all.contains(&line(t, "Passed")), "{t}: {all}");
    }
    let c = git(&dir, &env, &["checkout", "-b", "x"]);
    assert!((text(&c.stdout) + &text(&c.stderr)).contains(&line("post-checkout", "Passed")));
}

#[test]
fn snp_0_still_runs_a_git_lfs_like_legacy_pre_push_hook() {
    let env = env();
    let dir = repo(&env, "k", SAMPLE);
    let hooks = dir.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let marker = dir.join("lfs-ran");
    std::fs::write(
        hooks.join("pre-push.legacy"),
        format!("#!/bin/sh\ncat >/dev/null\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::PermissionsExt::set_mode(
        &mut std::fs::metadata(hooks.join("pre-push.legacy"))
            .unwrap()
            .permissions(),
        0o755,
    );
    let out = env
        .snp(&dir)
        .env("SNP", "0")
        .args([
            "hook-impl",
            "--config=.pre-commit-config.yaml",
            "--hook-type=pre-push",
            "--hook-dir",
        ])
        .arg(&hooks)
        .args(["--", "origin", "url"])
        .stdin(b"")
        .output();
    assert_eq!((out.code, text(&out.stdout)), (0, String::new()));
}
