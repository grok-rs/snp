//! snp-testkit acceptance (WBS WP-18): deterministic SHAs, the hermetic environment, every fixture of 10 §3/§7.3,
//! the runners (snp, the `pre-commit` alias, the oracles), the insta filters and `Bins::locate`.
//!
//! Owner: WP-18.

#[cfg(test)]
mod testkit {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use snp_testkit::{Bins, Fixture, Oracle, TestEnv};

    fn no_bins() -> Bins {
        Bins {
            snp: PathBuf::from("/nonexistent/snp"),
            probe: PathBuf::from("/nonexistent/snp-probe"),
        }
    }

    /// git in a test repository, outside the kit (plain `Command`, hermetic env).
    fn git(env: &TestEnv, cwd: &Path, args: &[&str]) -> String {
        let exe = snp_proc::resolve::find_executable(OsStr::new("git"), env.env(), cwd).unwrap();
        let mut c = Command::new(exe);
        env.env().apply_to(&mut c);
        let out = c.args(args).current_dir(cwd).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn script(env: &TestEnv) -> PathBuf {
        env.git_repo("work")
            .file("a.txt", b"a\n")
            .exec_file("bin/run.sh", b"#!/bin/sh\necho hi\n")
            .symlink("link", "a.txt")
            .stage_all()
            .commit("first")
            .tag("v1")
            .file("b.txt", b"b\n")
            .stage("b.txt")
            .config("core.fileMode", "true")
            .commit("second")
            .build()
    }

    #[test]
    fn two_runs_of_the_same_script_give_identical_shas() {
        let (one, two) = (TestEnv::new(no_bins()), TestEnv::new(no_bins()));
        let (r1, r2) = (script(&one), script(&two));
        assert_ne!(one.root(), two.root());
        for rev in ["HEAD", "HEAD^", "v1", "HEAD^{tree}"] {
            assert_eq!(
                git(&one, &r1, &["rev-parse", rev]),
                git(&two, &r2, &["rev-parse", rev]),
                "{rev}"
            );
        }
        // the modes come from the builder, not the umask
        let ls = git(&one, &r1, &["ls-files", "-s"]);
        assert!(ls.contains("100755 ") && ls.contains("bin/run.sh"), "{ls}");
        assert!(ls.contains("120000 ") && ls.contains("link"), "{ls}");
        assert!(ls.contains("100644 ") && ls.contains("a.txt"), "{ls}");
        assert_eq!(
            git(&one, &r1, &["log", "-1", "--format=%an <%ae> %at|%s"]),
            "test <test@example.com> 1577836802|second"
        );
        assert_eq!(
            git(&one, &r1, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "master"
        );
        // an existing repository is extended
        let again = one
            .git_repo("work")
            .file("c.txt", b"c")
            .stage("c.txt")
            .commit("third")
            .build();
        assert_eq!(again, r1);
        assert_eq!(git(&one, &r1, &["rev-list", "--count", "HEAD"]), "3");
    }

    #[test]
    fn the_environment_is_hermetic() {
        let env = TestEnv::new(no_bins());
        let root = env.root();
        assert!(root.is_absolute() && root.is_dir());
        let vars: Vec<(String, String)> = env
            .env()
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        let get = |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        for (k, want) in [
            ("HOME", root.join("home")),
            ("XDG_CACHE_HOME", root.join("home/.cache")),
            ("PRE_COMMIT_HOME", root.join("store")),
            ("SNP_HOME", root.join("snp-store")),
            ("GIT_CONFIG_GLOBAL", root.join("home/.gitconfig")),
            ("GIT_TEMPLATE_DIR", root.join("git-template")),
            ("TMPDIR", root.join("tmp")),
        ] {
            assert_eq!(get(k).map(PathBuf::from), Some(want), "{k}");
        }
        for (k, want) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("LC_ALL", "C.UTF-8"),
            ("TZ", "UTC"),
            ("GIT_AUTHOR_NAME", "test"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
            ("GIT_AUTHOR_DATE", "1577836800 +0000"),
            ("GIT_ALLOW_PROTOCOL", "file"),
            ("VIRTUALENV_NO_DOWNLOAD", "1"),
            ("SNP_ALLOW_OLD_GIT", "1"),
        ] {
            assert_eq!(get(k).as_deref(), Some(want), "{k}");
        }
        let allowed = [
            "PATH",
            "SYSTEMROOT",
            "PATHEXT",
            "COMSPEC",
            "WINDIR",
            "TEMP",
            "TMP",
            "HOME",
            "USERPROFILE",
            "XDG_CACHE_HOME",
            "PRE_COMMIT_HOME",
            "SNP_HOME",
            "GIT_CONFIG_GLOBAL",
            "GIT_TEMPLATE_DIR",
            "TMPDIR",
            "GIT_CONFIG_NOSYSTEM",
            "GIT_AUTHOR_NAME",
            "GIT_COMMITTER_NAME",
            "GIT_AUTHOR_EMAIL",
            "GIT_COMMITTER_EMAIL",
            "GIT_AUTHOR_DATE",
            "GIT_COMMITTER_DATE",
            "GIT_ALLOW_PROTOCOL",
            "LC_ALL",
            "LANG",
            "TZ",
            "VIRTUALENV_NO_DOWNLOAD",
            "SNP_ALLOW_OLD_GIT",
            "GIT_CEILING_DIRECTORIES",
        ];
        for (k, _) in &vars {
            assert!(
                allowed.contains(&k.as_str()),
                "{k} leaked into the child env"
            );
        }
        // the root sits under the target dir (inside the snp checkout): discovery stops at its parent, so a
        // directory of the root that is not a repository is never taken for the snp repository
        assert_eq!(
            get("GIT_CEILING_DIRECTORIES").map(PathBuf::from).as_deref(),
            root.parent()
        );
        let plain = root.join("not-a-repo");
        std::fs::create_dir_all(&plain).unwrap();
        let exe = snp_proc::resolve::find_executable(OsStr::new("git"), env.env(), &plain).unwrap();
        let mut c = Command::new(exe);
        env.env().apply_to(&mut c);
        let out = c
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&plain)
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(
            std::fs::read_dir(root.join("git-template"))
                .unwrap()
                .count(),
            0
        );
        let derived = env.with_env("SKIP", "x");
        assert_eq!(derived.root(), env.root());
        assert!(derived.env().get("SKIP").is_some() && env.env().get("SKIP").is_none());
        // git sees the pinned config and nothing else
        let repo = env.git_repo("r").build();
        assert_eq!(
            git(&env, &repo, &["config", "init.defaultBranch"]),
            "master"
        );
        assert_eq!(git(&env, &repo, &["config", "core.autocrlf"]), "false");
        assert!(
            !repo.join(".git/hooks").exists(),
            "no sample hooks from the template"
        );
    }

    #[test]
    fn resource_fixtures_are_verbatim_repositories() {
        let env = TestEnv::new(no_bins());
        let repo = env.fixture_repo(Fixture::Resource("script_hooks_repo"));
        assert_eq!(git(&env, &repo, &["log", "-1", "--format=%s"]), "make_repo");
        let ls = git(&env, &repo, &["ls-files", "-s"]);
        assert!(ls.contains("100755") && ls.contains("bin/hook.sh"), "{ls}");
        assert_eq!(
            std::fs::read_to_string(repo.join(".pre-commit-hooks.yaml")).unwrap(),
            "-   id: bash_hook\n    name: Bash hook\n    entry: bin/hook.sh\n    language: script\n    files: ''\n"
        );
        // a second copy gets its own directory
        let again = env.fixture_repo(Fixture::Resource("script_hooks_repo"));
        assert_ne!(again, repo);
        for name in [
            "arbitrary_bytes_repo",
            "arg_per_line_hooks_repo",
            "exclude_types_repo",
            "failing_hook_repo",
            "logfile_repo",
            "modified_file_returns_zero_repo",
            "not_found_exe",
            "prints_cwd_repo",
            "python_hooks_repo",
            "stdout_stderr_repo",
            "types_or_repo",
            "types_repo",
        ] {
            let r = env.fixture_repo(Fixture::Resource(name));
            assert!(r.join(".pre-commit-hooks.yaml").is_file(), "{name}");
            assert_eq!(
                git(&env, &r, &["status", "--porcelain"]),
                "",
                "{name} is committed"
            );
        }
    }

    #[test]
    fn conftest_fixtures() {
        let env = TestEnv::new(no_bins());

        let consumer = env.fixture_repo(Fixture::ConsumingRepo("script_hooks_repo"));
        let cfg = std::fs::read_to_string(consumer.join(".pre-commit-config.yaml")).unwrap();
        let hooks = env.root().join("script_hooks_repo");
        let rev = git(&env, &hooks, &["rev-parse", "HEAD"]);
        assert_eq!(
            cfg,
            format!(
                "repos:\n-   repo: file://{}\n    rev: {rev}\n    hooks:\n    -   id: bash_hook\n",
                hooks.display()
            )
        );

        let conflict = env.fixture_repo(Fixture::MergeConflict);
        assert!(
            conflict.join(".git/MERGE_HEAD").is_file() && conflict.join(".git/MERGE_MSG").is_file()
        );
        assert!(git(&env, &conflict, &["ls-files", "--unmerged"]).contains("conflict_file"));

        let sub = env.fixture_repo(Fixture::ConflictingSubmodule);
        assert!(sub.ends_with("sub"));
        let git_dir = PathBuf::from(git(&env, &sub, &["rev-parse", "--absolute-git-dir"]));
        assert!(git_dir.join("MERGE_HEAD").is_file());

        let commit_msg = env.fixture_repo(Fixture::CommitMsgRepo);
        assert!(
            std::fs::read_to_string(commit_msg.join(".pre-commit-config.yaml"))
                .unwrap()
                .contains("- commit-msg\n")
        );
        let prepare = env.fixture_repo(Fixture::PrepareCommitMsgRepo { failing: false });
        assert!(git(&env, &prepare, &["ls-files", "-s"]).contains("100755"));
        let failing = env.fixture_repo(Fixture::PrepareCommitMsgRepo { failing: true });
        assert!(
            std::fs::read_to_string(failing.join(".pre-commit-config.yaml"))
                .unwrap()
                .contains("bash -c \"exit 1\"")
        );

        let push = env.fixture_repo(Fixture::PushExample);
        assert_eq!(
            git(&env, &push, &["rev-list", "--count", "origin/master..HEAD"]),
            "1"
        );

        let ood = env.fixture_repo(Fixture::OutOfDate { tagged: false });
        assert_eq!(git(&env, &ood, &["rev-list", "--count", "HEAD"]), "2");
        let tagged = env.fixture_repo(Fixture::OutOfDate { tagged: true });
        assert_eq!(git(&env, &tagged, &["describe", "--tags"]), "v1.2.3");

        let gone = env.fixture_repo(Fixture::HookDisappearing);
        assert!(
            std::fs::read_to_string(gone.join(".pre-commit-hooks.yaml"))
                .unwrap()
                .starts_with("-   id: bar\n")
        );
        assert_eq!(
            git(&env, &gone, &["log", "-1", "--format=%s"]),
            "modify_manifest"
        );

        let img = env.fixture_repo(Fixture::ImgStaged);
        assert_eq!(git(&env, &img, &["status", "--porcelain"]), "A  img.jpg");

        let sub_staged = env.fixture_repo(Fixture::SubStaged);
        let status = git(&env, &sub_staged, &["status", "--porcelain"]);
        assert!(
            status.contains("A  .gitmodules") && status.contains("A  sub"),
            "{status}"
        );
    }

    #[test]
    fn filters_normalise_volatile_output() {
        let env = TestEnv::new(no_bins());
        let filters = env.filters();
        assert!(filters.len() >= 12);
        let root = env.root().display().to_string();
        let sample = format!(
            "Check the log at {root}/store/pre-commit.log\n[INFO] Initializing environment for file://{root}/repos/x.\n\
             [WARNING] Stashing unstaged files to {root}/store/patch1791403647-1780651.\r\n- duration: 0.12s\n\
             [master (root-commit) 1a2b3c4] msg\npre-commit 4.6.2\n"
        );
        // apply them the way insta does (each regex in order) — through insta itself
        let mut settings = insta::Settings::clone_current();
        for (re, rep) in &filters {
            settings.add_filter(re, rep.clone());
        }
        settings.bind(|| {
            insta::assert_snapshot!(sample, @"
            Check the log at [STORE]/pre-commit.log
            [INFO] Initializing environment for file://[ROOT]/repos/x.
            [WARNING] Stashing unstaged files to [STORE]/patch[TIME]-[PID].
            - duration: [D]s
            [master [SHORT]] msg
            [VERSION]
            ");
        });
    }

    #[test]
    fn bins_locate_finds_the_workspace_binaries() {
        let bins = Bins::locate().expect("`cargo build --workspace --bins` first (CI does)");
        assert!(bins.snp.is_file() && bins.probe.is_file());
        assert_eq!(bins.snp.file_stem(), Some(OsStr::new("snp")));
        assert_eq!(bins.probe.file_stem(), Some(OsStr::new("snp-probe")));
        let env = TestEnv::new(bins.clone());
        assert_eq!(env.probe(), bins.probe);
        // both brands through the runners (argv[0] decides, COMPATIBILITY §3.1)
        let out = env.snp(env.root()).arg("--version").output();
        assert_eq!((out.code, out.stdout.as_slice()), (0, &b"snp 2.0.0\n"[..]));
        let out = env.snp_as_pre_commit(env.root()).arg("--version").output();
        assert_eq!(
            (out.code, out.stdout.as_slice()),
            (0, &b"pre-commit 4.6.2\n"[..])
        );
        snp_testkit::snp_snapshot!(env, out, @"
        exit_code: 0
        ----- stdout -----
        [VERSION]

        ----- stderr -----
        ");
    }

    #[test]
    fn oracles_run_from_the_target_dir_never_path() {
        let env = TestEnv::new(no_bins());
        for which in [Oracle::Pc462Py312, Oracle::Pc462Py313] {
            let Some(cmd) = env.oracle(which, env.root()) else {
                eprintln!("oracle {which:?} not installed (cargo xtask oracle install): skipped");
                continue;
            };
            let out = cmd.arg("--version").output();
            assert_eq!(
                (out.code, out.stdout.as_slice()),
                (0, &b"pre-commit 4.6.2\n"[..]),
                "{which:?}"
            );
        }
    }
}
