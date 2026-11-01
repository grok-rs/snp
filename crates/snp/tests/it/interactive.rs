//! Integration tests: `interactive: true` hooks on a real pseudo-terminal (ADR-085). Each test starts snp (or
//! `git commit`) as the leader of a new session whose controlling terminal is a fresh pty (`snp_testkit::Session`,
//! util-linux `setsid --ctty`); the program's stdin is `/dev/null` like a git hook's, so the hook can only reach the
//! keyboard through `/dev/tty`. Tests skip where `setsid --ctty` is missing.
//!
//! Owner: WP-32 (ADR-085).

use std::path::{Path, PathBuf};
use std::time::Duration;

use snp_testkit::{Bins, Session, TestEnv};

const T: Duration = Duration::from_secs(30);

fn env() -> TestEnv {
    let snp = PathBuf::from(env!("CARGO_BIN_EXE_snp"));
    let probe = Bins::locate().map_or_else(|| snp.clone(), |b| b.probe);
    // the transcripts compare plain text (a terminal turns `--color auto` on)
    TestEnv::new(Bins { snp, probe }).with_env("PRE_COMMIT_COLOR", "never")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `s` repeated `n` times (`str::repeat` is a disallowed method).
fn rep(s: &str, n: usize) -> String {
    std::iter::repeat_n(s, n).collect()
}

/// `{name}{dots}{status}` at 80 columns.
fn line(name: &str, status: &str) -> String {
    format!(
        "{name}{}{status}\n",
        rep(".", 79 - name.chars().count() - status.chars().count())
    )
}

/// A local hook running the executable script `script` (`ask.sh`), always, without file names.
fn hook(id: &str, script: &str, extra: &str) -> String {
    format!(
        "    -   id: {id}\n        name: {id}\n        entry: ./{script}\n        language: script\n        always_run: true\n        pass_filenames: false\n{extra}"
    )
}

/// A committed repository with `hooks` as its local repo and the executable `scripts`.
fn repo(env: &TestEnv, hooks: &str, scripts: &[(&str, &str)]) -> PathBuf {
    let config = format!("repos:\n-   repo: local\n    hooks:\n{hooks}");
    let mut b = env
        .git_repo("r")
        .file(".pre-commit-config.yaml", config.as_bytes());
    for (name, body) in scripts {
        b = b.exec_file(name, body.as_bytes());
    }
    b.stage_all().commit("init").build()
}

/// `snp run <args>` in a pty session (stdin `/dev/null`), or `None` (skip) without `setsid --ctty`.
fn session(env: &TestEnv, dir: &Path, args: &[&str]) -> Option<Session> {
    let s = env.snp(dir).arg("run").args(args).session(true);
    if s.is_none() {
        eprintln!("skipped: util-linux `setsid --ctty` is not available");
    }
    s
}

const ASK: &str = "#!/bin/sh\n[ -t 0 ] && echo stdin-is-a-tty\nprintf 'name? '\nread answer\necho \"hello $answer\"\n";

/// Positions of `needles` in `hay`, which must appear in this order.
fn in_order(hay: &str, needles: &[&str]) {
    let mut at = 0;
    for n in needles {
        match hay.get(at..).and_then(|rest| rest.find(n)) {
            Some(i) => at += i + n.len(),
            None => panic!("{n:?} missing (or out of order) in:\n{hay}"),
        }
    }
}

#[test]
fn the_hook_reads_the_terminal_and_writes_to_it_directly() {
    let env = env();
    let dir = repo(
        &env,
        &hook("ask", "ask.sh", "        interactive: true\n"),
        &[("ask.sh", ASK)],
    );
    for brand in ["snp", "pre-commit"] {
        let cmd = if brand == "snp" {
            env.snp(&dir)
        } else {
            env.snp_as_pre_commit(&dir)
        };
        let Some(mut s) = cmd.args(["run", "ask"]).session(true) else {
            return;
        };
        s.expect("name? ", T);
        s.send(b"world\n");
        let out = s.wait(T);
        let t = text(&out.stdout);
        assert_eq!(out.code, 0, "{brand}: {t}");
        // the header line, the hook's own terminal I/O, then the usual status line
        assert!(t.starts_with(&line("ask", "(interactive)")), "{brand}: {t}");
        in_order(
            &t,
            &[
                "stdin-is-a-tty\n",
                "name? world\n",
                "hello world\n",
                &line("ask", "Passed"),
            ],
        );
        assert!(t.ends_with(&line("ask", "Passed")), "{brand}: {t}");
    }
}

#[test]
fn a_failing_interactive_hook_shows_no_captured_output() {
    let env = env();
    let dir = repo(
        &env,
        &hook(
            "ask",
            "ask.sh",
            "        interactive: true\n        fail_text: answer differently\n",
        ),
        &[(
            "ask.sh",
            "#!/bin/sh\nprintf 'name? '\nread answer\necho LIVE-OUTPUT\nexit 3\n",
        )],
    );
    let Some(mut s) = session(&env, &dir, &["ask"]) else {
        return;
    };
    s.expect("name? ", T);
    s.send(b"x\n");
    let out = s.wait(T);
    let t = text(&out.stdout);
    assert_eq!(out.code, 1, "{t}");
    let tail = format!(
        "LIVE-OUTPUT\n{}- hook id: ask\n- exit code: 3\n- answer differently\n",
        line("ask", "Failed")
    );
    assert!(t.ends_with(&tail), "{t}");
    assert_eq!(t.matches("LIVE-OUTPUT").count(), 1, "{t}");
}

/// A raw-mode prompt (as prompt libraries do) changes the terminal modes without being stopped by SIGTTOU.
#[test]
fn raw_mode_works_without_job_control_stops() {
    let env = env();
    let dir = repo(
        &env,
        &hook("key", "key.sh", "        interactive: true\n"),
        &[(
            "key.sh",
            "#!/bin/sh\nstty raw -echo || exit 9\nprintf 'press> '\nc=$(dd bs=1 count=1 2>/dev/null)\nstty sane\necho \"got:$c\"\n",
        )],
    );
    let Some(mut s) = session(&env, &dir, &["key"]) else {
        return;
    };
    s.expect("press> ", T);
    s.send(b"x");
    let out = s.wait(T);
    let t = text(&out.stdout);
    assert_eq!(out.code, 0, "{t}");
    in_order(&t, &["got:x\n", &line("key", "Passed")]);
}

/// ^C at the prompt reaches the hook once (the kernel's SIGINT to the foreground group; snp does not forward it
/// again), and snp ends like any interrupted run: stash restored, exit 130.
#[test]
fn ctrl_c_at_the_prompt_interrupts_once_and_restores_the_stash() {
    let env = env();
    let ints = env.root().join("ints");
    let script = "#!/bin/sh\ntrap 'echo INT >> \"$INTS\"' INT\nprintf 'prompt> '\nread answer\n# a second SIGINT would interrupt the sleep and run the trap again\nsleep 1\nexit 130\n";
    let config = format!(
        "repos:\n-   repo: local\n    hooks:\n{}",
        hook(
            "ask",
            "ask.sh",
            &format!(
                "        interactive: true\n        env: {{INTS: '{}'}}\n",
                ints.display()
            ),
        )
    );
    let dir = env
        .git_repo("r")
        .file(".pre-commit-config.yaml", config.as_bytes())
        .exec_file("ask.sh", script.as_bytes())
        .file("a.txt", b"a\n")
        .stage_all()
        .commit("init")
        .file("a.txt", b"a\nstaged\n")
        .stage("a.txt")
        .build();
    std::fs::write(dir.join("a.txt"), b"a\nstaged\nunstaged\n").unwrap();
    let Some(mut s) = session(&env, &dir, &[]) else {
        return;
    };
    s.expect("prompt> ", T);
    s.send(b"\x03");
    let out = s.wait(T);
    let t = text(&out.stdout);
    assert_eq!(out.code, 130, "{t}");
    assert!(t.contains("Interrupted (^C): KeyboardInterrupt"), "{t}");
    assert_eq!(std::fs::read_to_string(&ints).unwrap(), "INT\n", "{t}");
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "a\nstaged\nunstaged\n"
    );
}

/// `timeout` still applies: the hook is killed (the child process: it stays in snp's process group), the terminal
/// modes it changed are put back, and the hook fails with the usual line. Here snp's own stdin is the terminal.
#[test]
fn timeout_kills_an_interactive_hook_and_restores_the_terminal() {
    let env = env();
    let dir = repo(
        &env,
        &hook(
            "wait",
            "wait.sh",
            "        interactive: true\n        timeout: 1s\n",
        ),
        &[(
            "wait.sh",
            "#!/bin/sh\nstty -echo\nprintf 'wait> '\nread x\n",
        )],
    );
    let script = format!(
        "\"{}\" run wait; echo \"snp-exit:$?\"; stty -a",
        env!("CARGO_BIN_EXE_snp")
    );
    let Some(s) = env.sh(&dir, &script).session(false) else {
        eprintln!("skipped: util-linux `setsid --ctty` is not available");
        return;
    };
    s.expect("wait> ", T);
    let out = s.wait(T);
    let t = text(&out.stdout);
    in_order(
        &t,
        &[
            &line("wait", "(interactive)"),
            "wait> \n",
            &line("wait", "Failed"),
            "- hook id: wait\n- exit code: -15\n- hook timed out after 1s\n",
            "snp-exit:1\n",
        ],
    );
    let echo = t
        .rsplit("snp-exit:1\n")
        .next()
        .unwrap()
        .split_whitespace()
        .any(|m| m == "echo");
    assert!(echo, "echo restored: {t}");
}

/// No terminal at all (CI, cron): the hook runs like any other, captured, after a one-line stderr note.
#[test]
fn without_a_terminal_the_hook_runs_captured_with_a_note() {
    let env = env();
    let dir = repo(
        &env,
        &hook(
            "ask",
            "ask.sh",
            "        interactive: true\n        verbose: true\n",
        ),
        &[("ask.sh", ASK)],
    );
    let Some(cmd) = env.snp(&dir).args(["run", "ask"]).without_terminal() else {
        eprintln!("skipped: util-linux `setsid` is not available");
        return;
    };
    let out = cmd.output();
    assert_eq!(out.code, 0, "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stderr),
        "snp: interactive: no terminal is available; running 'ask' without a terminal\n"
    );
    // stdin is the usual empty pipe; the output is captured into the (verbose) block
    assert_eq!(
        text(&out.stdout),
        format!(
            "{}- hook id: ask\n- duration: 0s\n\nname? hello\n\n",
            line("ask", "Passed")
        )
        .replace("0s", &duration(&out.stdout))
    );
}

/// The `- duration: …s` value of a verbose block.
fn duration(stdout: &[u8]) -> String {
    let t = text(stdout);
    let d = t
        .lines()
        .find_map(|l| l.strip_prefix("- duration: "))
        .unwrap_or("0s");
    d.to_owned()
}

/// `SNP_NO_TTY=1` opts out (lefthook's `no_tty`): a terminal is there, but the hook runs captured, silently.
#[test]
fn snp_no_tty_runs_interactive_hooks_captured() {
    let env = env().with_env("SNP_NO_TTY", "1");
    let dir = repo(
        &env,
        &hook("ask", "ask.sh", "        interactive: true\n"),
        &[(
            "ask.sh",
            "#!/bin/sh\n[ -t 0 ] && echo stdin-is-a-tty\nread answer\necho \"captured:$answer\"\nexit 1\n",
        )],
    );
    let Some(s) = session(&env, &dir, &["ask"]) else {
        return;
    };
    let out = s.wait(T);
    let t = text(&out.stdout);
    assert_eq!(out.code, 1, "{t}");
    assert_eq!(
        t,
        format!(
            "{}- hook id: ask\n- exit code: 1\n\ncaptured:\n\n",
            line("ask", "Failed")
        )
    );
}

/// The scheduler (ADR-085): an interactive hook is a bucket of its own — the hook before it in its priority has
/// finished, the one after it has not started — and the blocks of hooks that finished earlier (held back by
/// `priority`'s config-order output) print before its line; its own block prints as soon as it ends.
#[test]
fn an_interactive_hook_runs_alone_after_earlier_output_is_flushed() {
    let env = env();
    let hooks = [
        hook(
            "slow",
            "mark.sh",
            "        args: [slow]\n        priority: 5\n",
        ),
        hook("early", "fail.sh", "        priority: 0\n"),
        hook(
            "before",
            "mark.sh",
            "        args: [before]\n        priority: 1\n",
        ),
        hook(
            "ask",
            "ask.sh",
            "        interactive: true\n        priority: 1\n",
        ),
        hook(
            "after",
            "mark.sh",
            "        args: [after]\n        priority: 1\n",
        ),
    ]
    .concat();
    let dir = repo(
        &env,
        &hooks,
        &[
            ("mark.sh", "#!/bin/sh\nsleep 0.3\n: > \"../$1.done\"\n"),
            ("fail.sh", "#!/bin/sh\necho EARLY-OUT\nexit 1\n"),
            (
                "ask.sh",
                "#!/bin/sh\n[ -f ../before.done ] && [ ! -f ../after.done ] && [ ! -f ../slow.done ] && echo alone\nprintf 'name? '\nread answer\n",
            ),
        ],
    );
    let Some(mut s) = session(&env, &dir, &[]) else {
        return;
    };
    s.expect("name? ", T);
    s.send(b"x\n");
    let out = s.wait(T);
    let t = text(&out.stdout);
    assert_eq!(out.code, 1, "{t}");
    in_order(
        &t,
        &[
            &line("early", "Failed"),
            "EARLY-OUT\n",
            &line("before", "Passed"),
            &line("ask", "(interactive)"),
            "alone\n",
            "name? x\n",
            &line("ask", "Passed"),
            &line("slow", "Passed"),
            &line("after", "Passed"),
        ],
    );
}

/// The commitizen hook of a real monorepo's lefthook.yml, migrated with `snp migrate lefthook` and run by
/// `git commit` (prepare-commit-msg): the prompt appears on the terminal and reads the keyboard; a commit made with
/// `-m` (message source `message`) does not prompt.
#[test]
fn a_migrated_commitizen_hook_prompts_during_git_commit() {
    let env = env().with_env("GIT_EDITOR", "true");
    let lefthook = "prepare-commit-msg:\n  commands:\n    commitzen:\n      interactive: true\n      run: test -n \"{2}\" || ./cz.sh\n      env: {LEFTHOOK: \"0\"}\n";
    let cz = "#!/bin/sh\nprintf 'Select the type of change: '\nread kind\nprintf '%s: %s\\n' \"$kind\" \"subject from cz\" > \"$(git rev-parse --git-dir)/COMMIT_EDITMSG\"\n";
    let dir = env
        .git_repo("r")
        .file("lefthook.yml", lefthook.as_bytes())
        .exec_file("cz.sh", cz.as_bytes())
        .stage_all()
        .commit("init")
        .build();
    let migrated = env.snp(&dir).args(["migrate", "lefthook"]).output();
    assert_eq!(migrated.code, 0, "{}", text(&migrated.stderr));
    let config = std::fs::read_to_string(dir.join(".snp.yaml")).unwrap();
    assert!(config.contains("        interactive: true\n"), "{config}");
    assert!(
        config.contains("test -n \"$PRE_COMMIT_COMMIT_MSG_SOURCE\" || ./cz.sh"),
        "{config}"
    );
    let installed = env
        .snp(&dir)
        .args(["install", "--hook-type", "prepare-commit-msg"])
        .output();
    assert_eq!(installed.code, 0, "{}", text(&installed.stderr));
    std::fs::write(dir.join("b.txt"), b"b\n").unwrap();
    let added = env.git(&dir).args(["add", "b.txt", ".snp.yaml"]).output();
    assert_eq!(added.code, 0);
    let Some(mut s) = env.git(&dir).args(["commit"]).session(false) else {
        eprintln!("skipped: util-linux `setsid --ctty` is not available");
        return;
    };
    s.expect("Select the type of change: ", T);
    s.send(b"feat\n");
    let out = s.wait(T);
    let t = text(&out.stdout);
    assert_eq!(out.code, 0, "{t}");
    in_order(
        &t,
        &[
            &line("commitzen", "(interactive)"),
            "Select the type of change: feat\n",
            &line("commitzen", "Passed"),
        ],
    );
    let subject = env.git(&dir).args(["log", "-1", "--format=%s"]).output();
    assert_eq!(text(&subject.stdout), "feat: subject from cz\n");
    // `git commit -m`: the source is `message`, so the hook passes without prompting
    std::fs::write(dir.join("c.txt"), b"c\n").unwrap();
    assert_eq!(env.git(&dir).args(["add", "c.txt"]).output().code, 0);
    let Some(s) = env.git(&dir).args(["commit", "-m", "plain"]).session(false) else {
        return;
    };
    let out = s.wait(T);
    let t = text(&out.stdout);
    assert_eq!(out.code, 0, "{t}");
    assert!(!t.contains("Select the type"), "{t}");
    let subject = env.git(&dir).args(["log", "-1", "--format=%s"]).output();
    assert_eq!(text(&subject.stdout), "plain\n");
}
