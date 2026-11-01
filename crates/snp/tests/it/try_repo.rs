//! Integration tests: `try-repo` (T10-TRY) through the binary: the throwaway store (ADR-059) needs WP-31's resolver and
//! WP-33's entry, so every test is `#[ignore = "needs WP-31 and WP-33"]` until both merge.
//!
//! Owner: WP-32.

use std::path::PathBuf;

use snp_testkit::{Bins, Fixture, Output, TestEnv};

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

/// stdout without the store-dependent `[INFO]` lines.
fn no_info(out: &Output) -> String {
    let mut s = String::new();
    for l in text(&out.stdout)
        .lines()
        .filter(|l| !l.starts_with("[INFO]"))
    {
        s.push_str(l);
        s.push('\n');
    }
    s
}

/// A consumer with `test-file` staged and the hook repo `modified_file_returns_zero_repo`.
fn setup(env: &TestEnv) -> (PathBuf, PathBuf) {
    let hooks = env.fixture_repo(Fixture::Resource("modified_file_returns_zero_repo"));
    let consumer = env
        .git_repo("consumer")
        .file("test-file", b"")
        .stage("test-file")
        .build();
    (hooks, consumer)
}

#[test]
#[ignore = "needs WP-31 and WP-33"]
fn t10_try_01_all_hooks_sorted_by_id() {
    let env = env();
    let (hooks, consumer) = setup(&env);
    let out = env
        .snp_as_pre_commit(&consumer)
        .arg("try-repo")
        .arg(&hooks)
        .arg("--verbose")
        .output();
    let s = no_info(&out);
    let rule = rep("=", 79);
    assert!(
        s.starts_with(&format!(
            "{rule}\nUsing config:\n{rule}\nrepos:\n-   repo: "
        )),
        "{s}"
    );
    assert!(s.contains(
        "    hooks:\n    -   id: bash_hook\n    -   id: bash_hook2\n    -   id: bash_hook3\n"
    ));
    assert!(s.contains("Bash hook............................................(no files to check)Skipped\n- hook id: bash_hook\n"));
    assert!(s.contains("- hook id: bash_hook2\n- duration: "));
    assert!(s.contains("\n\ntest-file\n\n"));
}

#[test]
#[ignore = "needs WP-31 and WP-33"]
fn t10_try_02_one_hook_without_reading_the_manifest() {
    let env = env();
    let (hooks, consumer) = setup(&env);
    let out = env
        .snp_as_pre_commit(&consumer)
        .arg("try-repo")
        .arg(&hooks)
        .args(["bash_hook", "--verbose"])
        .output();
    let s = no_info(&out);
    assert!(
        s.contains("    hooks:\n    -   id: bash_hook\n====="),
        "{s}"
    );
    assert_eq!(s.matches("Skipped").count(), 1);
}

#[test]
#[ignore = "needs WP-31 and WP-33"]
fn t10_try_03_relative_and_bare_repos() {
    let env = env();
    let (hooks, consumer) = setup(&env);
    let relative = format!("../{}", hooks.file_name().unwrap().to_string_lossy());
    assert_eq!(
        env.snp_as_pre_commit(&consumer)
            .args(["try-repo", &relative])
            .output()
            .code,
        0
    );
    let bare = hooks.join(".git");
    assert_eq!(
        env.snp_as_pre_commit(&consumer)
            .arg("try-repo")
            .arg(&bare)
            .output()
            .code,
        0
    );
}

#[test]
#[ignore = "needs WP-31 and WP-33"]
fn t10_try_04_explicit_ref() {
    let env = env();
    let (hooks, consumer) = setup(&env);
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&hooks)
        .output()
        .unwrap();
    let sha = text(&head.stdout).trim().to_owned();
    let out = env
        .snp_as_pre_commit(&consumer)
        .arg("try-repo")
        .arg(&hooks)
        .args(["--ref", &sha])
        .output();
    assert!(text(&out.stdout).contains(&format!("    rev: {sha}\n")));
}

#[test]
#[ignore = "needs WP-31 and WP-33"]
fn t10_try_05_uncommitted_manifest_change() {
    let env = env();
    let (hooks, consumer) = setup(&env);
    let manifest = hooks.join(".pre-commit-hooks.yaml");
    let m = std::fs::read_to_string(&manifest).unwrap().replacen(
        "name: Bash hook",
        "name: modified name!",
        1,
    );
    std::fs::write(&manifest, m).unwrap();
    let out = env
        .snp_as_pre_commit(&consumer)
        .arg("try-repo")
        .arg(&hooks)
        .arg("bash_hook")
        .output();
    let s = text(&out.stdout);
    assert!(
        s.starts_with("[WARNING] Creating temporary repo with uncommitted changes...\n"),
        "{s}"
    );
    assert!(s.contains("shadow-repo\n"));
    assert!(no_info(&out).ends_with(
        "modified name!...........................................................Passed\n"
    ));
}

#[test]
#[ignore = "needs WP-31 and WP-33"]
fn t10_try_06_staged_files_in_the_hook_repo() {
    let env = env();
    let (hooks, consumer) = setup(&env);
    std::fs::write(hooks.join("new-file"), b"x").unwrap();
    std::process::Command::new("git")
        .args(["add", "new-file"])
        .current_dir(&hooks)
        .output()
        .unwrap();
    assert_eq!(
        env.snp_as_pre_commit(&consumer)
            .arg("try-repo")
            .arg(&hooks)
            .output()
            .code,
        0
    );
}

#[test]
#[ignore = "needs WP-31 and WP-33"]
fn t10_try_07_repo_relative_to_the_root_from_a_subdirectory() {
    let env = env();
    let (_, consumer) = setup(&env);
    let hooks = env.fixture_repo(Fixture::Resource("script_hooks_repo"));
    std::fs::rename(&hooks, consumer.join("foo")).unwrap();
    std::fs::create_dir_all(consumer.join("sub")).unwrap();
    let out = env
        .snp_as_pre_commit(&consumer.join("sub"))
        .args(["try-repo", "../foo"])
        .output();
    assert!(
        text(&out.stdout).contains("-   repo: foo\n"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
#[ignore = "needs WP-31 and WP-33"]
fn try_repo_twice_initializes_and_installs_each_time() {
    let env = env();
    let (hooks, consumer) = setup(&env);
    for _ in 0..2 {
        let out = env
            .snp_as_pre_commit(&consumer)
            .arg("try-repo")
            .arg(&hooks)
            .output();
        assert!(
            text(&out.stdout).contains("[INFO] Initializing environment for "),
            "{}",
            text(&out.stdout)
        );
    }
    // the main store gains no repos-v1 entry
    let repos = env.root().join("snp-store").join("repos-v1");
    assert!(!repos.exists() || std::fs::read_dir(&repos).unwrap().next().is_none());
}

#[test]
#[ignore = "needs WP-31 and WP-33"]
fn x13_git_03_bad_repo() {
    let env = env();
    let (_, consumer) = setup(&env);
    let out = env
        .snp_as_pre_commit(&consumer)
        .args(["try-repo", "/nonexistent/repo"])
        .output();
    assert_eq!(out.code, 3);
    assert!(text(&out.stdout).contains("'ls-remote', '--exit-code', '/nonexistent/repo', 'HEAD')"));
}
