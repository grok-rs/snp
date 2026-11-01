//! `hook-impl` (T10-HKI, X13-HI; D-SEC-12, D-EXT-05, D-DATA-07, D-CRASH-14).

use std::path::Path;

use snp_core::failure::Failure;

use crate::harness::{H, hook_impl_args, line, local, rep, sys};

/// The sample local config of HKI-08.
const SAMPLE: &str = "repos:\n-   repo: local\n    hooks:\n    -   id: do_not_commit\n        name: Block if \"DO NOT COMMIT\" is found\n        entry: DO NOT COMMIT\n        language: pygrep\n";

fn hooks_dir(h: &H) -> std::path::PathBuf {
    let d = h.path(".git/hooks");
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn t10_hki_08_nothing_staged() {
    let h = H::new();
    h.config(SAMPLE);
    let o = h.hook_impl(
        hook_impl_args("pre-commit", &["--"], Some(&hooks_dir(&h))),
        b"",
    );
    assert_eq!(o.code, 0, "{o:?}");
    assert_eq!(
        o.stdout,
        "Block if \"DO NOT COMMIT\" is found....................(no files to check)Skipped\n"
    );
}

#[test]
fn t10_hki_09_hook_dir_is_optional() {
    let h = H::new();
    h.config(SAMPLE);
    let o = h.hook_impl(hook_impl_args("pre-commit", &["--"], None), b"");
    assert_eq!(o.code, 0, "{o:?}");
}

#[test]
fn t10_hki_01_missing_config() {
    let mut h = H::new();
    let mut a = hook_impl_args("pre-commit", &["--"], None);
    a.config = "DNE.yaml".into();
    let o = h.hook_impl(a.clone(), b"");
    assert_eq!(o.code, 1);
    assert_eq!(
        o.stdout,
        "No DNE.yaml file was found\n- To temporarily silence this, run `PRE_COMMIT_ALLOW_NO_CONFIG=1 git ...`\n\
         - To permanently silence this, install pre-commit with the --allow-missing-config option\n\
         - To uninstall pre-commit run `pre-commit uninstall`\n"
    );
    // skip: exit with the legacy hook's code
    let dir = hooks_dir(&h);
    h.exec(".git/hooks/pre-commit.legacy", b"#!/bin/sh\nexit 123\n");
    let mut b = hook_impl_args("pre-commit", &["--"], Some(&dir));
    b.config = "DNE.yaml".into();
    b.skip_on_missing_config = true;
    let o = h.hook_impl(b, b"");
    assert_eq!(
        (o.code, o.stdout.as_str()),
        (
            123,
            "`DNE.yaml` config file not found. Skipping `pre-commit`.\n"
        )
    );
    h.env_set("PRE_COMMIT_ALLOW_NO_CONFIG", "1");
    let o = h.hook_impl(a, b"");
    assert_eq!(
        (o.code, o.stdout.as_str()),
        (
            0,
            "`DNE.yaml` config file not found. Skipping `pre-commit`.\n"
        )
    );
}

#[test]
fn t10_hki_02_legacy_hook() {
    let h = H::new();
    h.config(SAMPLE);
    let dir = hooks_dir(&h);
    let log = h.path("legacy.log");
    h.exec(
        ".git/hooks/pre-commit.legacy",
        format!(
            "#!/bin/sh\necho \"hi $* $PRE_COMMIT_RUNNING_LEGACY\" > '{}'\nexit 1\n",
            log.display()
        )
        .as_bytes(),
    );
    let o = h.hook_impl(hook_impl_args("pre-commit", &["--"], Some(&dir)), b"");
    assert_eq!(o.code, 1, "{o:?}");
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "hi  1\n");
    // pre-push: the legacy hook gets the whole stdin
    let h = H::new();
    h.config(SAMPLE);
    let dir = hooks_dir(&h);
    let got = h.path("stdin.txt");
    h.exec(
        ".git/hooks/pre-push.legacy",
        format!("#!/bin/sh\ncat > '{}'\n", got.display()).as_bytes(),
    );
    let z = rep("0", 40);
    let stdin = format!("(delete) {z} refs/heads/b {}\n", rep("1", 40));
    let o = h.hook_impl(
        hook_impl_args("pre-push", &["--", "origin", "url"], Some(&dir)),
        stdin.as_bytes(),
    );
    assert_eq!(o.code, 0, "{o:?}");
    assert_eq!(std::fs::read_to_string(&got).unwrap(), stdin);
}

#[test]
fn x13_hi_08_migration_mode_recursion() {
    let mut h = H::new();
    h.config(SAMPLE);
    h.env_set("PRE_COMMIT_RUNNING_LEGACY", "1");
    let o = h.hook_impl(hook_impl_args("pre-commit", &["--"], None), b"");
    match o.failure {
        Some(Failure::SystemExit { code: 1, stderr }) => assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "bug: pre-commit's script is installed in migration mode\nrun `pre-commit install -f --hook-type \
             pre-commit` to fix this\n\nPlease report this bug at https://github.com/pre-commit/pre-commit/issues\n"
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn x13_hi_01_unknown_type_after_legacy_and_config() {
    let h = H::new();
    h.config(SAMPLE);
    let dir = hooks_dir(&h);
    let marker = h.path("ran");
    h.exec(
        ".git/hooks/bogus.legacy",
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()).as_bytes(),
    );
    let o = h.hook_impl(hook_impl_args("bogus", &["--"], Some(&dir)), b"");
    assert_eq!(o.code, 3);
    assert!(marker.exists(), "the legacy hook runs first");
    match o.failure {
        Some(Failure::Exc { exc, .. }) => {
            assert_eq!(exc.ty, "AssertionError");
            assert_eq!(exc.text, b"unexpected hook type: bogus");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn x13_hi_02_wrong_argument_count() {
    let h = H::new();
    h.config(SAMPLE);
    let o = h.hook_impl(hook_impl_args("pre-push", &["--", "origin"], None), b"");
    match o.failure {
        Some(Failure::SystemExit { code: 1, stderr }) => assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "hook-impl for pre-push expected 2 arguments but got 1: ['origin']\n"
        ),
        other => panic!("{other:?}"),
    }
    // `rest[1:]` drops the first token whatever it is
    let o = h.hook_impl(
        hook_impl_args("commit-msg", &[".git/COMMIT_EDITMSG"], None),
        b"",
    );
    match o.failure {
        Some(Failure::SystemExit { stderr, .. }) => {
            assert_eq!(
                stderr,
                b"hook-impl for commit-msg expected 1 argument but got 0: []\n"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn x13_hi_07_broken_legacy_hook_fails_but_hooks_run() {
    let h = H::new();
    h.config(&local(&sys("p", "passing", "'true'", "always_run: true")));
    let dir = hooks_dir(&h);
    h.exec(".git/hooks/pre-commit.legacy", b"#!/nonexistent/interp\n");
    let o = h.hook_impl(hook_impl_args("pre-commit", &["--"], Some(&dir)), b"");
    assert_eq!(o.code, 1, "{o:?}");
    assert_eq!(o.stderr, "Executable `/nonexistent/interp` not found\n");
    assert_eq!(o.stdout, line("passing", "Passed"));
    // a directory passes os.access(X_OK)
    std::fs::remove_file(dir.join("pre-commit.legacy")).unwrap();
    std::fs::create_dir_all(dir.join("pre-commit.legacy")).unwrap();
    let o = h.hook_impl(hook_impl_args("pre-commit", &["--"], Some(&dir)), b"");
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("is a directory"), "{}", o.stderr);
}

#[test]
fn d_data_07_cdpath_doubled_hook_dir() {
    let h = H::new();
    h.config(SAMPLE);
    let dir = hooks_dir(&h);
    let marker = h.path("ran");
    h.exec(
        ".git/hooks/pre-commit.legacy",
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()).as_bytes(),
    );
    let doubled = format!("{0}\n{0}", dir.display());
    let o = h.hook_impl(
        hook_impl_args("pre-commit", &["--"], Some(Path::new(&doubled))),
        b"",
    );
    assert_eq!(o.code, 0, "{o:?}");
    assert!(marker.exists());
}

#[test]
fn d_sec_12_huge_pre_push_stdin_fails_closed_before_the_legacy_hook() {
    let h = H::new();
    h.config(SAMPLE);
    let dir = hooks_dir(&h);
    let marker = h.path("ran");
    h.exec(
        ".git/hooks/pre-push.legacy",
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()).as_bytes(),
    );
    let big = vec![b'x'; 65 << 20];
    let o = h.hook_impl(
        hook_impl_args("pre-push", &["--", "origin", "url"], Some(&dir)),
        &big,
    );
    assert_eq!(o.code, 1);
    assert!(!marker.exists());
    match o.failure {
        Some(Failure::Exc { exc, .. }) => {
            assert_eq!(exc.ty, "FatalError");
            assert_eq!(
                exc.text,
                b"snp: hook stdin exceeds 64 MiB; refusing to run hooks on a truncated ref list"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn d_ext_05_kill_switch() {
    let mut h = H::new();
    h.config(&local(&sys("p", "passing", "'true'", "always_run: true")));
    let dir = hooks_dir(&h);
    let marker = h.path("ran");
    h.exec(
        ".git/hooks/pre-push.legacy",
        format!(
            "#!/bin/sh\ncat > /dev/null; touch '{}'; exit 4\n",
            marker.display()
        )
        .as_bytes(),
    );
    h.env_set("SNP", "0");
    let o = h.hook_impl(
        hook_impl_args("pre-push", &["--", "origin", "url"], Some(&dir)),
        b"",
    );
    assert_eq!((o.code, o.stdout.as_str()), (4, ""));
    assert!(
        marker.exists(),
        "a git-lfs-like legacy pre-push hook still runs"
    );
    // allow_disable: false overrides
    h.config(&format!(
        "allow_disable: false\n{}",
        local(&sys(
            "p",
            "passing",
            "'true'",
            "always_run: true\nstages: [pre-commit]"
        ))
    ));
    let o = h.hook_impl(hook_impl_args("pre-commit", &["--"], None), b"");
    assert_eq!(o.code, 0, "{o:?}");
    assert_eq!(o.stdout, line("passing", "Passed"));
    assert_eq!(
        o.stderr,
        "snp: SNP=0 ignored: .pre-commit-config.yaml sets allow_disable: false\n"
    );
}

/// A repo with a remote `origin` holding the first commit (HKI-06/07, X13-HI-05).
fn pushed() -> (H, String, String) {
    let h = H::new();
    h.config(&local(&sys(
        "env",
        "env",
        "sh -c 'echo \"$PRE_COMMIT_FROM_REF..$PRE_COMMIT_TO_REF $PRE_COMMIT_REMOTE_NAME $PRE_COMMIT_LOCAL_BRANCH\"' --",
        "always_run: true\nverbose: true\npass_filenames: false\nstages: [pre-push]",
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
    h.write("b", b"b");
    h.stage("b");
    h.commit("second");
    let second = String::from_utf8(h.git(&["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    (h, first, second)
}

#[test]
fn t10_hki_06_pre_push_namespaces() {
    let (h, first, second) = pushed();
    let stdin = format!("refs/heads/main {second} refs/heads/main {first}\n");
    let o = h.hook_impl(
        hook_impl_args("pre-push", &["--", "origin", "url"], None),
        stdin.as_bytes(),
    );
    assert_eq!(o.code, 0, "{o:?}");
    assert!(
        o.stdout
            .contains(&format!("\n\n{first}..{second} origin refs/heads/main\n\n")),
        "{}",
        o.stdout
    );
    // a new branch: from = the parent of the first commit not on the remote
    let z = rep("0", 40);
    let stdin = format!("refs/heads/feat {second} refs/heads/feat {z}\n");
    let o = h.hook_impl(
        hook_impl_args("pre-push", &["--", "origin", "url"], None),
        stdin.as_bytes(),
    );
    assert!(
        o.stdout
            .contains(&format!("\n\n{first}..{second} origin refs/heads/feat\n\n")),
        "{}",
        o.stdout
    );
    // a new branch at an already pushed revision: nothing to push, no run
    let stdin = format!("refs/heads/old {first} refs/heads/old {z}\n");
    let o = h.hook_impl(
        hook_impl_args("pre-push", &["--", "origin", "url"], None),
        stdin.as_bytes(),
    );
    assert_eq!((o.code, o.stdout.as_str()), (0, ""));
}

#[test]
fn t10_hki_07_delete_branch_push_prints_nothing() {
    let (h, first, _) = pushed();
    let z = rep("0", 40);
    let stdin = format!("(delete) {z} refs/heads/b {first}\n");
    let o = h.hook_impl(
        hook_impl_args("pre-push", &["--", "origin", "url"], None),
        stdin.as_bytes(),
    );
    assert_eq!((o.code, o.stdout.as_str(), o.stderr.as_str()), (0, "", ""));
}

#[test]
fn x13_hi_05_unknown_local_sha() {
    let (h, _, _) = pushed();
    let z = rep("0", 40);
    let bad = rep("1234567890", 4);
    let stdin = format!("refs/heads/x {bad} refs/heads/x {z}\n");
    let o = h.hook_impl(
        hook_impl_args("pre-push", &["--", "origin", "url"], None),
        stdin.as_bytes(),
    );
    assert_eq!(o.code, 3);
    match o.failure {
        Some(Failure::Exc { exc, .. }) => {
            assert_eq!(exc.ty, "CalledProcessError");
            assert_eq!(
                String::from_utf8_lossy(&exc.text),
                format!(
                    "Command '('git', 'rev-list', '{bad}', '--topo-order', '--reverse', '--not', '--remotes=origin')' \
                     returned non-zero exit status 128."
                )
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn hook_impl_commit_msg_runs_on_the_message_file() {
    let h = H::new();
    h.config(&local(&sys(
        "m",
        "msg",
        "sh -c 'cat \"$1\"' --",
        "stages: [commit-msg]\nverbose: true",
    )));
    h.write(".git/COMMIT_EDITMSG", b"hello\n");
    let o = h.hook_impl(
        hook_impl_args("commit-msg", &["--", ".git/COMMIT_EDITMSG"], None),
        b"",
    );
    assert_eq!(o.code, 0, "{o:?}");
    assert!(o.stdout.ends_with("\n\nhello\n\n"), "{}", o.stdout);
}

#[test]
fn hook_impl_rc_is_legacy_or_run() {
    let h = H::new();
    h.config(&local(&sys("f", "failing", "'false'", "always_run: true")));
    let dir = hooks_dir(&h);
    h.exec(".git/hooks/pre-commit.legacy", b"#!/bin/sh\nexit 4\n");
    let o = h.hook_impl(hook_impl_args("pre-commit", &["--"], Some(&dir)), b"");
    assert_eq!(o.code, 5);
}
