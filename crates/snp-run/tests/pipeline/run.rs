//! `run` (T10-RUN, X13-EXE, X13-SM): transcripts recorded with pre-commit 4.7.0 (`pc470-py312`) on the same trees.

use std::ffi::OsString;
use std::time::Duration;

use snp_core::stage::{HookType, Stage};

use crate::harness::{H, StepClock, all, args, contains_bytes, line, local, rep, sys, verbose};

const META_IDENTITY: &str = "-   repo: meta\n    hooks:\n    -   id: identity\n";

/// The RUN-50 config: pass / fail / no-match local hooks + meta identity.
fn run50() -> H {
    let h = H::new();
    let mut cfg = local(&format!(
        "{}{}{}",
        sys("pass", "passing hook", "'true'", ""),
        sys(
            "fail",
            "failing hook",
            "sh -c 'echo out; echo err >&2; exit 3' --",
            ""
        ),
        sys("nomatch", "nothing matches", "'true'", "files: \\.nomatch$"),
    ));
    cfg.push_str(META_IDENTITY);
    h.config_staged(&cfg);
    h.write("a.txt", b"a\n");
    h.write("b.py", b"b\n");
    h.stage("a.txt");
    h.stage("b.py");
    h
}

/// Replaces the `- duration: …s` value (the oracle's own timing).
fn no_duration(s: &str) -> String {
    s.lines()
        .map(|l| {
            if l.starts_with("- duration: ") {
                "- duration: Xs"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if s.ends_with('\n') { "\n" } else { "" }
}

#[test]
fn t10_run_50_mixed_transcript() {
    let h = run50();
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    let want = format!(
        "{}{}- hook id: fail\n- exit code: 3\n\nout\nerr\n\n\
         nothing matches......................................(no files to check)Skipped\n{}\
         - hook id: identity\n- duration: Xs\n\n.pre-commit-config.yaml\na.txt\nb.py\n\n",
        line("passing hook", "Passed"),
        line("failing hook", "Failed"),
        line("identity", "Passed"),
    );
    assert_eq!(no_duration(&o.stdout), want);
    assert_eq!(o.stderr, "");
}

#[test]
fn t10_run_52_skip_prints_verbose_block() {
    let mut h = run50();
    h.env_set("SKIP", "fail,identity");
    let o = h.run(args());
    assert_eq!(o.code, 0, "{o:?}");
    assert_eq!(
        o.stdout,
        format!(
            "{}{}nothing matches......................................(no files to check)Skipped\n{}- hook id: identity\n",
            line("passing hook", "Passed"),
            line("failing hook", "Skipped"),
            line("identity", "Skipped"),
        )
    );
}

#[test]
fn t10_run_53_nonexistent_file_skips_everything() {
    let h = run50();
    let mut a = args();
    a.files = vec![OsString::from("does-not-exist.txt")];
    let o = h.run(a);
    assert_eq!(o.code, 0, "{o:?}");
    let nf = |n: &str| {
        let dots = 80 - 30 - n.len() + 3;
        format!("{n}{}(no files to check)Skipped\n", rep(".", dots))
    };
    assert_eq!(
        o.stdout,
        format!(
            "{}{}{}{}- hook id: identity\n",
            nf("passing hook"),
            nf("failing hook"),
            nf("nothing matches"),
            nf("identity")
        )
    );
}

#[test]
fn t10_run_55_empty_config() {
    let h = H::new();
    h.config("repos: []\n");
    for a in [args(), all()] {
        let o = h.run(a);
        assert_eq!((o.code, o.stdout.as_str(), o.stderr.as_str()), (0, "", ""));
    }
}

#[test]
fn t10_run_15_duration_uses_the_clock() {
    let mut h = H::new();
    h.config(
        META_IDENTITY
            .replace("-   repo", "repos:\n-   repo")
            .as_str(),
    );
    h.clock = Some(std::sync::Arc::new(StepClock::new(Duration::from_millis(
        770,
    ))));
    let o = h.run(verbose(all()));
    assert_eq!(o.code, 0, "{o:?}");
    assert!(o.stdout.contains("\n- duration: 0.77s\n"), "{}", o.stdout);
}

#[test]
fn t10_run_51_colours_and_pty() {
    let mut h = run50();
    h.env_set("SKIP", "nomatch");
    h.color = true;
    let o = h.run(args());
    assert_eq!(o.code, 1);
    let want = "passing hook.............................................................\x1b[42mPassed\x1b[m\n\
failing hook.............................................................\x1b[41mFailed\x1b[m\n\
\x1b[2m- hook id: fail\x1b[m\n\x1b[2m- exit code: 3\x1b[m\n\nout\nerr\n\n\
nothing matches.........................................................\x1b[43;30mSkipped\x1b[m\n\
identity.................................................................\x1b[42mPassed\x1b[m\n\
\x1b[2m- hook id: identity\x1b[m\n";
    assert!(o.stdout.starts_with(want), "{:?}", o.stdout);
    // hooks run under a pty (stdout a terminal, stdin not)
    let mut h = H::new();
    h.config(&local(&sys(
        "tty",
        "tty",
        "sh -c 'test -t 1 && echo tty || echo notty; test -t 0 && echo in-tty || echo in-notty' --",
        "verbose: true\nalways_run: true\npass_filenames: false",
    )));
    h.color = true;
    let o = h.run(args());
    assert!(o.stdout.ends_with("\ntty\nin-notty\n\n"), "{:?}", o.stdout);
    h.color = false;
    let o = h.run(args());
    assert!(
        o.stdout.ends_with("\nnotty\nin-notty\n\n"),
        "{:?}",
        o.stdout
    );
}

#[test]
fn t10_run_10_failing_hook_output() {
    let h = H::new();
    h.config(&local(&sys(
        "failing_hook",
        "Failing hook",
        "sh -c 'echo Fail; for f; do echo $f; done; exit 1' --",
        "",
    )));
    h.write("foo.py", b"");
    h.stage("foo.py");
    let o = h.run(args());
    assert_eq!(o.code, 1);
    assert!(o.stdout.contains(&line("Failing hook", "Failed")));
    assert!(
        o.stdout
            .contains("- hook id: failing_hook\n- exit code: 1\n\nFail\nfoo.py\n\n"),
        "{}",
        o.stdout
    );
}

#[test]
fn t10_run_11_arbitrary_bytes_pass_through() {
    let h = H::new();
    // No file names: BSD printf (macOS) fails on arguments its format does not use.
    h.config(&local(&sys(
        "bytes",
        "bytes",
        "printf '\\342\\230\\203\\262\\n'",
        "pass_filenames: false",
    )));
    h.write("foo.py", b"");
    h.stage("foo.py");
    let o = h.run(args());
    assert_eq!(o.code, 0);
    let o = h.run(verbose(args()));
    let needle = b"\xe2\x98\x83\xb2\n";
    assert!(contains_bytes(&o.stdout_bytes, needle), "{:?}", o.stdout);
}

#[test]
fn t10_run_12_modified_file_returns_zero() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}{}",
        sys(
            "bash_hook",
            "Bash hook",
            "sh -c 'echo Modified - foo.py; echo x >> foo.py' --",
            "files: foo\\.py$",
        ),
        sys("bash_hook2", "Bash hook", "'true'", ""),
        sys(
            "bash_hook3",
            "Bash hook",
            "sh -c 'echo y >> foo.py' --",
            "files: foo\\.py$"
        ),
    )));
    h.write("foo.py", b"");
    h.write("bar.py", b"");
    h.stage("foo.py");
    h.stage("bar.py");
    let o = h.run(args());
    assert_eq!(o.code, 1);
    let want = format!(
        "{}- hook id: bash_hook\n- files were modified by this hook\n\nModified - foo.py\n\n{}{}- hook id: bash_hook3\n\
         - files were modified by this hook\n",
        line("Bash hook", "Failed"),
        line("Bash hook", "Passed"),
        line("Bash hook", "Failed"),
    );
    assert!(o.stdout.ends_with(&want), "{o:?}");
    // the stash restored the staged-only state: the modifications made inside the run are kept
    assert_eq!(h.read("foo.py"), "x\ny\n");
}

#[test]
fn t10_run_13_types_filters() {
    let h = H::new();
    let echo = "sh -c 'for f; do echo $f; done' --";
    h.config(&local(&format!(
        "{}{}{}",
        sys("t", "types", echo, "types: [python]\nverbose: true"),
        sys(
            "o",
            "types_or",
            echo,
            "types_or: [python, cython]\nverbose: true"
        ),
        sys(
            "x",
            "exclude_types",
            echo,
            "exclude_types: [executable]\nverbose: true\nfiles: ^(exe|bar\\.py)$"
        ),
    )));
    for f in ["bar.py", "bar.notpy", "bar.pxd"] {
        h.write(f, b"x\n");
        h.stage(f);
    }
    h.exec("exe", b"#!/usr/bin/env python3\n");
    h.stage("exe");
    let o = h.run(args());
    assert_eq!(o.code, 0, "{}", o.stdout);
    let blocks: Vec<&str> = o.stdout.split("- hook id: ").skip(1).collect();
    // `exe` is python through its shebang
    assert!(
        blocks[0].contains("\nbar.py\nexe\n") && !blocks[0].contains("notpy"),
        "{}",
        o.stdout
    );
    assert!(
        blocks[1].contains("bar.pxd")
            && blocks[1].contains("bar.py")
            && !blocks[1].contains("notpy")
    );
    assert!(
        blocks[2].contains("bar.py") && !blocks[2].contains("\nexe"),
        "{}",
        blocks[2]
    );
}

#[test]
fn t10_run_14_global_include_exclude_then_identity() {
    for (top, tail) in [
        (
            "exclude: ^foo\\.py$\n",
            "\n\n.pre-commit-config.yaml\nbar.py\n\n",
        ),
        ("files: ^bar\\.py$\n", "\n\nbar.py\n\n"),
    ] {
        let h = H::new();
        h.config_staged(&format!("{top}repos:\n{META_IDENTITY}"));
        h.write("foo.py", b"");
        h.write("bar.py", b"");
        h.stage("foo.py");
        h.stage("bar.py");
        let o = h.run(verbose(args()));
        assert_eq!(o.code, 0);
        assert!(
            o.stdout
                .starts_with(&format!("identity{}Passed\n", rep(".", 65))),
            "{}",
            o.stdout
        );
        assert!(o.stdout.ends_with(tail), "{:?}", o.stdout);
    }
}

/// A fixer config with a committed `a.txt`, a staged and an unstaged change (RUN-16/RUN-58).
fn fixer() -> H {
    let h = H::new();
    h.config(&local(&sys(
        "fixer",
        "fixer",
        "sh -c 'for f; do echo fixed >> \"$f\"; done' --",
        "",
    )));
    h.write("a.txt", b"a\n");
    h.stage("a.txt");
    h.commit("init");
    h.write("a.txt", b"a\nstaged\n");
    h.stage("a.txt");
    h.write("a.txt", b"a\nstaged\nunstaged\n");
    h
}

#[test]
fn t10_run_58_stash_conflict_restores_the_unstaged_work() {
    let h = fixer();
    let mut a = args();
    a.show_diff_on_failure = true;
    let o = h.run(a);
    assert_eq!(o.code, 1, "{o:?}");
    let patch_line = o.stdout.lines().nth(1).unwrap().to_owned();
    assert!(
        patch_line.starts_with("[INFO] Stashing unstaged files to "),
        "{}",
        o.stdout
    );
    let patch = patch_line
        .trim_start_matches("[INFO] Stashing unstaged files to ")
        .trim_end_matches('.')
        .to_owned();
    let want = format!(
        "[WARNING] Unstaged files detected.\n{patch_line}\n{}- hook id: fixer\n- files were modified by this hook\n\
         All changes made by hooks:\n",
        line("fixer", "Failed")
    );
    assert!(o.stdout.starts_with(&want), "{}", o.stdout);
    assert!(o.stdout.ends_with(&format!(
        "[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...\n\
         [INFO] Restored changes from {patch}.\n"
    )));
    // the user's unstaged work is back; the patch file is kept
    assert_eq!(h.read("a.txt"), "a\nstaged\nunstaged\n");
    assert!(std::path::Path::new(&patch).is_file());
}

#[test]
fn t10_run_16_show_diff_all_files_prelude() {
    let h = fixer();
    let mut a = all();
    a.show_diff_on_failure = true;
    let o = h.run(a);
    assert_eq!(o.code, 1);
    assert!(o.stdout.starts_with(&format!(
        "{}- hook id: fixer\n- files were modified by this hook\npre-commit hook(s) made changes.\n\
         If you are seeing this message in CI, reproduce locally with: `pre-commit run --all-files`.\n\
         To run `pre-commit` as part of git workflow, use `pre-commit install`.\nAll changes made by hooks:\n",
        line("fixer", "Failed")
    )));
}

#[test]
fn t10_run_17_selection_and_unknown_id() {
    let h = H::new();
    h.config(&local(&sys(
        "bash_hook",
        "Bash hook",
        "sh -c 'echo Hello World; for f; do echo $f; done' --",
        "",
    )));
    h.write("foo.py", b"");
    h.stage("foo.py");
    let o = h.run(args());
    assert_eq!(
        (o.code, o.stdout.as_str()),
        (0, line("Bash hook", "Passed").as_str())
    );
    let o = h.run(verbose(args()));
    assert!(
        o.stdout.contains("\n\nHello World\nfoo.py\n\n"),
        "{}",
        o.stdout
    );
    let mut a = args();
    a.hooks = vec![OsString::from("nope")];
    let o = h.run(a.clone());
    assert_eq!(
        (o.code, o.stdout.as_str()),
        (1, "No hook with id `nope` in stage `pre-commit`\n")
    );
    a.hook_stage = Stage::Hook(HookType::PrePush);
    let o = h.run(a);
    assert_eq!(o.stdout, "No hook with id `nope` in stage `pre-push`\n");
    h.commit("x");
    let o = h.run(args());
    assert_eq!(
        o.stdout,
        "Bash hook............................................(no files to check)Skipped\n"
    );
}

#[test]
fn t10_run_18_log_file_gets_the_stripped_output() {
    let h = H::new();
    h.config(&local(&sys(
        "logfile",
        "Logfile",
        "sh -c 'echo This is STDOUT output; echo This is STDERR output >&2; exit 1' --",
        "log_file: test.log",
    )));
    h.write("foo.py", b"");
    h.stage("foo.py");
    let o = h.run(args());
    assert_eq!(o.code, 1);
    assert!(
        o.stdout
            .contains("\n\nThis is STDOUT output\nThis is STDERR output\n\n")
    );
    let log = std::fs::read_to_string(h.path("test.log")).unwrap();
    assert_eq!(log, "This is STDOUT output\nThis is STDERR output\n");
    // append mode
    drop(h.run(args()));
    let log = std::fs::read_to_string(h.path("test.log")).unwrap();
    assert_eq!(log.matches("STDOUT").count(), 2);
}

#[test]
fn x13_exe_05_unwritable_log_file_warns_and_keeps_the_output() {
    let h = H::new();
    h.config(&local(&sys(
        "l",
        "l",
        "sh -c 'echo some-output; exit 1' --",
        "log_file: nodir/x.log",
    )));
    h.write("foo.py", b"");
    h.stage("foo.py");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stdout.contains(
        "\n\nsome-output\n[WARNING] Could not write to log_file nodir/x.log: [Errno 2] No such file or directory: \
         'nodir/x.log'\n\n"
    ), "{}", o.stdout);
}

#[test]
fn t10_run_19_always_run_and_hook_verbose() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys("always", "always", "'true'", "always_run: true"),
        sys(
            "v",
            "verbose hook",
            "echo Hello World",
            "always_run: true\nverbose: true"
        ),
    )));
    let o = h.run(args());
    assert_eq!(o.code, 0);
    assert!(o.stdout.starts_with(&line("always", "Passed")));
    assert!(
        o.stdout.contains("- hook id: v\n- duration: "),
        "{}",
        o.stdout
    );
    assert!(o.stdout.ends_with("\n\nHello World\n\n"));
    // `-c alternate_config.yaml`
    h.write(
        "alternate_config.yaml",
        h.read(".pre-commit-config.yaml").as_bytes(),
    );
    h.stage("alternate_config.yaml");
    let mut a = args();
    a.config = snp_cli::ConfigArg::given("alternate_config.yaml");
    assert_eq!(h.run(a).code, 0);
}

#[test]
fn t10_run_20_from_and_to_ref_pairing() {
    let h = run50();
    for (from, to) in [(Some("master"), None), (None, Some("master"))] {
        let mut a = args();
        a.from_ref = from.map(OsString::from);
        a.to_ref = to.map(OsString::from);
        let o = h.run(a);
        assert_eq!(
            (o.code, o.stdout.as_str()),
            (1, "[ERROR] Specify both --from-ref and --to-ref.\n")
        );
    }
}

#[test]
fn t10_run_21_hooks_see_the_run_variables() {
    let h = H::new();
    h.config(&local(&sys(
        "env",
        "env",
        "sh -c 'echo \"$PRE_COMMIT|$PRE_COMMIT_FROM_REF|$PRE_COMMIT_TO_REF|$PRE_COMMIT_ORIGIN|$PRE_COMMIT_SOURCE|$PRE_COMMIT_CHECKOUT_TYPE\"' --",
        "always_run: true\nverbose: true\npass_filenames: false",
    )));
    h.write("a", b"a");
    h.stage("a");
    h.commit("one");
    let mut a = args();
    a.from_ref = Some("HEAD".into());
    a.to_ref = Some("HEAD".into());
    a.checkout_type = Some("1".into());
    let o = h.run(a);
    assert!(
        o.stdout.contains("\n\n1|HEAD|HEAD|HEAD|HEAD|1\n\n"),
        "{}",
        o.stdout
    );
}

#[test]
fn t10_run_22_merge_conflict() {
    let h = H::new();
    h.config(&local(&sys(
        "bash_hook",
        "Bash hook",
        "sh -c 'for f; do echo $f; done' --",
        "verbose: true",
    )));
    h.write("c.txt", b"base\n");
    h.stage("c.txt");
    h.commit("base");
    h.git(&["checkout", "-q", "-b", "other"]);
    h.write("c.txt", b"other\n");
    h.stage("c.txt");
    h.commit("other");
    h.git(&["checkout", "-q", "main"]);
    h.write("c.txt", b"main\n");
    h.stage("c.txt");
    h.commit("main");
    let m = h.git_any(&["merge", "-q", "other"]);
    assert!(!m.status.success());
    let o = h.run(args());
    assert_eq!(
        (o.code, o.stdout.as_str()),
        (1, "[ERROR] Unmerged files.  Resolve before committing.\n")
    );
    let mut a = args();
    a.files = vec![OsString::from("placeholder")];
    h.write("placeholder", b"");
    assert_eq!(h.run(a).code, 0);
    h.write("c.txt", b"resolved\n");
    h.git(&["add", "."]);
    let o = h.run(args());
    assert_eq!(o.code, 0, "{o:?}");
    assert!(
        o.stdout.starts_with(&format!(
            "[INFO] Checking merge-conflict files only.\n{}",
            line("Bash hook", "Passed")
        )),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("\nc.txt\n"), "{}", o.stdout);
}

#[test]
fn t10_run_23_skipped_hooks_are_not_installed() {
    let mut h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys("bash_hook", "Bash hook", "'true'", ""),
        sys("skipme", "skip me", "'true'", "alias: skipme-1"),
    )));
    h.write("a", b"");
    h.stage("a");
    h.env_set("SKIP", "skipme-1");
    let o = h.run(args());
    assert_eq!(o.code, 0);
    assert!(o.stdout.ends_with(&line("skip me", "Skipped")));
    assert_eq!(h.installed(), vec![vec![String::from("bash_hook")]]);
}

#[test]
fn t10_run_24_hook_id_only_when_verbose() {
    let h = H::new();
    h.config(&local(&sys("bash_hook", "Bash hook", "'true'", "")));
    h.write("a", b"");
    h.stage("a");
    assert!(!h.run(args()).stdout.contains("bash_hook"));
    assert!(
        h.run(verbose(args()))
            .stdout
            .contains("- hook id: bash_hook\n")
    );
}

#[test]
fn t10_run_25_duplicates_and_aliases() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys("bash_hook", "Bash hook", "'true'", ""),
        sys("bash_hook", "Bash hook", "'true'", "alias: foo_bash"),
    )));
    h.write("a", b"");
    h.stage("a");
    assert_eq!(h.run(args()).stdout.matches("Bash hook").count(), 2);
    let mut a = args();
    a.hooks = vec![OsString::from("bash_hook")];
    assert_eq!(h.run(a).stdout.matches("Bash hook").count(), 2);
    let mut a = args();
    a.hooks = vec![OsString::from("foo_bash")];
    assert_eq!(h.run(a).stdout.matches("Bash hook").count(), 1);
}

#[test]
fn t10_run_26_unicode_ids_and_args() {
    let h = H::new();
    h.config(&local(&sys(
        "bash_hook",
        "Bash hook",
        "echo",
        "args: ['☃']\nverbose: true",
    )));
    h.write("a", b"");
    h.stage("a");
    let mut a = args();
    a.hooks = vec![OsString::from("☃")];
    assert_eq!(
        h.run(a).stdout,
        "No hook with id `☃` in stage `pre-commit`\n"
    );
    let o = h.run(args());
    assert!(o.stdout.contains("\n\n☃ a\n\n"), "{}", o.stdout);
}

#[test]
fn t10_run_27_many_long_file_names_are_partitioned() {
    let h = H::new();
    h.config(&local(&sys(
        "count",
        "count",
        "sh -c 'echo $#' --",
        "files: ''\nverbose: true",
    )));
    let long = rep("a", 100);
    for i in 0..400 {
        h.write(&format!("{long}{i}"), b"");
    }
    h.git(&["add", "."]);
    let o = h.run(args());
    assert_eq!(o.code, 0, "{o:?}");
    let total: usize = o
        .stdout
        .lines()
        .filter_map(|l| l.trim().parse::<usize>().ok())
        .sum();
    assert_eq!(total, 400);
}

#[test]
fn t10_run_28_modification_detection_ignores_textconv() {
    let h = H::new();
    h.write(".gitattributes", b"*.jpeg diff=empty\n");
    h.git(&["config", "diff.empty.textconv", "true"]);
    h.config(&local(&sys(
        "fixer",
        "add null",
        "sh -c 'for f; do printf \"\\000\" >> \"$f\"; done' --",
        "types: [jpeg]",
    )));
    h.write("a.jpeg", b"\xff\xd8");
    h.stage("a.jpeg");
    h.stage(".gitattributes");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{}", o.stdout);
    assert!(o.stdout.starts_with(&line("add null", "Failed")));
}

#[test]
fn t10_run_29_stages_select_hooks() {
    let h = H::new();
    let grep = |id: &str, name: &str, stage: &str| {
        format!(
            "    -   id: {id}\n        name: {name}\n        entry: DO NOT COMMIT\n        language: pygrep\n        stages: [{stage}]\n"
        )
    };
    h.config(&local(&format!(
        "{}{}{}",
        grep("h1", "hook 1", "pre-commit"),
        grep("h2", "hook 2", "pre-push"),
        grep("h3", "hook 3", "manual")
    )));
    h.write("a", b"ok\n");
    h.stage("a");
    for (stage, name) in [
        (Stage::Hook(HookType::PreCommit), "hook 1"),
        (Stage::Hook(HookType::PrePush), "hook 2"),
        (Stage::Manual, "hook 3"),
    ] {
        let mut a = args();
        a.hook_stage = stage;
        let o = h.run(a);
        assert_eq!(o.stdout.matches("hook ").count(), 1, "{}", o.stdout);
        assert!(o.stdout.starts_with(name), "{}", o.stdout);
    }
}

#[test]
fn t10_run_30_commit_msg_stages() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        "    -   id: must-sign\n        name: 'Must have \"Signed off by:\"'\n        entry: 'Signed off by:'\n        \
         language: pygrep\n        args: [--negate]\n        stages: [commit-msg]\n",
        sys(
            "add-sign",
            "'Add \"Signed off by:\"'",
            "\"sh -c 'echo Signed off by: >> \\\"$1\\\"' --\"",
            "stages: [prepare-commit-msg]",
        ),
    )));
    h.write(".git/COMMIT_EDITMSG", b"This is the commit message\n");
    let mut a = args();
    a.hook_stage = Stage::Hook(HookType::CommitMsg);
    a.commit_msg_filename = Some(".git/COMMIT_EDITMSG".into());
    let o = h.run(a);
    assert_eq!(o.code, 1, "{}", o.stdout);
    assert!(
        o.stdout.starts_with("Must have \"Signed off by:\"...") && o.stdout.contains("Failed"),
        "{o:?}"
    );
    let mut a = args();
    a.hook_stage = Stage::Hook(HookType::PrepareCommitMsg);
    a.commit_msg_filename = Some(".git/COMMIT_EDITMSG".into());
    a.prepare_commit_message_source = Some("commit".into());
    a.commit_object_name = Some("HEAD".into());
    let o = h.run(a);
    assert_eq!(o.code, 0, "{}", o.stdout);
    assert!(h.read(".git/COMMIT_EDITMSG").contains("Signed off by:"));
    // post-checkout runs hooks with no files
    let h = H::new();
    h.config("repos:\n-   repo: meta\n    hooks:\n    -   id: identity\n        stages: [post-checkout]\n");
    let mut a = args();
    a.hook_stage = Stage::Hook(HookType::PostCheckout);
    let o = h.run(a);
    assert_eq!(o.code, 0);
    assert!(o.stdout.starts_with("identity..."), "{}", o.stdout);
}

#[test]
fn t10_run_31_grep_and_shell_hooks() {
    let h = H::new();
    h.config(&local(&format!(
        "{}    -   id: do_not_commit\n        name: Block if \"DO NOT COMMIT\" is found\n        entry: DO NOT COMMIT\n        language: pygrep\n",
        sys("identity-copy", "identity copy", "'true'", "files: \\.py$"),
    )));
    h.write("a.py", b"print(1)\n");
    h.stage("a.py");
    assert_eq!(h.run(args()).code, 0);
    h.write("b.txt", b"DO NOT COMMIT\n");
    h.stage("b.txt");
    let o = h.run(args());
    assert_eq!(o.code, 1);
    assert!(
        o.stdout.contains("\n\nb.txt:1:DO NOT COMMIT\n\n"),
        "{}",
        o.stdout
    );
    let h = H::new();
    h.config(&local(&sys(
        "todo",
        "no todos",
        "sh -c '! grep -iI todo \"$@\"' --",
        "",
    )));
    h.write("a.txt", b"TODO: x\n");
    h.stage("a.txt");
    assert_eq!(h.run(args()).code, 1);
}

#[test]
fn t10_run_32_unstaged_config() {
    let h = run50();
    h.write(
        ".pre-commit-config.yaml",
        (h.read(".pre-commit-config.yaml") + "\n").as_bytes(),
    );
    let o = h.run(args());
    assert_eq!(
        (o.code, o.stdout.as_str()),
        (
            1,
            "[ERROR] Your pre-commit configuration is unstaged.\n`git add .pre-commit-config.yaml` to fix this.\n"
        )
    );
    assert_ne!(h.run(all()).stdout, o.stdout);
    let mut a = args();
    a.files = vec![".pre-commit-config.yaml".into()];
    assert!(!h.run(a).stdout.contains("unstaged"));
}

#[test]
fn t10_run_33_commit_msg_filename_required() {
    let h = run50();
    let mut a = args();
    a.hook_stage = Stage::Hook(HookType::CommitMsg);
    let o = h.run(a);
    assert_eq!(
        (o.code, o.stdout.as_str()),
        (
            1,
            "[ERROR] `--commit-msg-filename` is required for `--hook-stage commit-msg`\n"
        )
    );
}

#[test]
fn t10_run_34_files_relative_to_the_invocation_cwd() {
    let mut h = H::new();
    h.config(&local(&sys("e", "echo", "echo", "verbose: true")));
    h.write("subdir/foo.py", b"");
    h.stage("subdir/foo.py");
    h.cwd = Some(h.path("subdir"));
    let mut a = verbose(args());
    a.files = vec!["foo.py".into()];
    let o = h.run(a);
    assert!(o.stdout.contains("\n\nsubdir/foo.py\n\n"), "{}", o.stdout);
}

#[test]
fn t10_run_35_pass_filenames_and_args() {
    for (pass, args_yaml, want) in [
        (true, "[]", "foo.py\nHello World"),
        (false, "[]", "\nHello World"),
        (true, "[some, args]", "some args foo.py"),
        (false, "[some, args]", "some args"),
    ] {
        let h = H::new();
        let entry = if args_yaml == "[]" {
            "sh -c 'for f; do echo $f; done; echo Hello World' --"
        } else {
            "echo"
        };
        h.config(&local(&sys(
            "h",
            "h",
            entry,
            &format!("pass_filenames: {pass}\nargs: {args_yaml}\nverbose: true"),
        )));
        h.write("foo.py", b"");
        h.stage("foo.py");
        let o = h.run(args());
        assert!(o.stdout.contains(want), "{pass} {args_yaml}: {}", o.stdout);
        assert_eq!(o.stdout.contains("foo.py"), pass);
    }
}

#[test]
fn t10_run_36_fail_fast() {
    let failing = |extra: &str| sys("f", "Failing hook", "'false'", extra);
    for (top, first, cli) in [
        ("fail_fast: true\n", "", false),
        ("", "fail_fast: true", false),
        ("", "", true),
    ] {
        let h = H::new();
        h.config(&format!(
            "{top}{}",
            local(&format!("{}{}", failing(first), failing("")))
        ));
        h.write("a", b"");
        h.stage("a");
        let mut a = args();
        a.fail_fast = cli;
        let o = h.run(a);
        assert_eq!(o.code, 1);
        assert_eq!(o.stdout.matches("Failing hook").count(), 1, "{}", o.stdout);
    }
    // a passing fail_fast hook does not stop the run
    let h = H::new();
    h.config("repos:\n-   repo: meta\n    hooks:\n    -   id: identity\n        fail_fast: true\n    -   id: identity\n        name: run me!\n");
    let o = h.run(all());
    assert_eq!(o.stdout.matches("run me!").count(), 1);
}

#[test]
fn t10_run_37_classifier_lexists_and_symlinks() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys("s", "symlinks", "echo", "types: [symlink]\nverbose: true"),
        sys("f", "files", "echo", "types: [file]\nverbose: true"),
    )));
    h.write("bar", b"x");
    std::os::unix::fs::symlink("bar", h.path("foo")).unwrap();
    std::os::unix::fs::symlink("nowhere", h.path("broken")).unwrap();
    let mut a = args();
    a.files = vec![
        "foo".into(),
        "bar".into(),
        "broken".into(),
        "missing".into(),
    ];
    let o = h.run(a);
    let blocks: Vec<&str> = o.stdout.split("- hook id: ").skip(1).collect();
    assert!(blocks[0].contains("\n\nfoo broken\n\n"), "{}", blocks[0]);
    assert!(blocks[1].contains("\n\nbar\n\n"), "{}", blocks[1]);
}

#[test]
fn t10_run_38_include_exclude_is_search() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}{}",
        sys("py", "py", "echo", "files: ^.*\\.py$\nverbose: true"),
        sys("yaml", "yaml", "echo", "files: \\.yaml$\nverbose: true"),
        sys("ex", "ex", "echo", "exclude: \\.py$\nverbose: true"),
    )));
    for f in [
        ".pre-commit-hooks.yaml",
        "pre_commit/git.py",
        "pre_commit/main.py",
    ] {
        h.write(f, b"x\n");
    }
    let mut a = args();
    a.files = vec![
        ".pre-commit-hooks.yaml".into(),
        "pre_commit/git.py".into(),
        "pre_commit/main.py".into(),
    ];
    let o = h.run(a);
    let blocks: Vec<&str> = o.stdout.split("- hook id: ").skip(1).collect();
    assert!(blocks[0].contains("pre_commit/git.py") && blocks[0].contains("pre_commit/main.py"));
    assert!(
        blocks[1].contains("\n\n.pre-commit-hooks.yaml\n\n"),
        "{}",
        blocks[1]
    );
    assert!(
        blocks[2].contains("\n\n.pre-commit-hooks.yaml\n\n"),
        "{}",
        blocks[2]
    );
}

#[test]
fn t10_run_39_one_hook_selected() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys("identity-copy", "identity-copy", "'true'", ""),
        sys("do_not_commit", "do not commit", "'true'", ""),
    )));
    h.write("a", b"");
    h.stage("a");
    let mut a = args();
    a.hooks = vec!["do_not_commit".into()];
    let o = h.run(a);
    assert!(!o.stdout.contains("identity-copy"));
    assert!(o.stdout.contains("do not commit"));
}

#[test]
fn t10_run_40_skip_post_checkout_before_config_load() {
    let mut h = H::new();
    h.env_set("_PRE_COMMIT_SKIP_POST_CHECKOUT", "1");
    let mut a = args();
    a.hook_stage = Stage::Hook(HookType::PostCheckout);
    let o = h.run(a);
    assert_eq!((o.code, o.stdout.as_str(), o.stderr.as_str()), (0, "", ""));
}

#[test]
fn t10_run_54_files_without_a_hook_filter_run_every_hook() {
    let h = run50();
    let mut a = args();
    a.files = vec!["a.txt".into(), "pass".into()];
    let o = h.run(a);
    assert!(o.stdout.contains(&line("passing hook", "Passed")));
    assert!(o.stdout.contains(&line("failing hook", "Failed")));
}

#[test]
fn t10_run_56_missing_config() {
    let h = H::new();
    let o = h.run(args());
    assert_eq!(o.code, 1);
    match o.failure {
        Some(snp_core::failure::Failure::Exc { exc, .. }) => {
            assert_eq!(exc.ty, "InvalidConfigError");
            assert_eq!(
                String::from_utf8_lossy(&exc.text),
                "\n=====> .pre-commit-config.yaml is not a file"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn t10_run_57_outside_git() {
    let mut h = H::new();
    let outside = h.base.join("home");
    h.cwd = Some(outside);
    let o = h.run(args());
    assert_eq!(o.code, 1);
    match o.failure {
        Some(snp_core::failure::Failure::Exc { exc, .. }) => {
            assert_eq!(exc.ty, "FatalError");
            assert_eq!(
                String::from_utf8_lossy(&exc.text),
                "git failed. Is it installed, and are you in a Git repository directory?"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn t10_xrg_17_require_serial_keeps_order() {
    let h = H::new();
    h.config(&local(&sys(
        "e",
        "e",
        "echo",
        "require_serial: true\nverbose: true",
    )));
    let files: Vec<OsString> = (0..10).map(|i| OsString::from(i.to_string())).collect();
    for f in &files {
        h.write(&f.to_string_lossy(), b"");
    }
    let mut a = args();
    a.files = files;
    let o = h.run(a);
    assert!(
        o.stdout.ends_with("\n\n0 1 2 3 4 5 6 7 8 9\n\n"),
        "{}",
        o.stdout
    );
}

#[test]
fn t10_xrg_18_entry_args_files() {
    let h = H::new();
    h.config(&local(&sys(
        "e",
        "e",
        "echo hi",
        "args: [hello]\nverbose: true",
    )));
    h.write("file", b"");
    let mut a = args();
    a.files = vec!["file".into(), "file".into(), "file".into()];
    let o = h.run(a);
    assert!(
        o.stdout.ends_with("\n\nhi hello file file file\n\n"),
        "{}",
        o.stdout
    );
}

#[test]
fn x13_exe_01_empty_entry_fails_the_hook() {
    let h = H::new();
    h.config(&local(&sys("empty", "empty", "''", "")));
    h.write("a", b"");
    h.stage("a");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(
        o.stdout
            .contains("- exit code: 1\n\nExecutable `` not found\n\n"),
        "{}",
        o.stdout
    );
}

#[test]
fn x13_exe_02_shlex_error_fails_the_hook() {
    let h = H::new();
    h.config(&local(&sys("q", "q", "\"echo 'unterminated\"", "")));
    h.write("a", b"");
    h.stage("a");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(
        o.stdout
            .contains("\n\nValueError: No closing quotation\n\n"),
        "{}",
        o.stdout
    );
}

#[test]
fn x13_exe_03_argument_list_too_long() {
    let h = H::new();
    let big = rep("x", 140_000);
    h.config(&local(&sys(
        "big",
        "big",
        "echo",
        &format!("args: ['{big}']"),
    )));
    h.write("a", b"");
    h.stage("a");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{}", o.stdout.get(..400).unwrap_or(&o.stdout));
    assert!(
        o.stdout
            .contains("\n\n[Errno 7] Argument list too long: '/usr/bin/echo'\n\n")
            || o.stdout
                .contains("\n\n[Errno 7] Argument list too long: '/bin/echo'\n\n"),
        "{}",
        o.stdout
    );
}

#[test]
fn x13_sm_06_e2big_without_files() {
    let h = H::new();
    // Linux limits one argument to 128 KiB (MAX_ARG_STRLEN); macOS and the BSDs only the whole argv + env (ARG_MAX,
    // 1 MiB on macOS).
    let big = rep(
        "x",
        if cfg!(target_os = "linux") {
            140_000
        } else {
            1_100_000
        },
    );
    h.config(&local(&sys(
        "big",
        "big",
        "echo",
        &format!("args: ['{big}']\npass_filenames: false"),
    )));
    h.write("a", b"");
    h.stage("a");
    let o = h.run(args());
    assert_eq!(o.code, 1);
    assert!(
        o.stdout.contains("[Errno 7] Argument list too long: '"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("- exit code: 1\n"));
}

#[test]
fn x13_exe_04_unreadable_entry_script() {
    let h = H::new();
    h.config(&local(&sys("s", "s", "./s.sh", "")));
    h.exec("s.sh", b"#!/bin/sh\necho hi\n");
    std::fs::set_permissions(
        h.path("s.sh"),
        std::os::unix::fs::PermissionsExt::from_mode(0o111),
    )
    .unwrap();
    h.write("a", b"");
    h.stage("a");
    let o = h.run(args());
    // root can read anything: the hook then simply runs
    if o.code == 1 {
        assert!(
            o.stdout.contains("[Errno 13] Permission denied: './s.sh'"),
            "{}",
            o.stdout
        );
    }
}

#[test]
fn x13_sm_07_empty_first_token_and_hash() {
    for (entry, msg) in [
        ("\"'' x\"", "Executable `` not found"),
        ("'#'", "Executable `#` not found"),
    ] {
        let h = H::new();
        h.config(&local(&sys("e", "e", entry, "")));
        h.write("a", b"");
        h.stage("a");
        let o = h.run(args());
        assert!(
            o.stdout.contains(&format!("\n\n{msg}\n\n")),
            "{entry}: {}",
            o.stdout
        );
    }
}

#[test]
fn x13_git_04_bad_from_ref_reports_the_two_dot_diff() {
    let h = run50();
    h.commit("one");
    let mut a = args();
    a.from_ref = Some("bogus".into());
    a.to_ref = Some("HEAD".into());
    let o = h.run(a);
    assert_eq!(o.code, 3, "{o:?}");
    match o.failure {
        Some(snp_core::failure::Failure::Exc { exc, .. }) => {
            assert_eq!(exc.ty, "CalledProcessError");
            let t = String::from_utf8_lossy_owned(exc.text);
            assert!(
                t.contains("'diff', '--name-only', '--no-ext-diff', '-z', 'bogus..HEAD')"),
                "{t}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn signal_deaths_are_negative_exit_codes() {
    let h = H::new();
    h.config(&local(&sys(
        "sig",
        "killed by signal",
        "sh -c 'kill -9 $$' --",
        "",
    )));
    h.write("a", b"");
    h.stage("a");
    let o = h.run(args());
    assert!(o.stdout.contains("- exit code: -9\n"), "{}", o.stdout);
}

#[test]
fn exec_errors_render_like_pre_commit() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}{}{}",
        sys("notfound", "not found", "does-not-exist-xyz", ""),
        sys("dir", "is a dir", "./sub", ""),
        sys("noexec", "not executable", "./noexec.sh", ""),
        sys(
            "noshebang",
            "no shebang",
            "./noshebang.sh",
            "require_serial: true"
        ),
    )));
    std::fs::create_dir_all(h.path("sub")).unwrap();
    h.write("noexec.sh", b"#!/bin/sh\n");
    h.exec("noshebang.sh", b"echo hi\n");
    h.write("a", b"");
    h.stage("a");
    let o = h.run(args());
    for want in [
        "\n\nExecutable `does-not-exist-xyz` not found\n\n",
        "\n\nExecutable `./sub` is a directory\n\n",
        "\n\nExecutable `./noexec.sh` is not executable\n\n",
        "\n\n[Errno 8] Exec format error: './noshebang.sh'\n\n",
    ] {
        assert!(o.stdout.contains(want), "{want:?} in {}", o.stdout);
    }
}

#[test]
fn interrupt_restores_the_stash_and_reports_interrupted() {
    let mut h = fixer();
    h.config(&local(&sys(
        "slow",
        "slow",
        "sleep 20",
        "pass_filenames: false",
    )));
    h.write("a.txt", b"a\nstaged2\n");
    h.stage("a.txt");
    h.write("a.txt", b"a\nstaged2\nunstaged\n");
    h.cancel_after = Some(Duration::from_millis(500));
    let t0 = std::time::Instant::now();
    let o = h.run(args());
    assert!(t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
    assert_eq!(o.code, 130, "{o:?}");
    assert!(matches!(
        o.failure,
        Some(snp_core::failure::Failure::Interrupted(
            snp_core::cancel::CancelReason::Interrupt
        ))
    ));
    assert!(
        o.stdout.contains("[INFO] Restored changes from "),
        "{}",
        o.stdout
    );
    assert_eq!(h.read("a.txt"), "a\nstaged2\nunstaged\n");
}

#[test]
fn epipe_stops_the_run_and_restores_the_stash() {
    let mut h = fixer();
    h.broken_stdout = true;
    let o = h.run(args());
    assert_eq!(o.code, 120, "{o:?}");
    assert!(matches!(
        o.failure,
        Some(snp_core::failure::Failure::StdoutClosed)
    ));
    assert_eq!(h.read("a.txt"), "a\nstaged\nunstaged\n");
}

/// The git subcommands `snp_run::run` spawned, logged by a `git` wrapper first on `PATH` (`--version` excluded).
fn logged_git(h: &mut H) -> std::path::PathBuf {
    let bin = h.base.join("wrap-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = h.base.join("git.log");
    let real = ["/usr/bin/git", "/usr/local/bin/git", "/bin/git"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec {real} \"$@\"\n",
        log.display()
    );
    let wrapper = bin.join("git");
    std::fs::write(&wrapper, script).unwrap();
    let mut perm = std::fs::metadata(&wrapper).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    std::fs::set_permissions(&wrapper, perm).unwrap();
    let path = format!("{}:/usr/local/bin:/usr/bin:/bin", bin.display());
    h.env_set("PATH", path);
    log
}

fn git_spawns(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|l| *l != "--version")
        .map(str::to_owned)
        .collect()
}

/// ARCHITECTURE §12.1 B2/B3 shapes, warm second run: the git spawn count is an upper bound (an O(hooks) `git diff`
/// or a duplicated query shows up here long before it shows up as a timing regression). With three or more CPUs
/// (ADR-104) both take 7: `rev-parse`, F `ls-files -s -z` (which also answers A), G `config --list --show-origin
/// -z`, B `diff --quiet <cfg>`, the stash's ITA query, `write-tree` and `diff-index`, and no `git diff` (the Stat
/// mode applies from one hook on a small tree). Below three CPUs: A instead of F/G, plus F/G after the load for B3
/// (8) and one `git diff` for B2 (7).
#[test]
fn spawn_bounds_for_the_b2_b3_shapes() {
    let hoist = snp_proc::xargs::cpu_count().get() >= 3;
    let b3 = if hoist { 7 } else { 8 };
    for (n, bound) in [(1_usize, 7_usize), (10, b3)] {
        let mut h = H::new();
        let log = logged_git(&mut h);
        let hooks: String = (1..=n)
            .map(|i| {
                sys(
                    &format!("noop-{i:02}"),
                    &format!("no-op {i:02}"),
                    "'true'",
                    "always_run: true\npass_filenames: false",
                )
            })
            .collect();
        h.config(&local(&hooks));
        assert_eq!(h.run(args()).code, 0);
        let _ignored = std::fs::remove_file(&log);
        let o = h.run(args());
        assert_eq!(o.code, 0, "{o:?}");
        let spawns = git_spawns(&log);
        assert!(
            spawns.len() <= bound,
            "{n} hooks: {} git spawns > {bound}:\n{}",
            spawns.len(),
            spawns.join("\n")
        );
        if n >= 3 || hoist {
            assert!(
                !spawns.iter().any(|s| s.starts_with("diff --no-ext-diff")),
                "the stat oracle elides the per-hook `git diff`:\n{}",
                spawns.join("\n")
            );
        }
    }
}

/// ADR-084: with three or more hooks the `DiffOracle` inputs are read before the stash; when the stash clears
/// intent-to-add entries that early index read is stale and is taken again, so a hook that writes into an
/// intent-to-add file (untracked while the hooks run) is not reported as modifying files, exactly as with `git diff`,
/// and a hook that modifies a tracked file still is.
#[test]
fn early_oracle_inputs_follow_an_intent_to_add_clearing() {
    for (target, modified) in [("n.txt", false), ("t.txt", true)] {
        let h = H::new();
        let mut hooks = sys(
            "touch",
            "touch",
            &format!("sh -c 'echo more >> {target}' --"),
            "always_run: true\npass_filenames: false",
        );
        for i in 1..=3 {
            hooks.push_str(&sys(
                &format!("noop-{i}"),
                &format!("noop {i}"),
                "'true'",
                "always_run: true\npass_filenames: false",
            ));
        }
        h.config(&local(&hooks));
        h.write("t.txt", b"t\n");
        h.git(&["add", "t.txt"]);
        h.git(&["commit", "-qm", "t"]);
        h.write("n.txt", b"n\n");
        h.git(&["add", "-N", "n.txt"]);
        h.write("t.txt", b"t\nunstaged\n");
        let o = h.run(args());
        assert_eq!(o.code, i32::from(modified), "{target}: {o:?}");
        assert_eq!(
            o.stdout.contains("- files were modified by this hook"),
            modified,
            "{target}: {}",
            o.stdout
        );
    }
}

/// ADR-084 (B9): `hook-impl` discovers the repository once; the pipeline reuses it (one combined `rev-parse`).
#[test]
fn hook_impl_discovers_the_repository_once() {
    let mut h = H::new();
    let log = logged_git(&mut h);
    h.config(&local(&sys(
        "noop",
        "noop",
        "'true'",
        "always_run: true\npass_filenames: false",
    )));
    let o = h.hook_impl(
        crate::harness::hook_impl_args("pre-commit", &["--"], None),
        b"",
    );
    assert_eq!(o.code, 0, "{o:?}");
    let spawns = git_spawns(&log);
    let rev_parses = spawns
        .iter()
        .filter(|s| s.starts_with("rev-parse --show-cdup"))
        .count();
    assert_eq!(rev_parses, 1, "{}", spawns.join("\n"));
}

/// Three `always_run` noop hooks (Stat mode applies from three on).
fn three_noops() -> String {
    (1..=3)
        .map(|i| {
            sys(
                &format!("noop-{i}"),
                &format!("noop {i}"),
                "'true'",
                "always_run: true\npass_filenames: false",
            )
        })
        .collect()
}

/// An unmerged index: both sides modified `f.txt`.
fn conflicted(h: &H) {
    h.write("f.txt", b"base\n");
    h.git(&["add", "f.txt"]);
    h.git(&["commit", "-qm", "base"]);
    h.git(&["checkout", "-qb", "other"]);
    h.write("f.txt", b"other\n");
    h.git(&["commit", "-qam", "other"]);
    h.git(&["checkout", "-q", "main"]);
    h.write("f.txt", b"mine\n");
    h.git(&["commit", "-qam", "mine"]);
    let _conflict = h.git_any(&["merge", "-q", "other"]);
}

/// ADR-104: on a small index F (`ls-files -s -z`) starts before the config load and answers query A, so no
/// `ls-files --unmerged` runs; the preflight output is the one of the asked query (`SNP_DIFF_MODE=git` turns the
/// speculation off), for an unmerged index and after it is resolved.
#[test]
fn unmerged_preflight_from_early_f_matches_the_asked_query() {
    for resolved in [false, true] {
        let run = |mode: Option<&str>| {
            let mut h = H::new();
            h.config(&local(&three_noops()));
            conflicted(&h);
            if resolved {
                h.write("f.txt", b"resolved\n");
                h.git(&["add", "f.txt"]);
            }
            if let Some(m) = mode {
                h.env_set("SNP_DIFF_MODE", m);
            }
            let log = logged_git(&mut h);
            let o = h.run(args());
            (o, git_spawns(&log))
        };
        let (spec, spec_log) = run(None);
        let (asked, asked_log) = run(Some("git"));
        assert_eq!(spec.code, i32::from(!resolved), "{spec:?}");
        assert_eq!(
            (spec.code, &spec.stdout, &spec.stderr),
            (asked.code, &asked.stdout, &asked.stderr),
            "resolved={resolved}"
        );
        assert!(
            !spec_log.iter().any(|s| s == "ls-files --unmerged"),
            "A is derived from F:\n{}",
            spec_log.join("\n")
        );
        assert!(
            asked_log.iter().any(|s| s == "ls-files --unmerged"),
            "{}",
            asked_log.join("\n")
        );
    }
}

/// ADR-104: when F fails (here an index whose header is sound but whose garbage entry `ls-files -s -z` prints in a
/// shape the strict parser refuses), the preflight asks `git ls-files --unmerged` at the same position, so the
/// outcome is the one of pre-commit's own query.
#[test]
fn a_failing_early_f_reports_the_unmerged_query_error() {
    let run = |mode: Option<&str>| {
        let mut h = H::new();
        h.config(&local(&three_noops()));
        // `DIRC`, version 2, one entry, then garbage
        let mut index = b"DIRC\0\0\0\x02\0\0\0\x01".to_vec();
        index.extend_from_slice(&[0xab; 64]);
        std::fs::write(h.path(".git/index"), index).unwrap();
        if let Some(m) = mode {
            h.env_set("SNP_DIFF_MODE", m);
        }
        let log = logged_git(&mut h);
        let o = h.run(args());
        (o, git_spawns(&log))
    };
    let (spec, spec_log) = run(None);
    let (asked, _) = run(Some("git"));
    assert_ne!(spec.code, 0, "{spec:?}");
    assert_eq!(
        (spec.code, &spec.stdout, &spec.stderr),
        (asked.code, &asked.stdout, &asked.stderr),
        "{spec:?}"
    );
    assert_eq!(
        format!("{:?}", spec.failure),
        format!("{:?}", asked.failure)
    );
    let position = |s: &str| spec_log.iter().position(|l| l == s);
    assert!(
        position("ls-files -s -z").is_some() && position("ls-files --unmerged").is_some(),
        "F failed, then A was asked:\n{}",
        spec_log.join("\n")
    );
}

/// ADR-104 (C2): F from step 6 is kept only while the index is unchanged; the stash's own checkout rewrites the
/// index, so F is asked again after it, and a hook's edit is still seen.
#[test]
fn early_f_is_asked_again_after_an_index_rewrite() {
    let mut h = H::new();
    let hooks = format!(
        "{}{}",
        sys(
            "touch",
            "touch",
            "sh -c 'echo more >> t.txt' --",
            "always_run: true\npass_filenames: false",
        ),
        three_noops()
    );
    h.config(&local(&hooks));
    h.write("t.txt", b"t\n");
    h.git(&["add", "."]);
    h.git(&["commit", "-qm", "t"]);
    h.write("t.txt", b"t\nunstaged\n");
    let log = logged_git(&mut h);
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(
        o.stdout.contains("- files were modified by this hook"),
        "{}",
        o.stdout
    );
    let spawns = git_spawns(&log);
    assert_eq!(
        spawns.iter().filter(|s| *s == "ls-files -s -z").count(),
        2,
        "{}",
        spawns.join("\n")
    );
    assert_eq!(h.read("t.txt"), "t\nunstaged\n", "restored");
}

/// ADR-104: one process hook uses the Stat mode on a small tree (no after-hook `git diff`) and Git mode on a larger
/// one, where the stat pass would cost more than the diff.
#[test]
fn one_hook_uses_the_stat_mode_only_on_a_small_tree() {
    for (files, stat) in [(3_usize, true), (300, false)] {
        let mut h = H::new();
        h.config(&local(&sys(
            "noop",
            "noop",
            "'true'",
            "always_run: true\npass_filenames: false",
        )));
        for i in 0..files {
            h.write(&format!("d{}/f{i}.txt", i % 7), b"x\n");
        }
        h.git(&["add", "."]);
        h.git(&["commit", "-qm", "files"]);
        assert_eq!(h.run(args()).code, 0);
        let log = logged_git(&mut h);
        let o = h.run(args());
        assert_eq!(o.code, 0, "{o:?}");
        let spawns = git_spawns(&log);
        let diffs = spawns
            .iter()
            .filter(|s| s.starts_with("diff --no-ext-diff"))
            .count();
        assert_eq!(
            diffs,
            usize::from(!stat),
            "{files} files:\n{}",
            spawns.join("\n")
        );
    }
}
