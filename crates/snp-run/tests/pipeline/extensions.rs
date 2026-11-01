//! snp's config extension keys in the run pipeline (ARCHITECTURE §8; COMPATIBILITY §7).

use std::ffi::OsString;
use std::time::{Duration, Instant};

use snp_core::brand::Brand;

use crate::harness::{H, args, line, local, rep, sys};

fn staged(h: &H, path: &str, content: &[u8]) {
    h.write(path, content);
    h.stage(path);
}

#[test]
fn priority_buckets_run_concurrently_and_print_in_config_order() {
    let h = H::new();
    let a = h.path("a.flag");
    let b = h.path("b.flag");
    // each hook waits (≤ 5 s) for the other's flag: they pass only when they run at the same time
    let wait = |me: &std::path::Path, other: &std::path::Path, out: &str| {
        format!(
            "sh -c 'touch {}; i=0; while [ ! -e {} ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done; \
             [ -e {} ] && echo {out}' --",
            me.display(),
            other.display(),
            other.display()
        )
    };
    h.config(&local(&format!(
        "{}{}{}",
        sys(
            "one",
            "one",
            &wait(&a, &b, "A"),
            "priority: 0\nverbose: true\nalways_run: true\npass_filenames: false"
        ),
        sys(
            "two",
            "two",
            &wait(&b, &a, "B"),
            "priority: 0\nverbose: true\nalways_run: true\npass_filenames: false"
        ),
        sys(
            "three",
            "three",
            "echo C",
            "priority: 1\nverbose: true\nalways_run: true\npass_filenames: false"
        ),
    )));
    let o = h.run(args());
    assert_eq!(o.code, 0, "{}", o.stdout);
    let one = o.stdout.find("one....").unwrap();
    let two = o.stdout.find("two....").unwrap();
    let three = o.stdout.find("three....").unwrap();
    assert!(one < two && two < three, "{}", o.stdout);
    assert!(o.stdout.contains("\n\nA\n\n") && o.stdout.contains("\n\nB\n\n"));
}

#[test]
fn priority_reorders_execution_not_output() {
    let h = H::new();
    let flag = h.path("first.flag");
    h.config(&local(&format!(
        "{}{}",
        sys(
            "late",
            "late",
            &format!("sh -c 'test -e {}' --", flag.display()),
            "priority: 10\nalways_run: true\npass_filenames: false"
        ),
        sys(
            "early",
            "early",
            &format!("sh -c 'touch {}' --", flag.display()),
            "priority: 1\nalways_run: true\npass_filenames: false"
        ),
    )));
    let o = h.run(args());
    assert_eq!(o.code, 0, "{}", o.stdout);
    assert_eq!(
        o.stdout,
        format!("{}{}", line("late", "Passed"), line("early", "Passed"))
    );
}

#[test]
fn jobs_1_and_jobs_32_print_the_same_bytes() {
    let cfg = local(&format!(
        "{}{}{}",
        sys(
            "a",
            "a",
            "sh -c 'echo a; for f; do echo $f; done' --",
            "priority: 0\nverbose: true"
        ),
        sys("b", "b", "sh -c 'echo b; exit 1' --", "priority: 0"),
        sys("c", "c", "echo c", "priority: 1\nverbose: true"),
    ));
    let mut outs = Vec::new();
    for jobs in [1, 32] {
        let mut h = H::new();
        h.jobs = jobs;
        h.config(&cfg);
        for i in 0..20 {
            staged(&h, &format!("f{i}"), b"x");
        }
        let o = h.run(args());
        let norm: String = o
            .stdout
            .lines()
            .filter(|l| !l.starts_with("- duration"))
            .collect::<Vec<_>>()
            .join("\n");
        outs.push((o.code, norm));
    }
    assert_eq!(outs[0], outs[1]);
}

/// A fixer that rewrites `bad` to `good`, exiting 1 when it fixed something.
const FIX: &str = "sh -c 'r=0; for f; do if grep -q bad \"$f\"; then sed -i s/bad/good/ \"$f\"; r=1; fi; done; exit $r' --";

#[test]
fn stage_fixed_verify_stages_the_fixes() {
    let h = H::new();
    h.config(&local(&sys("fix", "fix", FIX, "stage_fixed: true")));
    staged(&h, "a.txt", b"bad\n");
    let o = h.run(args());
    assert_eq!(o.code, 0, "{o:?}");
    assert_eq!(
        o.stdout,
        format!(
            "{}- hook id: fix\n- files were modified by this hook and staged\n",
            line("fix", "Passed")
        )
    );
    let cached = String::from_utf8(h.git(&["show", ":a.txt"]).stdout).unwrap();
    assert_eq!(cached, "good\n");
    assert_eq!(h.git(&["diff"]).stdout, b"");
}

#[test]
fn stage_fixed_with_unstaged_changes_applies_cleanly_or_refuses() {
    // the unstaged change is in another file: the fix is staged, the unstaged work restored
    let h = H::new();
    h.config(&local(&sys(
        "fix",
        "fix",
        FIX,
        "stage_fixed: trust\nfiles: ^a",
    )));
    staged(&h, "a.txt", b"bad\n");
    staged(&h, "b.txt", b"b\n");
    h.write("b.txt", b"b\nunstaged\n");
    let o = h.run(args());
    assert_eq!(o.code, 0, "{o:?}");
    assert_eq!(
        String::from_utf8(h.git(&["show", ":a.txt"]).stdout).unwrap(),
        "good\n"
    );
    assert_eq!(h.read("b.txt"), "b\nunstaged\n");
    // the unstaged change conflicts with the fix: nothing is staged, the run fails closed
    let h = H::new();
    h.config(&local(&sys("fix", "fix", FIX, "stage_fixed: trust")));
    staged(&h, "a.txt", b"bad\nline\n");
    h.write("a.txt", b"bad\nline\nunstaged\n");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(
        o.stderr
            .contains("snp: stage_fixed: the unstaged changes would not apply over the fixes")
    );
    assert_eq!(
        String::from_utf8(h.git(&["show", ":a.txt"]).stdout).unwrap(),
        "bad\nline\n"
    );
    assert_eq!(h.read("a.txt"), "bad\nline\nunstaged\n");
}

#[test]
fn stage_fixed_never_stages_files_another_hook_touched() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys("fix", "fix", FIX, "stage_fixed: trust"),
        sys("other", "other", "sh -c 'echo more >> a.txt' --", ""),
    )));
    staged(&h, "a.txt", b"bad\n");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stdout.contains(&line("other", "Failed")));
    assert_eq!(
        String::from_utf8(h.git(&["show", ":a.txt"]).stdout).unwrap(),
        "bad\n"
    );
}

#[test]
fn timeout_kills_the_hook_and_its_grandchildren() {
    let h = H::new();
    h.config(&local(&sys(
        "slow",
        "slow",
        "sh -c 'sleep 30 & wait' --",
        "timeout: 500ms\nalways_run: true\npass_filenames: false",
    )));
    let t0 = Instant::now();
    let o = h.run(args());
    assert!(
        t0.elapsed() < Duration::from_secs(10),
        "took {:?}",
        t0.elapsed()
    );
    assert_eq!(o.code, 1, "{o:?}");
    assert!(
        o.stdout.starts_with(&line("slow", "Failed")),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("- hook timed out after 500ms\n"),
        "{}",
        o.stdout
    );
}

#[test]
fn groups_and_selectors() {
    let mut h = H::new();
    h.brand = Brand::Snp;
    h.config(&local(&format!(
        "{}{}{}{}",
        sys(
            "lint:js",
            "lint js",
            "'true'",
            "groups: [lint, js]\nalways_run: true"
        ),
        sys(
            "lint:py",
            "lint py",
            "'true'",
            "groups: [lint, py]\nalways_run: true"
        ),
        sys("slow", "slow", "'true'", "groups: [slow]\nalways_run: true"),
        sys("plain", "plain", "'true'", "always_run: true"),
    )));
    let names = |a: snp_cli::RunArgs| -> Vec<String> {
        h.run(a)
            .stdout
            .lines()
            .map(|l| l.split('.').next().unwrap().to_owned())
            .collect()
    };
    let mut a = args();
    a.hooks = vec![OsString::from("lint:js")];
    assert_eq!(names(a), ["lint js"]);
    let mut a = args();
    a.ext.groups = vec!["js".into(), "py".into()];
    assert_eq!(names(a), ["lint js", "lint py"]);
    let mut a = args();
    a.ext.require_groups = vec!["lint".into(), "py".into()];
    assert_eq!(names(a), ["lint py"]);
    let mut a = args();
    a.ext.groups = vec!["lint".into()];
    a.ext.no_groups = vec!["js".into()];
    assert_eq!(names(a), ["lint py"]);
    let mut a = args();
    a.ext.groups = vec!["@ungrouped".into()];
    assert_eq!(names(a), ["plain"]);
    h.env_set("SNP_SKIP_GROUPS", "slow");
    let o = h.run(args());
    assert!(o.stdout.contains(&line("slow", "Skipped")), "{}", o.stdout);
}

#[test]
fn skip_and_only_conditions() {
    let mut h = H::new();
    h.config(&local(&format!(
        "{}{}{}",
        sys(
            "e",
            "skip on env",
            "'true'",
            "skip: {env: SKIPME}\nalways_run: true"
        ),
        sys(
            "r",
            "only on main",
            "'true'",
            "only: {ref: main}\nalways_run: true"
        ),
        sys(
            "x",
            "only on rel",
            "'true'",
            "only: [{ref: 'rel/*'}, {run: 'false'}]\nalways_run: true"
        ),
    )));
    let o = h.run(args());
    assert_eq!(
        o.stdout,
        format!(
            "{}{}{}",
            line("skip on env", "Passed"),
            line("only on main", "Passed"),
            line("only on rel", "Skipped")
        )
    );
    h.env_set("SKIPME", "1");
    let o = h.run(args());
    assert!(o.stdout.starts_with(&line("skip on env", "Skipped")));
    // root conditions skip every hook
    let h = H::new();
    h.config(&format!(
        "skip: {{run: 'true'}}\n{}",
        local(&sys("a", "a", "'false'", "always_run: true"))
    ));
    let o = h.run(args());
    assert_eq!((o.code, o.stdout), (0, line("a", "Skipped")));
}

#[test]
fn fail_on_changes_policies() {
    let modifier = |policy: &str| {
        local(&sys(
            "m",
            "modifier",
            "sh -c 'for f; do echo x >> \"$f\"; done' --",
            &format!("fail_on_changes: {policy}"),
        ))
    };
    let mut h = H::new();
    h.config(&modifier("never"));
    staged(&h, "a.txt", b"a\n");
    let o = h.run(args());
    assert_eq!(o.code, 0, "{}", o.stdout);
    assert_eq!(
        o.stdout,
        format!(
            "{}- hook id: m\n- files were modified by this hook\n",
            line("modifier", "Passed")
        )
    );
    h.config(&modifier("ci"));
    staged(&h, "a.txt", b"b\n");
    h.env_set("CI", "true");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    h.env_set("CI", "0");
    staged(&h, "a.txt", b"c\n");
    assert_eq!(h.run(args()).code, 0);
}

#[test]
fn root_runs_in_the_subdirectory_with_relative_files() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys(
            "r",
            "rooted",
            "sh -c 'pwd; for f; do echo $f; done' --",
            "root: sub\nverbose: true"
        ),
        sys(
            "m",
            "missing root",
            "'true'",
            "root: nowhere\nalways_run: true"
        ),
    )));
    staged(&h, "sub/x.py", b"");
    staged(&h, "subway/y.py", b"");
    staged(&h, "top.py", b"");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{}", o.stdout);
    let sub = h.path("sub");
    assert!(
        o.stdout
            .contains(&format!("\n\n{}\nx.py\n\n", sub.display())),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains(&format!(
            "{}- hook id: m\n- exit code: 1\n- root directory 'nowhere' does not exist\n",
            line("missing root", "Failed")
        )),
        "{}",
        o.stdout
    );
}

#[test]
fn env_and_fail_text() {
    let h = H::new();
    h.config(&local(&sys(
        "e",
        "env",
        "sh -c 'echo $FOO; exit 1' --",
        "env: {FOO: bar}\nfail_text: run make fmt\nalways_run: true\npass_filenames: false",
    )));
    let o = h.run(args());
    assert_eq!(
        o.stdout,
        format!(
            "{}- hook id: e\n- exit code: 1\n- run make fmt\n\nbar\n\n",
            line("env", "Failed")
        )
    );
}

#[test]
fn use_stdin_gets_the_pre_push_lines() {
    let h = H::new();
    h.config(&local(&sys(
        "s",
        "stdin",
        "cat",
        "use_stdin: true\nalways_run: true\npass_filenames: false\nverbose: true\nstages: [pre-push]",
    )));
    let remote = h.base.join("remote.git");
    std::process::Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&remote)
        .output()
        .unwrap();
    h.git(&["remote", "add", "origin", &remote.to_string_lossy()]);
    h.git(&["push", "-q", "origin", "main"]);
    let first = String::from_utf8(h.git(&["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    staged(&h, "b", b"b");
    h.commit("two");
    let second = String::from_utf8(h.git(&["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let stdin = format!("refs/heads/main {second} refs/heads/main {first}\n");
    let o = h.hook_impl(
        crate::harness::hook_impl_args("pre-push", &["--", "origin", "u"], None),
        stdin.as_bytes(),
    );
    assert_eq!(o.code, 0, "{o:?}");
    assert!(
        o.stdout.ends_with(&format!("\n\n{}\n\n", stdin.trim_end())),
        "{}",
        o.stdout
    );
}

#[test]
fn local_overlay_patches_hooks() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys(
            "a",
            "a",
            "echo base",
            "always_run: true\npass_filenames: false\nverbose: true"
        ),
        sys("b", "b", "'true'", "always_run: true"),
    )));
    h.write(
        ".pre-commit-config.local.yaml",
        b"hooks:\n  a:\n    entry: \"echo wrapped {entry}\"\n  b:\n    skip: true\n",
    );
    let o = h.run(args());
    assert_eq!(o.code, 0, "{o:?}");
    assert!(
        o.stdout.contains("\n\nwrapped echo base\n\n"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.ends_with(&line("b", "Skipped")), "{}", o.stdout);
    assert!(
        o.stderr.contains("snp: using local overrides from"),
        "{}",
        o.stderr
    );
    // --no-local-config
    let mut a = args();
    a.ext.no_local_config = true;
    let o = h.run(a);
    assert!(o.stdout.contains("\n\nbase\n\n"));
}

#[test]
fn output_modes() {
    let mut h = H::new();
    h.config(&format!(
        "output: summary\n{}",
        local(&format!(
            "{}{}",
            sys("p", "pass", "'true'", "always_run: true"),
            sys("f", "fail", "'false'", "always_run: true"),
        ))
    ));
    let o = h.run(args());
    assert_eq!(o.code, 1);
    assert!(
        o.stdout.contains(&format!("{}\nSummary: ", rep("=", 79))),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("1 passed, 1 failed, 0 skipped in "),
        "{}",
        o.stdout
    );
    h.env_set("SNP_OUTPUT", "quiet");
    let o = h.run(args());
    assert_eq!(
        o.stdout,
        format!("{}- hook id: f\n- exit code: 1\n", line("fail", "Failed"))
    );
}

#[test]
fn compat_strict_turns_extensions_into_unknown_keys() {
    let mut h = H::new();
    h.config(&local(&sys(
        "e",
        "env",
        "sh -c 'echo \"[$FOO]\"' --",
        "env: {FOO: bar}\nalways_run: true\nverbose: true",
    )));
    h.env_set("SNP_COMPAT_STRICT", "1");
    let o = h.run(args());
    assert!(
        o.stdout
            .starts_with("[WARNING] Unexpected key(s) present on local => e: env\n"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("\n\n[]\n\n"), "{}", o.stdout);
}

#[test]
fn baseline_after_a_slow_install_counts_preexisting_changes() {
    let h = H::new();
    h.config(&local(&sys("p", "pass", "'true'", "")));
    staged(&h, "a.txt", b"a\n");
    *h.fake.installs.delay.lock().unwrap() = Some(Duration::from_millis(400));
    *h.fake.installs.touch.lock().unwrap() = Some(h.path("a.txt"));
    let o = h.run(args());
    assert_eq!(
        o.code, 0,
        "an autosave during a slow install is not blamed on the hook: {o:?}"
    );
    assert_eq!(o.stdout, line("pass", "Passed"));
}

#[test]
fn install_hooks_installs_every_hook() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys("a", "a", "'true'", "stages: [pre-push]"),
        sys("b", "b", "'true'", ""),
    )));
    let o = h.install_hooks();
    assert_eq!((o.code, o.stdout.as_str()), (0, ""));
    assert_eq!(h.installed(), vec![vec!["a".to_owned(), "b".to_owned()]]);
}

#[test]
fn list_text_and_json() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys("lint", "Lint", "'true'", "groups: [fast]"),
        sys("push-only", "Push", "'true'", "stages: [pre-push]"),
    )));
    let la = |json: bool, stage: Option<snp_core::stage::Stage>, groups: Vec<OsString>| {
        snp_cli::ListArgs {
            config: ".pre-commit-config.yaml".into(),
            json,
            stage,
            groups,
            require_groups: Vec::new(),
            no_groups: Vec::new(),
        }
    };
    let o = h.list(la(false, None, Vec::new()));
    assert_eq!(o.stdout, "lint       Lint [fast]\npush-only  Push\n");
    let o = h.list(la(false, None, vec!["fast".into()]));
    assert_eq!(o.stdout, "lint  Lint [fast]\n");
    let o = h.list(la(
        true,
        Some(snp_core::stage::Stage::Hook(
            snp_core::stage::HookType::PreCommit,
        )),
        vec!["@ungrouped".into()],
    ));
    assert_eq!(o.stdout, "[]\n");
    let o = h.list(la(
        true,
        Some(snp_core::stage::Stage::Hook(
            snp_core::stage::HookType::PrePush,
        )),
        vec!["@ungrouped".into()],
    ));
    assert_eq!(
        o.stdout,
        "[\n  {\"id\": \"push-only\", \"alias\": \"\", \"name\": \"Push\", \"repo\": \"local\", \"language\": \
         \"unsupported\", \"stages\": [\"pre-push\"], \"groups\": []}\n]\n"
    );
}

/// The index content of `path`.
fn cached(h: &H, path: &str) -> String {
    String::from_utf8(h.git(&["show", &format!(":{path}")]).stdout).unwrap()
}

#[test]
fn stage_fixed_is_withdrawn_when_a_final_bucket_changes_the_file() {
    // the bucket is the last priority: no later sequential hook would catch the second change
    let h = H::new();
    h.config(&local(&format!(
        "{}{}{}",
        sys("fix", "fix", FIX, "stage_fixed: trust\npriority: 1"),
        sys(
            "more",
            "more",
            "sh -c 'echo INJECTED >> a.txt' --",
            "priority: 2\npass_filenames: false\nalways_run: true"
        ),
        sys(
            "p1",
            "p1",
            "'true'",
            "priority: 2\npass_filenames: false\nalways_run: true"
        ),
    )));
    staged(&h, "a.txt", b"bad\n");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stdout.contains(&line("more", "Failed")), "{}", o.stdout);
    assert_eq!(cached(&h, "a.txt"), "bad\n", "{o:?}");
    assert!(
        o.stderr
            .contains("snp: stage_fixed: a.txt was changed by a later hook; not staged\n"),
        "{}",
        o.stderr
    );
    // a failing bucket member that appends to the accepted file
    let h = H::new();
    h.config(&local(&format!(
        "{}{}{}",
        sys("fix", "fix", FIX, "stage_fixed: trust\npriority: 0"),
        sys(
            "p1",
            "p1",
            "sh -c 'echo EVIL >> a.txt; exit 1' --",
            "priority: 1\npass_filenames: false\nalways_run: true"
        ),
        sys(
            "p2",
            "p2",
            "sh -c 'exit 0' --",
            "priority: 1\npass_filenames: false\nalways_run: true"
        ),
    )));
    staged(&h, "a.txt", b"bad\n");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert_eq!(cached(&h, "a.txt"), "bad\n", "{o:?}");
}

#[test]
fn stage_fixed_withdrawn_by_a_later_sequential_hook_says_so() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys("fix", "fix", FIX, "stage_fixed: trust"),
        sys(
            "later",
            "later",
            "sh -c 'echo more >> a.txt' --",
            "pass_filenames: false\nalways_run: true"
        ),
    )));
    staged(&h, "a.txt", b"bad\n");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert_eq!(cached(&h, "a.txt"), "bad\n");
    assert!(
        o.stderr
            .contains("snp: stage_fixed: a.txt was changed by a later hook; not staged\n"),
        "{}",
        o.stderr
    );
}

#[test]
fn bucket_blames_a_writer_without_filenames() {
    // `writer` gets no file list, so it could have written x.py: the linter alone is never blamed
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys(
            "writer",
            "writer",
            "sh -c 'echo w >> x.py' --",
            "priority: 1\nalways_run: true\npass_filenames: false"
        ),
        sys("lint", "lint", "'true'", "priority: 1\nfiles: \\.py$"),
    )));
    staged(&h, "x.py", b"x = 1\n");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stdout.contains(&line("writer", "Failed")), "{}", o.stdout);
    // a bucket of constrained writers still attributes to the single owner
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys(
            "fa",
            "fa",
            "sh -c 'sed -i s/bad/good/ \"$@\"' --",
            "priority: 1\nfiles: ^a"
        ),
        sys("fb", "fb", "'true'", "priority: 1\nfiles: ^b"),
    )));
    staged(&h, "a.txt", b"bad\n");
    staged(&h, "b.txt", b"b\n");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stdout.contains(&line("fa", "Failed")), "{}", o.stdout);
    assert!(o.stdout.contains(&line("fb", "Passed")), "{}", o.stdout);
}

#[test]
fn stage_fixed_works_inside_a_priority_bucket() {
    let h = H::new();
    h.config(&local(&format!(
        "{}{}{}",
        sys(
            "fa",
            "fa",
            FIX,
            "stage_fixed: trust\npriority: 0\nfiles: ^a"
        ),
        sys(
            "fb",
            "fb",
            FIX,
            "stage_fixed: trust\npriority: 0\nfiles: ^b"
        ),
        sys("c", "c", "'true'", "priority: 0\nfiles: ^c"),
    )));
    staged(&h, "a.txt", b"bad\n");
    staged(&h, "b.txt", b"bad\n");
    staged(&h, "c.txt", b"c\n");
    let o = h.run(args());
    assert_eq!(o.code, 0, "{o:?}");
    let staged_line = "- files were modified by this hook and staged\n";
    assert_eq!(
        o.stdout,
        format!(
            "{}- hook id: fa\n{staged_line}{}- hook id: fb\n{staged_line}{}",
            line("fa", "Passed"),
            line("fb", "Passed"),
            line("c", "Passed")
        )
    );
    assert_eq!(cached(&h, "a.txt"), "good\n");
    assert_eq!(cached(&h, "b.txt"), "good\n");
}

#[test]
fn stage_fixed_combines_with_fail_on_changes() {
    for (key, ci, fails) in [
        ("always", false, true),
        ("ci", true, true),
        ("ci", false, false),
        ("never", true, false),
    ] {
        let mut h = H::new();
        if ci {
            h.env_set("CI", "1");
        }
        h.config(&local(&sys(
            "fix",
            "fix",
            FIX,
            &format!("stage_fixed: trust\nfail_on_changes: {key}"),
        )));
        staged(&h, "a.txt", b"bad\n");
        let o = h.run(args());
        let status = if fails { "Failed" } else { "Passed" };
        assert_eq!(o.code, i32::from(fails), "{key} CI={ci}: {o:?}");
        assert!(
            o.stdout.starts_with(&line("fix", status))
                && o.stdout
                    .contains("- files were modified by this hook and staged\n"),
            "{key} CI={ci}: {}",
            o.stdout
        );
    }
    // the root default (`always`, pre-commit's) does not fail an accepted fix
    let h = H::new();
    h.config(&local(&sys("fix", "fix", FIX, "stage_fixed: trust")));
    staged(&h, "a.txt", b"bad\n");
    assert_eq!(h.run(args()).code, 0);
    // an explicit root value combines too
    let mut h = H::new();
    h.env_set("CI", "true");
    h.config(&format!(
        "fail_on_changes: ci\n{}",
        local(&sys("fix", "fix", FIX, "stage_fixed: trust"))
    ));
    staged(&h, "a.txt", b"bad\n");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
}

#[test]
fn stage_fixed_counts_further_changes_to_already_dirty_files() {
    // `dirt` dirtied b.txt; `fx` fixes its input a.txt and also changes b.txt again: O ≠ ∅ ⇒ normal failure
    let h = H::new();
    h.config(&local(&format!(
        "{}{}",
        sys(
            "dirt",
            "dirt",
            "sh -c 'echo x >> b.txt; exit 1' --",
            "pass_filenames: false\nalways_run: true"
        ),
        sys(
            "fx",
            "fx",
            "sh -c 'sed -i s/bad/good/ a.txt; echo y >> b.txt; exit 1' --",
            "stage_fixed: trust\nfiles: ^a"
        ),
    )));
    staged(&h, "a.txt", b"bad\n");
    staged(&h, "b.txt", b"b\n");
    let o = h.run(args());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stdout.contains(&line("fx", "Failed")), "{}", o.stdout);
    assert!(!o.stdout.contains("and staged"), "{}", o.stdout);
    assert_eq!(cached(&h, "a.txt"), "bad\n");
    assert_eq!(cached(&h, "b.txt"), "b\n");
}

#[test]
fn a_large_bucket_runs_on_a_bounded_set_of_threads() {
    let mut h = H::new();
    h.jobs = 1;
    let hooks: String = (0..40)
        .map(|i| {
            sys(
                &format!("h{i:02}"),
                &format!("h{i:02}"),
                "'true'",
                "priority: 0\nalways_run: true\npass_filenames: false",
            )
        })
        .collect();
    h.config(&local(&hooks));
    let o = h.run(args());
    assert_eq!(o.code, 0, "{o:?}");
    let want: String = (0..40)
        .map(|i| line(&format!("h{i:02}"), "Passed"))
        .collect();
    assert_eq!(o.stdout, want);
}

#[test]
fn local_overlay_tracked_check_is_literal() {
    // the config lives in `sub[1]/`; a tracked `sub1/.pre-commit-config.local.yaml` must not count as the overlay
    let h = H::new();
    let cfg = local(&sys(
        "a",
        "a",
        "echo base",
        "always_run: true\npass_filenames: false\nverbose: true",
    ));
    h.write("sub[1]/cfg.yaml", cfg.as_bytes());
    h.stage("sub[1]/cfg.yaml");
    h.write(
        "sub1/.pre-commit-config.local.yaml",
        b"hooks:\n  a:\n    entry: \"echo tracked {entry}\"\n",
    );
    h.stage("sub1/.pre-commit-config.local.yaml");
    h.commit("configs");
    h.write(
        "sub[1]/.pre-commit-config.local.yaml",
        b"hooks:\n  a:\n    entry: \"echo wrapped {entry}\"\n",
    );
    let mut a = args();
    a.config = OsString::from("sub[1]/cfg.yaml");
    let o = h.run(a);
    assert_eq!(o.code, 0, "{o:?}");
    assert!(
        o.stdout.contains("\n\nwrapped echo base\n\n"),
        "{}\n{}",
        o.stdout,
        o.stderr
    );
}
