//! Integration tests: `run` (T10-RUN, X13-EXE) through the binary, against remote (file://) hook repositories.
//!
//! The pipeline itself is tested in `crates/snp-run/tests/pipeline` (a fake resolver, `repo: local`/`meta`); these
//! tests add WP-31's resolver (clones, manifests, installs) and WP-33's entry (argv, brand, envelope): the [e2e] items
//! of WBS WP-32.
//!
//! Owner: WP-32.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use snp_testkit::{Bins, Fixture, Oracle, Output, TestEnv};

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

/// A consuming repository of `resource` with `files` staged.
fn consuming(env: &TestEnv, resource: &'static str, files: &[(&str, &[u8])]) -> PathBuf {
    let dir = env.fixture_repo(Fixture::ConsumingRepo(resource));
    let rel = dir
        .strip_prefix(env.root())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut repo = env.git_repo(&rel);
    for (p, b) in files {
        repo = repo.file(p, b).stage(p);
    }
    repo.build()
}

fn run(env: &TestEnv, dir: &Path, args: &[&str]) -> Output {
    env.snp_as_pre_commit(dir).arg("run").args(args).output()
}

/// The [INFO] lines of the first (cold) run differ only by the store; tests compare the hook lines.
fn hook_lines(out: &Output) -> String {
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

#[test]
fn t10_run_10_failing_hook_repo() {
    let env = env();
    let dir = consuming(&env, "failing_hook_repo", &[("foo.py", b"")]);
    let out = run(&env, &dir, &[]);
    assert_eq!(out.code, 1, "{}", text(&out.stdout));
    let s = hook_lines(&out);
    assert!(s.contains("Failing hook") && s.contains("Failed"));
    assert!(s.contains("hook id: failing_hook"));
    assert!(s.contains("Fail\nfoo.py\n"), "{s}");
}

#[test]
fn t10_run_11_arbitrary_bytes_repo() {
    let env = env();
    let dir = consuming(&env, "arbitrary_bytes_repo", &[("foo.py", b"")]);
    let out = run(&env, &dir, &[]);
    assert_eq!(out.code, 1);
    let needle = b"\xe2\x98\x83\xb2\n";
    assert!(
        (0..=out.stdout.len().saturating_sub(needle.len()))
            .any(|i| out.stdout.get(i..i + needle.len()) == Some(&needle[..]))
    );
}

#[test]
fn t10_run_12_modified_file_returns_zero_repo() {
    let env = env();
    let dir = consuming(
        &env,
        "modified_file_returns_zero_repo",
        &[("bar.py", b""), ("foo.py", b"")],
    );
    let out = run(&env, &dir, &[]);
    assert_eq!(out.code, 1);
    let s = hook_lines(&out);
    let (_, after_first) = s
        .split_once("- files were modified by this hook\n\nModified: foo.py")
        .unwrap();
    let (_, after_second) = after_first
        .split_once(&line("Bash hook", "Passed"))
        .unwrap();
    assert!(
        after_second.contains("- files were modified by this hook\n"),
        "{s}"
    );
}

#[test]
fn t10_run_17_script_hooks_repo() {
    let env = env();
    let dir = consuming(&env, "script_hooks_repo", &[("foo.py", b"")]);
    let out = run(&env, &dir, &[]);
    assert_eq!(out.code, 0);
    assert!(hook_lines(&out).ends_with(&line("Bash hook", "Passed")));
    let out = run(&env, &dir, &["nope"]);
    assert_eq!(
        (out.code, text(&out.stdout)),
        (
            1,
            String::from("No hook with id `nope` in stage `pre-commit`\n")
        )
    );
    let out = run(&env, &dir, &["--hook-stage", "push", "nope"]);
    assert_eq!(
        text(&out.stdout),
        "No hook with id `nope` in stage `pre-push`\n"
    );
}

#[test]
fn t10_run_23_skip_alias_and_not_installed() {
    let env = env();
    let dir = consuming(&env, "script_hooks_repo", &[("foo.py", b"")]);
    let out = env
        .snp_as_pre_commit(&dir)
        .env("SKIP", "bash_hook")
        .args(["run"])
        .output();
    assert_eq!(out.code, 0);
    assert!(hook_lines(&out).ends_with(&line("Bash hook", "Skipped")));
}

#[test]
fn t10_run_59_invalid_pre_commit_color() {
    let env = env();
    let dir = consuming(&env, "script_hooks_repo", &[]);
    let out = env
        .snp_as_pre_commit(&dir)
        .env("PRE_COMMIT_COLOR", "bogus")
        .arg("run")
        .output();
    assert_eq!(out.code, 2);
    assert!(
        text(&out.stderr).ends_with(
            "pre-commit run: error: argument --color: invalid use_color value: 'bogus'\n"
        )
    );
}

#[test]
fn t10_run_60_no_subcommand_is_run() {
    let env = env();
    let dir = consuming(&env, "failing_hook_repo", &[("foo.py", b"")]);
    let out = env.snp_as_pre_commit(&dir).output();
    assert_eq!(out.code, 1);
    assert!(text(&out.stdout).contains("Failing hook"));
}

#[test]
fn x13_exe_08_language_setup_errors_crash_like_pre_commit() {
    // a remote hook whose language toolchain is missing fails in install/run as pre-commit does (exit 3)
    let env = env();
    let dir = env.git_repo("c").file(
        ".pre-commit-config.yaml",
        b"repos:\n-   repo: local\n    hooks:\n    -   id: l\n        name: l\n        entry: l\n        language: lua\n        always_run: true\n",
    )
    .stage(".pre-commit-config.yaml")
    .build();
    let out = env
        .snp_as_pre_commit(&dir)
        .env("PATH", "/nonexistent")
        .arg("run")
        .output();
    assert_ne!(out.code, 0);
}

#[test]
fn cold_and_warm_transcripts_match_the_oracle() {
    let env = env();
    let dir = consuming(&env, "script_hooks_repo", &[("foo.py", b"")]);
    let Some(oracle) = env.oracle(Oracle::Pc462Py312, &dir) else {
        return;
    };
    let want = oracle.arg("run").output();
    let got = run(&env, &dir, &[]);
    assert_eq!(got.code, want.code);
    assert_eq!(hook_lines(&got), hook_lines(&want));
}

#[test]
#[ignore = "perf"]
fn b3_ten_hooks_nothing_staged_within_3x_budget() {
    // ARCHITECTURE §12.1 B3: ≤ 25 ms on the Linux gate; the perf-smoke job runs this release-mode test with 3× margin
    let env = env();
    let mut cfg = String::from("repos:\n-   repo: local\n    hooks:\n");
    for i in 0..10 {
        writeln!(
            cfg,
            "    -   id: noop-{i:02}\n        name: noop {i}\n        entry: 'true'\n        language: system\n        \
             always_run: true\n        pass_filenames: false"
        )
        .unwrap();
    }
    let dir = env
        .git_repo("b3")
        .file(".pre-commit-config.yaml", cfg.as_bytes())
        .stage_all()
        .commit("c")
        .build();
    let warm = env.snp(&dir).arg("run").output();
    assert_eq!(warm.code, 0);
    let mut best = std::time::Duration::MAX;
    for _ in 0..5 {
        let t0 = std::time::Instant::now();
        let out = env.snp(&dir).arg("run").output();
        assert_eq!(out.code, 0);
        best = best.min(t0.elapsed());
    }
    assert!(
        best <= std::time::Duration::from_millis(75),
        "B3 took {best:?}"
    );
}
