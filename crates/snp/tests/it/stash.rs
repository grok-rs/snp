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

// POSIX only: the interrupt is `kill -INT <pid>` (Git for Windows' MSYS `kill` cannot signal a native Windows
// process). The Windows console CTRL_C path is snp-proc's `windows_ctrl_c_reaches_inherit_children_only`.
#[cfg(unix)]
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

/// A run killed with SIGKILL while its stash is held leaves the patch; the next stashing run reports it exactly once
/// on stderr (ARCHITECTURE §4.6: the scan inside `snp_stash::enter` is the only one of a stashing run).
#[cfg(unix)]
#[test]
fn an_orphaned_stash_is_reported_once() {
    let env = env();
    let dir = dirty(
        &env,
        "repos:\n-   repo: local\n    hooks:\n    -   id: slow\n        name: slow\n        entry: sh -c 'test -e .second-run || { touch .hook-started; exec sleep 30; }' --\n        language: system\n",
    );
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_snp"))
        .arg("run")
        .current_dir(&dir)
        .env_clear()
        .envs(env.env().iter())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while !dir.join(".hook-started").exists() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "the hook never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    std::fs::write(dir.join(".second-run"), b"").unwrap();
    let out = env.snp(&dir).arg("run").output();
    let err = text(&out.stderr);
    let notices = err
        .matches("an earlier run was interrupted before it restored your unstaged changes")
        .count();
    assert_eq!(notices, 1, "{err}");
}

/// The hook of [`orphaned`]: on the first run it marks its start and sleeps until killed; later runs pass.
#[cfg(unix)]
const ORPHAN_CFG: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: slow\n        name: slow\n        entry: sh -c 'test -e .second-run || { touch .hook-started; exec sleep 30; }' --\n        language: system\n";

/// A [`dirty`] repository whose first `snp run` was killed with SIGKILL while its stash was held: the worktree is at
/// the staged content and the unstaged line is only in the orphaned patch.
#[cfg(unix)]
fn orphaned(env: &TestEnv) -> PathBuf {
    let dir = dirty(env, ORPHAN_CFG);
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_snp"))
        .arg("run")
        .current_dir(&dir)
        .env_clear()
        .envs(env.env().iter())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while !dir.join(".hook-started").exists() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "the hook never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    std::fs::write(dir.join(".second-run"), b"").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "a\nstaged\n"
    );
    dir
}

const NOTICE: &str = "an earlier run was interrupted before it restored your unstaged changes";

/// The shell command of the recovery notice in `stderr` (the text after `Restore with: ` on its line).
#[cfg(unix)]
fn restore_command(stderr: &str) -> String {
    let line = stderr
        .lines()
        .find(|l| l.contains(NOTICE))
        .unwrap_or_else(|| panic!("no notice in {stderr}"));
    let cmd = line.split_once("Restore with: ").unwrap().1;
    // The command proper; the rest is what to do when the apply refuses (ADR-084).
    cmd.split("; if git apply reports")
        .next()
        .unwrap()
        .to_owned()
}

/// ADR-083: a run that ends in a preflight check before the stash (here: an unstaged config) or that does not stash
/// (`--all-files`) still reports the orphan, exactly once each.
#[cfg(unix)]
#[test]
fn an_orphaned_stash_is_reported_once_on_every_path() {
    let env = env();
    let dir = orphaned(&env);
    let mut cfg = ORPHAN_CFG.to_owned();
    cfg.push_str("# edited\n");
    std::fs::write(dir.join(".pre-commit-config.yaml"), cfg).unwrap();
    let out = env.snp(&dir).arg("run").output();
    let err = text(&out.stderr);
    assert_eq!(out.code, 1, "{err}");
    assert!(
        text(&out.stdout).contains("Your pre-commit configuration is unstaged."),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(err.matches(NOTICE).count(), 1, "{err}");
    std::fs::write(dir.join(".pre-commit-config.yaml"), ORPHAN_CFG).unwrap();
    let out = env.snp(&dir).args(["run", "--all-files"]).output();
    let err = text(&out.stderr);
    assert_eq!(out.code, 0, "{err}");
    assert_eq!(err.matches(NOTICE).count(), 1, "{err}");
    assert_eq!(err.matches("snp doctor --fix --stash").count(), 1, "{err}");
}

/// ADR-083: once the notice's own command has put the changes back, the next run retires the journal with a
/// one-time notice and later runs are silent; the patch file is kept.
#[cfg(unix)]
#[test]
fn following_the_recovery_notice_retires_it() {
    let env = env();
    let dir = orphaned(&env);
    let out = env.snp(&dir).arg("run").output();
    let err = text(&out.stderr);
    assert_eq!(err.matches(NOTICE).count(), 1, "{err}");
    let cmd = restore_command(&err);
    let patch = err
        .split_once("they are saved in ")
        .unwrap()
        .1
        .split_once(" (index tree")
        .unwrap()
        .0
        .to_owned();
    let st = std::process::Command::new("sh")
        .args(["-c", &cmd])
        .current_dir(&dir)
        .env_clear()
        .envs(env.env().iter())
        .status()
        .unwrap();
    assert!(st.success(), "{cmd}");
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "a\nstaged\nunstaged\n"
    );
    let out = env.snp(&dir).arg("run").output();
    let err = text(&out.stderr);
    assert_eq!(out.code, 0, "{err}");
    assert_eq!(err.matches(NOTICE).count(), 0, "{err}");
    assert_eq!(
        err.matches(
            "are back in the worktree; the recovery notice of that interrupted run is retired"
        )
        .count(),
        1,
        "{err}"
    );
    let out = env.snp(&dir).arg("run").output();
    let err = text(&out.stderr);
    assert!(!err.contains("snp: "), "{err}");
    assert!(std::path::Path::new(&patch).is_file(), "{patch}");
    // The unstaged line survived both runs.
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "a\nstaged\nunstaged\n"
    );
}

/// ADR-083: `snp doctor --fix --stash` dismisses a notice whose changes are not back, keeps and names the patch, and
/// the patch still restores the work afterwards.
#[cfg(unix)]
#[test]
fn doctor_fix_stash_dismisses_the_notice_and_keeps_the_patch() {
    let env = env();
    let dir = orphaned(&env);
    let out = env.snp(&dir).args(["doctor", "--stash"]).output();
    let stdout = text(&out.stdout);
    assert!(stdout.contains("unrestored changes"), "{stdout}");
    let out = env.snp(&dir).args(["doctor", "--fix", "--stash"]).output();
    let stdout = text(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.contains("recovery notice dismissed"))
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(line.contains("NOT in the worktree"), "{line}");
    assert!(!stdout.contains("unrestored changes"), "{stdout}");
    let patch = line
        .split_once("git apply --whitespace=nowarn ")
        .unwrap()
        .1
        .split_once(" still")
        .unwrap()
        .0
        .to_owned();
    assert!(std::path::Path::new(&patch).is_file(), "{patch}");
    let out = env.snp(&dir).arg("run").output();
    let err = text(&out.stderr);
    assert_eq!(out.code, 0, "{err}");
    assert!(!err.contains("snp: "), "{err}");
    let st = std::process::Command::new("git")
        .args(["apply", "--whitespace=nowarn", &patch])
        .current_dir(&dir)
        .env_clear()
        .envs(env.env().iter())
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "a\nstaged\nunstaged\n"
    );
}
