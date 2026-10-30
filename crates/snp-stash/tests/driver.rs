//! The process-level matrices through `snp-stash-driver` (feature `test-driver`, posix): the conflict transcript
//! against the pre-commit 4.6.2 golden, SIGTERM/SIGHUP during hooks (sent and kernel origin), SIGTERM at each
//! transition, ^C with a slow `git apply` across the 2 s grace, a second ^C during `git apply`, the kill -9 matrix
//! (an abort at every transition) with recovery through the pointer, and a full disk at the patch and journal writes.
//!
//! Run with `cargo test -p snp-stash --features test-driver --test driver`.
//!
//! Owner: WP-23.

#[cfg(test)]
#[expect(
    dead_code,
    reason = "shared with the catalog binary; this binary uses a subset"
)]
mod common;

#[cfg(test)]
#[cfg(unix)]
mod process {
    use std::ffi::OsStr;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitStatus, Output, Stdio};
    use std::time::{Duration, Instant};

    use rustix::process::{Pid, Signal};

    use crate::common::{FOO, Sandbox};

    const DRIVER: &str = env!("CARGO_BIN_EXE_snp-stash-driver");

    /// A repository: `foo` staged + an unstaged `1`→`a`, a staged `fixme`, an intent-to-add `n`.
    fn scenario(sb: &Sandbox) -> PathBuf {
        let repo = sb.init("r");
        sb.write(&repo, "foo", FOO.as_bytes());
        sb.write(&repo, "fixme", b"x  \n");
        sb.write(&repo, "n", b"new\n");
        sb.git(&repo, &["add", "foo", "fixme"]);
        sb.git(&repo, &["add", "--intent-to-add", "n"]);
        sb.write(&repo, "foo", FOO.replace('1', "a").as_bytes());
        repo
    }

    fn driver(sb: &Sandbox, repo: &Path, args: &[&str], hook: &[&str]) -> Command {
        let mut c = Command::new(DRIVER);
        c.env_clear().envs(sb.env.iter()).current_dir(repo);
        c.arg(repo)
            .arg(sb.patches())
            .args(args)
            .arg("--")
            .args(hook);
        c
    }

    /// `PATH` with a `git` wrapper first: `apply` sleeps `slow` seconds after touching `<root>/apply-started`;
    /// `SIGTERM_AT=<subcommand>` sends SIGTERM to the driver (the wrapper's parent) before that command runs.
    fn wrapper_path(sb: &Sandbox) -> std::ffi::OsString {
        let dir = sb.path("wrap");
        std::fs::create_dir_all(&dir).unwrap();
        let real =
            snp_proc::resolve::find_executable(OsStr::new("git"), &sb.env, &sb.root).unwrap();
        let script = format!(
            "#!/bin/sh\nsub=$1\n[ \"$1\" = -c ] && sub=$3\nif [ -n \"${{SIGTERM_AT:-}}\" ] && [ \"$sub\" = \"$SIGTERM_AT\" ]; then kill -TERM $PPID; sleep 0.2; fi\n\
             if [ \"$sub\" = apply ] && [ -n \"${{SLOW_APPLY:-}}\" ]; then touch '{}'; sleep \"$SLOW_APPLY\"; fi\nexec '{}' \"$@\"\n",
            sb.path("apply-started").display(),
            real.display()
        );
        let git = dir.join("git");
        std::fs::write(&git, script).unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut path = dir.into_os_string();
        path.push(":");
        path.push(sb.env.get("PATH").unwrap());
        path
    }

    fn wait_for(p: &Path) {
        let t = Instant::now();
        while !p.exists() {
            assert!(
                t.elapsed() < Duration::from_secs(30),
                "timed out waiting for {}",
                p.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish(child: Child) -> Output {
        child.wait_with_output().unwrap()
    }

    fn text(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    /// The patch path printed by `[INFO] Stashing unstaged files to <p>.`.
    fn patch_in(out: &str) -> Option<PathBuf> {
        out.lines().find_map(|l| {
            l.split("Stashing unstaged files to ")
                .nth(1)
                .and_then(|r| r.strip_suffix('.'))
                .map(PathBuf::from)
        })
    }

    fn assert_restored(sb: &Sandbox, repo: &Path) {
        assert_eq!(sb.read(repo, "foo"), FOO.replace('1', "a").as_bytes());
        assert_eq!(sb.status_of(repo, "n"), "A");
        assert_eq!(sb.git(repo, &["show", ":foo"]), FOO.as_bytes());
    }

    fn killed_by(status: ExitStatus, sig: i32) {
        assert_eq!(status.signal(), Some(sig), "{status:?}");
    }

    #[test]
    fn conflict_transcript_stash_lines_equal_pre_commit() {
        for (color, golden) in [(false, "conflict.txt"), (true, "conflict.color.txt")] {
            let sb = Sandbox::new();
            let repo = sb.foo_staged("r");
            sb.write(&repo, "foo", FOO.replace('1', "a").as_bytes());
            let args: &[&str] = if color { &["--color"] } else { &[] };
            let out = driver(&sb, &repo, args, &["sed", "-i", "s/1/b/", "foo"])
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", text(&out.stderr));
            let stdout = text(&out.stdout);
            let patch = patch_in(&stdout).unwrap();
            let got = stdout.replace(&*patch.to_string_lossy(), "<PATCH>");
            let want = std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/goldens")
                    .join(golden),
            )
            .unwrap();
            assert_eq!(got, want, "color={color}");
            assert_eq!(sb.read(&repo, "foo"), FOO.replace('1', "a").as_bytes());
        }
    }

    fn signal_during_hook(sig: Signal, raw: i32) {
        let sb = Sandbox::new();
        let repo = scenario(&sb);
        let marker = sb.path("hook-started");
        let script = format!("touch '{}'; exec sleep 30", marker.display());
        let child = driver(&sb, &repo, &[], &["sh", "-c", &script])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        wait_for(&marker);
        rustix::process::kill_process(Pid::from_child(&child), sig).unwrap();
        let t = Instant::now();
        let out = finish(child);
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "the hook was not stopped"
        );
        killed_by(out.status, raw);
        let stdout = text(&out.stdout);
        assert!(stdout.contains("[INFO] Restored changes from "), "{stdout}");
        assert!(!stdout.contains("Interrupted"), "no banner for {sig:?}");
        assert_restored(&sb, &repo);
    }

    #[test]
    fn sigterm_sent_during_hooks_restores_and_dies_by_signal() {
        signal_during_hook(Signal::TERM, 15);
    }

    #[test]
    fn sighup_sent_during_hooks_restores_and_dies_by_signal() {
        signal_during_hook(Signal::HUP, 1);
    }

    #[test]
    fn sighup_from_a_terminal_hangup_restores_and_dies_by_signal() {
        use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
        let Ok(master) = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
        else {
            return; // no pty support in this sandbox
        };
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let slave_path = ptsname(&master, Vec::new()).unwrap();
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(OsStr::new(&*slave_path.to_string_lossy()))
            .unwrap();
        let sb = Sandbox::new();
        let repo = scenario(&sb);
        let marker = sb.path("hook-started");
        let script = format!("touch '{}'; exec sleep 30", marker.display());
        // `setsid --ctty`: a new session whose controlling terminal is the pty (the child is not a group leader,
        // so setsid execs the driver in place).
        let mut c = Command::new("setsid");
        c.arg("--ctty")
            .arg(DRIVER)
            .env_clear()
            .envs(sb.env.iter())
            .current_dir(&repo);
        c.arg(&repo)
            .arg(sb.patches())
            .arg("--")
            .args(["sh", "-c", &script]);
        c.stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave);
        let child = c.spawn().unwrap();
        wait_for(&marker);
        // Hang up: close the master — the kernel sends SIGHUP to the session (kernel origin).
        drop(master);
        let out = finish(child);
        killed_by(out.status, 1);
        assert_restored(&sb, &repo);
    }

    #[test]
    fn sigterm_at_each_transition_restores_the_pre_run_state() {
        for at in ["write-tree", "diff-index", "checkout", "apply", "add"] {
            let sb = Sandbox::new();
            let repo = scenario(&sb);
            let index = sb.index(&repo);
            let diff = sb.unstaged_diff(&repo);
            let out = driver(&sb, &repo, &[], &["true"])
                .env("PATH", wrapper_path(&sb))
                .env("SIGTERM_AT", at)
                .output()
                .unwrap();
            killed_by(out.status, 15);
            assert_eq!(
                sb.unstaged_diff(&repo),
                diff,
                "SIGTERM at {at}: {}",
                text(&out.stdout)
            );
            assert_eq!(sb.index(&repo), index, "SIGTERM at {at}");
        }
    }

    #[test]
    fn ctrl_c_with_a_quick_hook_never_signals_a_slow_apply() {
        let sb = Sandbox::new();
        let repo = scenario(&sb);
        let marker = sb.path("hook-started");
        let script = format!("touch '{}'; exec sleep 30", marker.display());
        let child = driver(&sb, &repo, &[], &["sh", "-c", &script])
            .env("PATH", wrapper_path(&sb))
            .env("SLOW_APPLY", "3")
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        wait_for(&marker);
        let pgid = Pid::from_child(&child);
        // Terminal ^C: the whole foreground group (driver + hook).
        rustix::process::kill_process_group(pgid, Signal::INT).unwrap();
        let t = Instant::now();
        let out = finish(child);
        // The apply slept 3 s, past the 2 s kill grace, and still completed.
        assert!(t.elapsed() >= Duration::from_secs(3));
        assert_eq!(out.status.code(), Some(130), "{}", text(&out.stderr));
        let stdout = text(&out.stdout);
        assert!(stdout.contains("[INFO] Restored changes from "), "{stdout}");
        assert!(
            stdout.ends_with("Interrupted (^C): KeyboardInterrupt: \n"),
            "{stdout}"
        );
        assert_restored(&sb, &repo);
    }

    #[test]
    fn a_second_ctrl_c_during_git_apply_never_interrupts_it() {
        let sb = Sandbox::new();
        let repo = scenario(&sb);
        let hook_marker = sb.path("hook-started");
        let script = format!("touch '{}'; exec sleep 30", hook_marker.display());
        let child = driver(&sb, &repo, &[], &["sh", "-c", &script])
            .env("PATH", wrapper_path(&sb))
            .env("SLOW_APPLY", "2")
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        wait_for(&hook_marker);
        let pgid = Pid::from_child(&child);
        rustix::process::kill_process_group(pgid, Signal::INT).unwrap();
        wait_for(&sb.path("apply-started"));
        for _ in 0..3 {
            rustix::process::kill_process_group(pgid, Signal::INT).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        let out = finish(child);
        assert_eq!(out.status.code(), Some(130));
        let stderr = text(&out.stderr);
        assert_eq!(
            stderr
                .matches("snp: restoring stashed changes, please wait")
                .count(),
            1,
            "{stderr}"
        );
        assert_restored(&sb, &repo);
    }

    /// The kill -9 matrix: an abort (no unwinding, no destructors) right after each transition.
    #[test]
    fn kill_9_at_every_transition_leaves_a_recoverable_state() {
        for n in 1..=10_u8 {
            let sb = Sandbox::new();
            let repo = scenario(&sb);
            let out = driver(
                &sb,
                &repo,
                &["--abort-on-fault", "--stage-fixed", "fixme"],
                &["sed", "-i", "s/  $//", "fixme"],
            )
            .env("SNP_FAULT", format!("StashState({n})"))
            .output()
            .unwrap();
            if n == 8 {
                // The rollback transition: not reached without a conflict (see the conflict case below).
                assert!(out.status.success());
                continue;
            }
            killed_by(out.status, 6);
            // Recovery from any store: the pointer leads to the journal.
            let other = sb.path("other-store/patches");
            let found = snp_stash::recovery_scan(&repo.join(".git"), &other, &repo);
            match n {
                // Before the checkout or after the restore: the user's change is still (or again) in the worktree.
                1..=4 | 9 | 10 => {
                    assert_eq!(
                        sb.read(&repo, "foo"),
                        FOO.replace('1', "a").as_bytes(),
                        "StashState({n})"
                    );
                    if n == 4 {
                        assert_eq!(found.len(), 1, "the durable patch is reported");
                    } else {
                        assert!(found.is_empty(), "StashState({n}): {found:?}");
                    }
                }
                _ => {
                    assert_eq!(found.len(), 1, "StashState({n})");
                    let u = &found[0];
                    // The next run prints exactly `hint()` on stderr.
                    let next = driver(&sb, &repo, &[], &[]).output().unwrap();
                    assert_eq!(
                        text(&next.stderr),
                        format!("{}\n", u.hint()),
                        "StashState({n})"
                    );
                    // Following the hint (after discarding the hook fixes) restores the work.
                    sb.git(&repo, &["checkout", "--", "foo"]);
                    let cmds = u.hint().split("Restore with: ").nth(1).unwrap().to_owned();
                    let st = Command::new("sh")
                        .arg("-c")
                        .arg(&cmds)
                        .current_dir(&repo)
                        .env_clear()
                        .envs(sb.env.iter())
                        .status()
                        .unwrap();
                    assert!(st.success(), "StashState({n}): {cmds}");
                    assert_eq!(
                        sb.read(&repo, "foo"),
                        FOO.replace('1', "a").as_bytes(),
                        "StashState({n})"
                    );
                    assert_eq!(
                        sb.git(&repo, &["show", ":fixme"]),
                        b"x  \n",
                        "StashState({n}): the staged fix is reset"
                    );
                }
            }
        }
    }

    fn with_file_limit(sb: &Sandbox, repo: &Path, blocks: u32) -> Output {
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg(format!(
                "trap '' XFSZ; ulimit -f {blocks}; exec \"$0\" \"$@\""
            ))
            .arg(DRIVER)
            .arg(repo)
            .arg(sb.patches())
            .arg("--")
            .arg("true")
            .env_clear()
            .envs(sb.env.iter())
            .current_dir(repo);
        c.output().unwrap()
    }

    #[test]
    fn full_disk_at_the_patch_write_aborts_before_any_mutation() {
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        let mut big = String::new();
        for i in 0..2000 {
            big.push_str("line ");
            big.push_str(&i.to_string());
            big.push('\n');
        }
        sb.write(&repo, "foo", big.as_bytes());
        let out = with_file_limit(&sb, &repo, 1);
        assert_eq!(out.status.code(), Some(3), "{}", text(&out.stdout));
        assert!(
            text(&out.stdout).starts_with(
                "An unexpected error has occurred: OSError: [Errno 27] File too large: "
            )
        );
        assert_eq!(sb.read(&repo, "foo"), big.as_bytes());
        assert!(
            sb.patch_files().iter().all(|f| !f.starts_with("patch")),
            "{:?}",
            sb.patch_files()
        );
    }

    #[test]
    fn kill_9_while_rolling_back_is_recoverable() {
        let sb = Sandbox::new();
        let repo = scenario(&sb);
        let out = driver(
            &sb,
            &repo,
            &["--abort-on-fault"],
            &["sed", "-i", "s/1/b/", "foo"],
        )
        .env("SNP_FAULT", "StashState(8)")
        .output()
        .unwrap();
        killed_by(out.status, 6);
        let found = snp_stash::recovery_scan(&repo.join(".git"), &sb.path("x/patches"), &repo);
        assert_eq!(
            (found.len(), found.first().map(|u| &*u.state)),
            (1, Some("rolling-back"))
        );
        sb.git(&repo, &["checkout", "--", "foo"]);
        sb.git(
            &repo,
            &[
                OsStr::new("apply"),
                OsStr::new("--3way"),
                found[0].patch.as_os_str(),
            ],
        );
        assert_eq!(sb.read(&repo, "foo"), FOO.replace('1', "a").as_bytes());
    }

    #[test]
    fn full_disk_at_the_journal_write_aborts_before_any_mutation() {
        let sb = Sandbox::new();
        // Long absolute paths make the journal larger than the 512-byte limit; the patch stays below it.
        let repo =
            sb.foo_staged(&std::iter::repeat_n("long-worktree-name-", 12).collect::<String>());
        sb.write(&repo, "foo", b"x\n");
        // The patch (< 512 bytes) fits; the journal (absolute paths, token, …) does not.
        let out = with_file_limit(&sb, &repo, 1);
        assert_eq!(out.status.code(), Some(3), "{}", text(&out.stdout));
        assert!(text(&out.stdout).contains("File too large"));
        assert_eq!(sb.read(&repo, "foo"), b"x\n");
        assert!(!repo.join(".git/snp-stash.journal").exists());
    }
}
