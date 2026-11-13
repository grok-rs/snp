//! WP-18's catalog rows (`tests/catalog/index.tsv`): pre-commit's `tests/git_test.py` (T10-GIT-01…14), the clone
//! rows of `tests/store_test.py` (T10-STO-05, 07, 08, 13) and the 13 X.4 rows X13-GIT-07, X13-GIT-10, X13-HI-03,
//! X13-HI-04 and X13-SM-05 — against real git in a hermetic sandbox. Behaviour tests beyond the catalog live in
//! `behaviour/`.
//!
//! Owner: WP-18.

#[cfg(test)]
mod behaviour;
#[cfg(test)]
mod common;

/// The catalog rows, named by their IDs.
#[cfg(test)]
mod rows {
    use std::ffi::{OsStr, OsString};
    use std::sync::Arc;

    use crate::common::{Sandbox, exc, sorted, strs, zl};
    use snp_core::env::Env;
    use snp_git::{CloneMode, MergeState, OwnedRepo, no_git_env};

    const FATAL_GIT_FAILED: &str =
        "git failed. Is it installed, and are you in a Git repository directory?";
    const FATAL_TOPLEVEL: &str = "git toplevel unexpectedly empty! make sure you are not inside the `.git` directory of your repository.";

    #[test]
    fn t10_git_01_get_root_at_root_deeper_and_failures() {
        let sb = Sandbox::new();
        let repo = sb.init("repo");
        let (git, info) = sb.discover(&repo);
        assert_eq!(git.root(), repo);
        assert_eq!(info.root, repo);
        let deeper = repo.join("foo");
        std::fs::create_dir_all(&deeper).unwrap();
        assert_eq!(sb.discover(&deeper).1.root, repo);

        let exe = sb.exe();
        let fail = |cwd: &std::path::Path| {
            snp_git::Git::discover(Arc::clone(&exe), &sb.env, cwd, sb.backend()).unwrap_err()
        };
        let objects = repo.join(".git/objects");
        let e = fail(&objects);
        assert_eq!(exc(&e), ("FatalError".into(), FATAL_TOPLEVEL.into()));
        assert_eq!(e.exit_code(), 1);
        let e = fail(&repo.join(".git"));
        assert_eq!(exc(&e), ("FatalError".into(), FATAL_TOPLEVEL.into()));
        let e = fail(&sb.root); // the parent of the repository is not a repository
        assert_eq!(exc(&e), ("FatalError".into(), FATAL_GIT_FAILED.into()));
    }

    #[test]
    fn t10_git_02_bare_worktree_git_dir_and_worktree_in_git() {
        let sb = Sandbox::new();
        let src = sb.init("src");
        sb.commit(&src, "commit!");

        // test_get_root_bare_worktree
        let bare = sb.path("bare.git");
        sb.git(
            &sb.root,
            &[
                OsStr::new("clone"),
                OsStr::new("-q"),
                OsStr::new("--bare"),
                src.as_os_str(),
                bare.as_os_str(),
            ],
        );
        sb.git(&bare, &["worktree", "add", "-q", "foo", "HEAD"]);
        let (_, info) = sb.discover(&bare.join("foo"));
        assert_eq!(info.root, bare.join("foo"));

        // test_get_git_dir (#1972)
        let wt = sb.path("worktree");
        sb.git(&src, &["worktree", "add", "-q", "../worktree"]);
        let (_, info) = sb.discover(&wt);
        assert_eq!(info.root, wt);
        assert_eq!(info.git_dir, src.join(".git/worktrees/worktree"));
        assert_eq!(info.common_dir, src.join(".git"));
        assert!(info.is_worktree);
        assert_eq!(info.index_file, src.join(".git/worktrees/worktree/index"));

        // test_get_root_worktree_in_git
        sb.git(&src, &["worktree", "add", "-q", ".git/trees/foo", "HEAD"]);
        let inner = src.join(".git/trees/foo");
        assert_eq!(sb.discover(&inner).1.root, inner);
        let (_, info) = sb.discover(&src);
        assert!(!info.is_worktree);
    }

    #[test]
    fn t10_git_03_staged_files_exclude_deletions() {
        let sb = Sandbox::new();
        let repo = sb.init("repo");
        sb.write(&repo, "test", b"");
        sb.git(&repo, &["add", "test"]);
        sb.commit(&repo, "commit!");
        sb.git(&repo, &["rm", "-q", "--cached", "test"]);
        let (git, _) = sb.discover(&repo);
        assert!(git.staged_files().unwrap().is_empty());
    }

    #[test]
    fn t10_git_04_merge_state_merge_submodule_cherry_pick() {
        let sb = Sandbox::new();
        let repo = sb.init("plain");
        let (git, info) = sb.discover(&repo);
        assert_eq!(git.merge_state(&info), MergeState::None);

        let conflict = sb.merge_conflict("conflict");
        let (git, info) = sb.discover(&conflict);
        assert_eq!(git.merge_state(&info), MergeState::Merging);

        let sub = sb.conflicting_submodule("sm");
        let (git, info) = sb.discover(&sub);
        assert_eq!(git.merge_state(&info), MergeState::Merging);

        // test_cherry_pick_conflict
        sb.git(&conflict, &["merge", "--abort"]);
        let foo = sb.git_text(&conflict, &["rev-parse", "foo"]);
        let (code, _, _) = sb.git_status(&conflict, &["cherry-pick", foo.as_str()]);
        assert_ne!(code, 0);
        assert_eq!(git.merge_state(&sb.discover(&conflict).1), MergeState::None);
    }

    #[test]
    fn t10_git_05_conflicted_files() {
        let sb = Sandbox::new();
        // test_get_conflicted_files
        let repo = sb.merge_conflict("c1");
        sb.resolve_conflict(&repo);
        sb.write(&repo, "other_file", b"oh hai");
        sb.git(&repo, &["add", "other_file"]);
        let (git, info) = sb.discover(&repo);
        assert_eq!(
            sorted(zl(&git.conflicted_files(&info).unwrap())),
            strs(&["conflict_file", "other_file"])
        );

        // test_get_conflicted_files_in_submodule
        let sub = sb.conflicting_submodule("sm");
        sb.resolve_conflict(&sub);
        let (git, info) = sb.discover(&sub);
        assert_eq!(
            zl(&git.conflicted_files(&info).unwrap()),
            strs(&["conflict_file"])
        );

        // test_get_conflicted_files_unstaged_files
        let repo = sb.merge_conflict("c2");
        sb.resolve_conflict(&repo);
        sb.write(&repo, "bar_only_file", b"new contents!\n");
        let (git, info) = sb.discover(&repo);
        assert_eq!(
            zl(&git.conflicted_files(&info).unwrap()),
            strs(&["conflict_file"])
        );

        // test_get_conflicted_files_with_file_named_head (needs the `--` separator)
        let repo = sb.merge_conflict("c3");
        sb.resolve_conflict(&repo);
        sb.write(&repo, "HEAD", b"");
        sb.git(&repo, &["add", "HEAD"]);
        let (git, info) = sb.discover(&repo);
        assert_eq!(
            sorted(zl(&git.conflicted_files(&info).unwrap())),
            strs(&["HEAD", "conflict_file"])
        );
    }

    #[test]
    fn t10_git_06_merge_msg_tab_lines() {
        const MERGE_MSG: &[u8] = b"Merge branch 'foo' into bar\n\nConflicts:\n\tconflict_file\n";
        let sb = Sandbox::new();
        for (msg, expected) in [
            (MERGE_MSG.to_vec(), vec!["conflict_file"]),
            (
                [MERGE_MSG, b"\tother_conflict_file\n"].concat(),
                vec!["conflict_file", "other_conflict_file"],
            ),
        ] {
            let repo = sb.merge_conflict(&format!("m{}", expected.len()));
            sb.resolve_conflict(&repo);
            std::fs::write(repo.join(".git/MERGE_MSG"), &msg).unwrap();
            let (git, info) = sb.discover(&repo);
            // the MERGE_MSG lines come first, in file order; the merge diff adds nothing new here
            assert_eq!(zl(&git.conflicted_files(&info).unwrap()), strs(&expected));
        }
    }

    #[test]
    fn t10_git_07_changed_files_three_dot_then_two_dot() {
        let sb = Sandbox::new();
        let repo = sb.init("repo");
        sb.commit(&repo, "commit!");
        sb.write(&repo, "a.txt", b"");
        sb.write(&repo, "b.txt", b"");
        sb.git(&repo, &["add", "."]);
        sb.commit(&repo, "commit!");
        let (git, _) = sb.discover(&repo);
        let changed =
            |a: &str, b: &str| zl(&git.changed_files(OsStr::new(a), OsStr::new(b)).unwrap());
        assert_eq!(changed("HEAD^", "HEAD"), strs(&["a.txt", "b.txt"]));
        assert_eq!(changed("HEAD", "HEAD^"), Vec::<Vec<u8>>::new());

        // test_get_changed_files_disparate_histories
        let repo = sb.init("disparate");
        sb.commit(&repo, "commit!");
        sb.write(&repo, "a.txt", b"");
        sb.git(&repo, &["add", "."]);
        sb.commit(&repo, "commit!");
        sb.git(&repo, &["branch", "-m", "branch1"]);
        sb.git(&repo, &["checkout", "-q", "--orphan", "branch2"]);
        sb.git(&repo, &["rm", "-q", "-rf", "."]);
        sb.write(&repo, "a.txt", b"");
        sb.write(&repo, "b.txt", b"");
        sb.git(&repo, &["add", "."]);
        sb.commit(&repo, "commit!");
        let (git, _) = sb.discover(&repo);
        assert_eq!(
            zl(&git
                .changed_files(OsStr::new("branch1"), OsStr::new("branch2"))
                .unwrap()),
            strs(&["b.txt"])
        );
        // both forms failing reports the two-dot command (X13-GIT-04)
        let e = git
            .changed_files(OsStr::new("bogus"), OsStr::new("HEAD"))
            .unwrap_err();
        let (ty, text) = exc(&e);
        assert_eq!(ty, "CalledProcessError");
        assert!(
            text.contains("'diff', '--name-only', '--no-ext-diff', '-z', 'bogus..HEAD')"),
            "{text}"
        );
        assert_eq!(e.exit_code(), 3);
    }

    #[test]
    fn t10_git_08_zsplit() {
        let sb = Sandbox::new();
        let repo = sb.init("repo");
        for (out, expected) in [
            (&b"foo\0bar\0"[..], vec!["foo", "bar"]),
            (b"foo\0", vec!["foo"]),
            (b"", vec![]),
            (b"foo", vec!["foo"]),
        ] {
            let backend = Arc::new(crate::common::Faulty::new().on(
                &["ls-files", "-z", "--deduplicate"],
                0,
                out,
                b"",
            ));
            let (git, _) = snp_git::Git::discover(sb.exe(), &sb.env, &repo, backend).unwrap();
            assert_eq!(zl(&git.all_files().unwrap()), strs(&expected));
        }
    }

    #[test]
    fn t10_git_09_non_ascii_names_are_never_quoted() {
        let sb = Sandbox::new();
        let repo = sb.init("repo");
        sb.commit(&repo, "commit!");
        sb.write(&repo, "интервью", b"");
        sb.git(&repo, &["add", "."]);
        sb.commit(&repo, "commit!");
        let (git, _) = sb.discover(&repo);
        let name = strs(&["интервью"]);
        assert_eq!(zl(&git.all_files().unwrap()), name);
        assert_eq!(
            zl(&git
                .changed_files(OsStr::new("HEAD^"), OsStr::new("HEAD"))
                .unwrap()),
            name
        );
        sb.write(&repo, "интервью", b"hi");
        sb.git(&repo, &["add", "."]);
        assert_eq!(zl(&git.staged_files().unwrap()), name);

        // test_get_conflicted_files_non_ascii
        let repo = sb.merge_conflict("conflict");
        sb.write(&repo, "интервью", b"");
        sb.git(&repo, &["add", "."]);
        let (git, info) = sb.discover(&repo);
        assert_eq!(
            sorted(zl(&git.conflicted_files(&info).unwrap())),
            sorted(strs(&["conflict_file", "интервью"]))
        );
    }

    #[test]
    fn t10_git_10_all_files_deduplicated_in_merge_conflict() {
        let sb = Sandbox::new();
        let repo = sb.merge_conflict("conflict");
        let (git, _) = sb.discover(&repo);
        assert_eq!(
            sorted(zl(&git.all_files().unwrap())),
            strs(&[
                ".pre-commit-config.yaml",
                "bar_only_file",
                "conflict_file",
                "foo_only_file",
                "placeholder"
            ])
        );
    }

    #[test]
    fn t10_git_11_intent_to_add() {
        let sb = Sandbox::new();
        let repo = sb.init("repo");
        sb.write(&repo, "a", b"");
        sb.git(&repo, &["add", "--intent-to-add", "a"]);
        let (git, _) = sb.discover(&repo);
        assert_eq!(zl(&git.intent_to_add().unwrap()), strs(&["a"]));

        // test_status_output_with_rename
        let repo = sb.init("rename");
        sb.write(&repo, "a", b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        sb.git(&repo, &["add", "a"]);
        sb.commit(&repo, "commit!");
        sb.git(&repo, &["mv", "a", "b"]);
        sb.write(&repo, "c", b"");
        sb.git(&repo, &["add", "--intent-to-add", "c"]);
        let (git, _) = sb.discover(&repo);
        assert_eq!(zl(&git.intent_to_add().unwrap()), strs(&["c"]));
    }

    #[test]
    fn t10_git_12_no_git_env() {
        let env = Env::from_pairs([
            ("http_proxy", "http://myproxy:80"),
            ("GIT_EXEC_PATH", "/some/git/exec/path"),
            ("GIT_SSH", "/usr/bin/ssh"),
            ("GIT_SSH_COMMAND", "ssh -o"),
            ("GIT_DIR", "/none/shall/pass"),
            ("GIT_CONFIG_KEY_0", "user.name"),
            ("GIT_CONFIG_VALUE_0", "anthony"),
            ("GIT_CONFIG_KEY_1", "user.email"),
            ("GIT_CONFIG_VALUE_1", "asottile@example.com"),
            ("GIT_CONFIG_COUNT", "2"),
        ]);
        let got: Vec<(String, String)> = no_git_env(&env)
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        let want: Vec<(String, String)> = [
            ("http_proxy", "http://myproxy:80"),
            ("GIT_EXEC_PATH", "/some/git/exec/path"),
            ("GIT_SSH", "/usr/bin/ssh"),
            ("GIT_SSH_COMMAND", "ssh -o"),
            ("GIT_CONFIG_KEY_0", "user.name"),
            ("GIT_CONFIG_VALUE_0", "anthony"),
            ("GIT_CONFIG_KEY_1", "user.email"),
            ("GIT_CONFIG_VALUE_1", "asottile@example.com"),
            ("GIT_CONFIG_COUNT", "2"),
        ]
        .iter()
        .map(|(k, v)| {
            let k = if cfg!(windows) {
                k.to_uppercase()
            } else {
                (*k).to_owned()
            };
            (k, (*v).to_owned())
        })
        .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn t10_git_13_init_repo_uses_the_empty_template() {
        let sb = Sandbox::new();
        let dest = sb.path("dest");
        std::fs::create_dir_all(&dest).unwrap();
        // `git.init_repo(tmp, remote='dne')`: the clone then fails at the fetch, after init + remote add
        let e = OwnedRepo::clone_rev(
            sb.exe(),
            &sb.env,
            &dest,
            "dne",
            "HEAD",
            CloneMode::Foreground,
            sb.backend(),
        )
        .unwrap_err();
        assert_eq!(exc(&e).0, "CalledProcessError");
        assert!(dest.join(".git").is_dir());
        assert!(!dest.join(".git/hooks").exists());
        assert_eq!(sb.git_text(&dest, &["remote", "get-url", "origin"]), "dne");
    }

    #[test]
    fn t10_git_14_merge_state_and_the_conflicted_files_union() {
        let sb = Sandbox::new();
        let repo = sb.merge_conflict("conflict");
        let (git, info) = sb.discover(&repo);
        // in-conflict ⇔ both MERGE_MSG and MERGE_HEAD exist in the (worktree) git dir
        assert_eq!(git.merge_state(&info), MergeState::Merging);
        let msg = std::fs::read(info.git_dir.join("MERGE_MSG")).unwrap();
        assert!(
            String::from_utf8_lossy(&msg).contains("#\tconflict_file\n"),
            "git writes the `#\\t` form"
        );
        sb.resolve_conflict(&repo);
        sb.write(&repo, "other_file", b"oh hai");
        sb.git(&repo, &["add", "other_file"]);
        // the `#\t` lines of MERGE_MSG first (file order), then the merge diff, without duplicates
        std::fs::write(
            info.git_dir.join("MERGE_MSG"),
            b"Merge branch 'foo' into bar\n\n# Conflicts:\n#\tz_only_in_msg\n#\tconflict_file\n",
        )
        .unwrap();
        assert_eq!(
            zl(&git.conflicted_files(&info).unwrap()),
            strs(&["z_only_in_msg", "conflict_file", "other_file"])
        );
        std::fs::remove_file(info.git_dir.join("MERGE_HEAD")).unwrap();
        assert_eq!(git.merge_state(&info), MergeState::None);
    }

    #[test]
    fn t10_sto_05_clone_failure_names_the_repo_and_leaves_nothing_else() {
        let sb = Sandbox::new();
        let store = sb.path("store");
        let dest = store.join("repo1234");
        std::fs::create_dir_all(&dest).unwrap();
        let e = OwnedRepo::clone_rev(
            sb.exe(),
            &sb.env,
            &dest,
            "/i_dont_exist_lol",
            "fake_rev",
            CloneMode::Foreground,
            sb.backend(),
        )
        .unwrap_err();
        let (ty, text) = exc(&e);
        assert_eq!(ty, "CalledProcessError");
        assert_eq!(e.exit_code(), 3);
        assert!(text.contains("/i_dont_exist_lol"), "{text}");
        // the error is the fallback's step (X13-GIT-01): `fetch origin --tags`
        assert!(text.contains("'fetch', 'origin', '--tags')"), "{text}");
        let entries: Vec<OsString> = std::fs::read_dir(&store)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![OsString::from("repo1234")]);
    }

    #[test]
    fn t10_sto_07_shallow_failure_falls_back_to_a_complete_clone() {
        let sb = Sandbox::new();
        let path = sb.init("upstream");
        sb.commit(&path, "commit!");
        let rev = sb.head(&path);
        sb.commit(&path, "commit!");
        // force the shallow fetch to fail
        let backend = Arc::new(crate::common::Faulty::new().on(
            &["-c", "protocol.version=2", "fetch", "origin"],
            1,
            b"",
            b"",
        ));
        let dest = sb.path("clone");
        std::fs::create_dir_all(&dest).unwrap();
        OwnedRepo::clone_rev(
            sb.exe(),
            &sb.env,
            &dest,
            path.to_str().unwrap(),
            &rev,
            CloneMode::Foreground,
            backend,
        )
        .unwrap();
        // checked out at the rev, by the complete clone: full history, remote branches, not shallow
        assert_eq!(sb.head(&dest), rev);
        assert!(!dest.join(".git/shallow").exists());
        assert_eq!(
            sb.git_text(&dest, &["rev-parse", "origin/master"]),
            sb.head(&path)
        );
    }

    #[test]
    fn t10_sto_08_tag_only_reachable_from_a_deleted_branch() {
        let sb = Sandbox::new();
        let path = sb.init("upstream");
        sb.commit(&path, "commit!");
        sb.git(&path, &["checkout", "-q", "master", "-b", "branch"]);
        sb.commit(&path, "commit!");
        sb.git(&path, &["tag", "v1"]);
        let tagged = sb.head(&path);
        sb.git(&path, &["checkout", "-q", "master"]);
        sb.git(&path, &["branch", "-q", "-D", "branch"]);
        let dest = sb.path("clone");
        std::fs::create_dir_all(&dest).unwrap();
        OwnedRepo::clone_rev(
            sb.exe(),
            &sb.env,
            &dest,
            path.to_str().unwrap(),
            "v1",
            CloneMode::Foreground,
            sb.backend(),
        )
        .unwrap();
        assert_eq!(sb.head(&dest), tagged);
    }

    #[test]
    fn t10_sto_13_nested_submodules_are_checked_out() {
        let sb = Sandbox::new();
        let sub = sb.init("sub");
        sb.write(&sub, "submodule", b"i am a submodule");
        sb.git(&sub, &["add", "."]);
        sb.commit(&sub, "init");
        let repo = sb.init("repo");
        sb.write(&repo, "repository", b"i am a repo");
        sb.git(&repo, &["add", "."]);
        sb.git(
            &repo,
            &[
                OsStr::new("submodule"),
                OsStr::new("add"),
                OsStr::new("-q"),
                sub.as_os_str(),
                OsStr::new("sub"),
            ],
        );
        sb.commit(&repo, "init");
        let rev = sb.head(&repo);
        let outer = sb.init("outer");
        sb.git(
            &outer,
            &[
                OsStr::new("submodule"),
                OsStr::new("add"),
                OsStr::new("-q"),
                repo.as_os_str(),
                OsStr::new("nested"),
            ],
        );
        sb.commit(&outer, "init");
        let outer_rev = sb.head(&outer);

        let dest = sb.path("clone");
        std::fs::create_dir_all(&dest).unwrap();
        OwnedRepo::clone_rev(
            sb.exe(),
            &sb.env,
            &dest,
            repo.to_str().unwrap(),
            &rev,
            CloneMode::Foreground,
            sb.backend(),
        )
        .unwrap();
        assert!(dest.join("repository").is_file());
        assert_eq!(
            std::fs::read(dest.join("sub/submodule")).unwrap(),
            b"i am a submodule"
        );

        let dest = sb.path("clone-nested");
        std::fs::create_dir_all(&dest).unwrap();
        OwnedRepo::clone_rev(
            sb.exe(),
            &sb.env,
            &dest,
            outer.to_str().unwrap(),
            &outer_rev,
            CloneMode::Foreground,
            sb.backend(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(dest.join("nested/sub/submodule")).unwrap(),
            b"i am a submodule"
        );
    }

    #[test]
    fn x13_git_07_merge_msg_conflict_line_with_non_utf8_bytes() {
        let sb = Sandbox::new();
        let repo = sb.init("repo");
        sb.write(&repo, "a", b"a");
        sb.git(&repo, &["add", "."]);
        sb.commit(&repo, "commit!");
        let head = sb.head(&repo);
        std::fs::write(repo.join(".git/MERGE_HEAD"), format!("{head}\n")).unwrap();
        std::fs::write(
            repo.join(".git/MERGE_MSG"),
            b"Merge branch 'foo'\n\n# Conflicts:\n#\tcaf\xe9.txt\n#\tok.txt\n",
        )
        .unwrap();
        let (git, info) = sb.discover(&repo);
        assert_eq!(git.merge_state(&info), MergeState::Merging);
        // pre-commit: UnicodeDecodeError (exit 3); snp keeps the raw bytes (D-CRASH-12)
        assert_eq!(
            zl(&git.conflicted_files(&info).unwrap()),
            vec![b"caf\xe9.txt".to_vec(), b"ok.txt".to_vec()]
        );
    }

    #[test]
    fn x13_git_10_rev_parse_echoing_git_dir_is_a_fatal_error() {
        use snp_git::{GitCaps, GitExe, GitVersion};
        let sb = Sandbox::new();
        let repo = sb.init("repo");
        // git < 2.5 echoes the options it does not know
        let scripted = Arc::new(
            crate::common::Faulty::new()
                .on(&["rev-parse", "--show-cdup"], 0, b"\n", b"")
                .on(&["rev-parse", "--is-inside-git-dir"], 0, b"false\n", b"")
                .on(&["rev-parse", "--git-dir"], 0, b"--git-dir\n", b"")
                .on(
                    &["rev-parse", "--git-common-dir"],
                    0,
                    b"--git-common-dir\n",
                    b"",
                )
                .on(
                    &["rev-parse", "--git-path", "index"],
                    0,
                    b"--git-path\nindex\n",
                    b"",
                ),
        );
        let real = sb.exe();
        let old = Arc::new(GitExe {
            argv0: real.argv0.clone(),
            caps: GitCaps {
                version: Some(GitVersion {
                    major: 2,
                    minor: 4,
                    patch: 0,
                }),
                ls_files_dedup: false,
                path_format: false,
                config_env: false,
                init_object_format: false,
                config_hooks: false,
                pathspec_from_file: false,
            },
        });
        let e = snp_git::Git::discover(old, &sb.env, &repo, scripted).unwrap_err();
        let (ty, text) = exc(&e);
        assert_eq!(ty, "FatalError");
        assert_eq!(e.exit_code(), 1);
        assert!(
            text.starts_with(
                "git 2.4.0 is not supported: `git rev-parse --git-dir` needs git 2.5.0 or newer"
            ),
            "{text}"
        );
    }

    /// A repository with one root commit, for the pre-push rows.
    fn push_repo(sb: &Sandbox) -> (std::path::PathBuf, String) {
        let repo = sb.init("push");
        sb.write(&repo, "f", b"f");
        sb.git(&repo, &["add", "."]);
        sb.commit(&repo, "commit!");
        let head = sb.head(&repo);
        (repo, head)
    }

    #[test]
    fn x13_hi_03_malformed_pre_push_lines_fail_closed() {
        let sb = Sandbox::new();
        let (repo, head) = push_repo(&sb);
        let (git, _) = sb.discover(&repo);
        let z40 = crate::common::rep('0', 40);
        for (stdin, got) in [
            ("\n".to_owned(), 0),
            (format!("refs/heads/main {head} refs/heads/main\n"), 3),
            // an empty line after a skipped deletion line
            (format!("(delete) {z40} refs/heads/gone {head}\n\n"), 0),
        ] {
            let e = git
                .pre_push(stdin.as_bytes(), OsStr::new("origin"))
                .unwrap_err();
            assert_eq!(
                exc(&e),
                (
                    "ValueError".to_owned(),
                    format!("not enough values to unpack (expected 4, got {got})")
                )
            );
            assert_eq!(e.exit_code(), 3);
        }
    }

    #[test]
    fn x13_hi_04_non_utf8_pre_push_stdin_runs_on_bytes() {
        let sb = Sandbox::new();
        let (repo, head) = push_repo(&sb);
        let (git, _) = sb.discover(&repo);
        let z40 = crate::common::rep('0', 40);
        let stdin = [
            b"refs/heads/caf\xe9 ".as_slice(),
            head.as_bytes(),
            b" refs/heads/x ",
            z40.as_bytes(),
            b"\n",
        ]
        .concat();
        let ns = git.pre_push(&stdin, OsStr::new("origin")).unwrap().unwrap();
        // pushing a whole new history: the first new commit is a root
        assert!(ns.all_files);
        assert_eq!((ns.from_ref, ns.to_ref), (None, None));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            assert_eq!(ns.local_branch.as_bytes(), b"refs/heads/caf\xe9");
        }
        assert_eq!(ns.remote_branch, OsString::from("refs/heads/x"));
    }

    #[test]
    fn x13_sm_05_remote_sha_not_present_locally_proceeds() {
        let sb = Sandbox::new();
        let (repo, root) = push_repo(&sb);
        sb.write(&repo, "g", b"g");
        sb.git(&repo, &["add", "."]);
        sb.commit(&repo, "commit!");
        let head = sb.head(&repo);
        // a remote branch that knows the root commit only
        sb.git(
            &repo,
            &["update-ref", "refs/remotes/origin/master", root.as_str()],
        );
        let (git, _) = sb.discover(&repo);
        let unknown = crate::common::rep('1', 40);
        let stdin = format!("refs/heads/master {head} refs/heads/master {unknown}\n");
        // `git cat-file -e <unknown>^{commit}` fails (its `fatal: Not a valid object name` goes to stderr): the
        // rev-list path decides
        let ns = git
            .pre_push(stdin.as_bytes(), OsStr::new("origin"))
            .unwrap()
            .unwrap();
        assert!(!ns.all_files);
        assert_eq!(ns.from_ref, Some(OsString::from(root)));
        assert_eq!(ns.to_ref, Some(OsString::from(&head)));
        assert_eq!(ns.local_branch, OsString::from("refs/heads/master"));
    }
}
