//! WP-23's catalog rows (`tests/catalog/index.tsv`): pre-commit's `tests/staged_files_only_test.py` (T10-SFO-01…17)
//! and the 13 X.4 rows X13-ST-04, X13-GIT-09 and X13-STASH-01…03 — against real git in a hermetic sandbox. Behaviour
//! tests beyond the catalog (lock, journal, recovery, `stage_fixed`, hazards, `DiffOracle`) live in `behaviour/`.
//!
//! Owner: WP-23.

#[cfg(test)]
mod behaviour;
#[cfg(test)]
mod common;

/// The catalog rows, named by their IDs.
#[cfg(test)]
mod rows {
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::Arc;

    use snp_core::brand::Brand;
    use snp_stash::{StashGuard, Stashed};

    use crate::common::{Broken, Buf, FOO, Reply, Sandbox, Spy, exc, printer};

    const IMG1: &[u8] = include_bytes!("../../snp-testkit/resources/img1.jpg");
    const IMG2: &[u8] = include_bytes!("../../snp-testkit/resources/img2.jpg");
    const IMG3: &[u8] = include_bytes!("../../snp-testkit/resources/img3.jpg");

    /// `with staged_files_only(patch_dir): during()` — returns the guard's stdout.
    fn with_stash(sb: &Sandbox, repo: &Path, during: impl FnOnce(&StashGuard)) -> String {
        let (cx, out, _err) = sb.ctx(repo);
        let guard = snp_stash::enter(cx).unwrap();
        during(&guard);
        guard.restore().unwrap();
        out.text()
    }

    /// The resolved git (`normalize_cmd(('git',))`), as `CalledProcessError` texts show it.
    fn git_exe(sb: &Sandbox, repo: &Path) -> String {
        snp_proc::resolve::normalize_cmd(&[OsString::from("git")], &sb.env, repo).unwrap()[0]
            .to_string_lossy()
            .into_owned()
    }

    fn foo_state(sb: &Sandbox, repo: &Path, contents: &[u8], status: &str) {
        assert_eq!(sb.read(repo, "foo"), contents);
        assert_eq!(sb.status_of(repo, "foo"), status);
    }

    #[test]
    fn t10_sfo_01_nothing_unstaged() {
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        let out = with_stash(&sb, &repo, |g| {
            assert_eq!(g.stashed(), &Stashed::Nothing);
            assert!(g.diff_index_clean());
            foo_state(&sb, &repo, FOO.as_bytes(), "A");
        });
        foo_state(&sb, &repo, FOO.as_bytes(), "A");
        assert_eq!(out, "");
        // Clean runs never touch the store.
        assert!(!sb.patches().exists());
    }

    #[test]
    fn t10_sfo_02_something_unstaged() {
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        sb.write(&repo, "foo", b"herp\nderp\n");
        foo_state(&sb, &repo, b"herp\nderp\n", "AM");
        let mut patch = None;
        let out = with_stash(&sb, &repo, |g| {
            foo_state(&sb, &repo, FOO.as_bytes(), "A");
            assert!(!g.diff_index_clean());
            let Stashed::Patch(p) = g.stashed() else {
                panic!("no patch")
            };
            patch = Some(p.clone());
        });
        foo_state(&sb, &repo, b"herp\nderp\n", "AM");
        let p = patch.unwrap();
        assert!(p.starts_with(sb.patches()));
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("patch") && name.ends_with(&format!("-{}", std::process::id())));
        assert_eq!(
            out,
            format!(
                "[WARNING] Unstaged files detected.\n[INFO] Stashing unstaged files to {0}.\n[INFO] Restored changes \
                 from {0}.\n",
                p.display()
            )
        );
        // The patch is kept (parity); journal and pointer are gone (Done).
        assert!(p.is_file());
        assert_eq!(sb.patch_files(), vec![name]);
        assert!(!repo.join(".git/snp-stash.journal").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn t10_sfo_03_patch_dir_missing() {
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        sb.write(&repo, "foo", b"hello\nworld\n");
        assert!(!sb.patches().exists());
        with_stash(&sb, &repo, |_| {});
        assert!(sb.patches().is_dir());
        foo_state(&sb, &repo, b"hello\nworld\n", "AM");
    }

    #[test]
    fn t10_sfo_04_ext_diff_and_color_always() {
        for cfg in [["diff.external", "/bin/echo"], ["color.diff", "always"]] {
            let sb = Sandbox::new();
            let repo = sb.foo_staged("r");
            sb.git(&repo, &["config", "--local", cfg[0], cfg[1]]);
            sb.write(&repo, "foo", b"herp\nderp\n");
            with_stash(&sb, &repo, |_| foo_state(&sb, &repo, FOO.as_bytes(), "A"));
            foo_state(&sb, &repo, b"herp\nderp\n", "AM");
        }
    }

    #[test]
    fn t10_sfo_05_both_modify_non_conflicting() {
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        sb.write(&repo, "foo", format!("{FOO}9\n").as_bytes());
        with_stash(&sb, &repo, |_| {
            foo_state(&sb, &repo, FOO.as_bytes(), "A");
            sb.write(&repo, "foo", FOO.replace('1', "a").as_bytes());
        });
        foo_state(
            &sb,
            &repo,
            format!("{}9\n", FOO.replace('1', "a")).as_bytes(),
            "AM",
        );
    }

    #[test]
    fn t10_sfo_06_both_modify_conflicting() {
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        sb.write(&repo, "foo", FOO.replace('1', "a").as_bytes());
        let out = with_stash(&sb, &repo, |_| {
            foo_state(&sb, &repo, FOO.as_bytes(), "A");
            sb.write(&repo, "foo", FOO.replace('1', "b").as_bytes());
            foo_state(&sb, &repo, FOO.replace('1', "b").as_bytes(), "AM");
        });
        foo_state(&sb, &repo, FOO.replace('1', "a").as_bytes(), "AM");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines[2],
            "[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes..."
        );
        assert!(lines[3].starts_with("[INFO] Restored changes from "));
    }

    #[test]
    fn t10_sfo_07_binary_images() {
        // nothing unstaged
        let sb = Sandbox::new();
        let repo = sb.init("r");
        sb.write(&repo, "img.jpg", IMG1);
        sb.git(&repo, &["add", "img.jpg"]);
        with_stash(&sb, &repo, |_| assert_eq!(sb.read(&repo, "img.jpg"), IMG1));
        assert_eq!(
            (sb.read(&repo, "img.jpg"), sb.status_of(&repo, "img.jpg")),
            (IMG1.to_vec(), "A".to_owned())
        );
        // something unstaged
        sb.write(&repo, "img.jpg", IMG2);
        with_stash(&sb, &repo, |_| {
            assert_eq!(
                (sb.read(&repo, "img.jpg"), sb.status_of(&repo, "img.jpg")),
                (IMG1.to_vec(), "A".to_owned())
            );
        });
        assert_eq!(
            (sb.read(&repo, "img.jpg"), sb.status_of(&repo, "img.jpg")),
            (IMG2.to_vec(), "AM".to_owned())
        );
        // conflict during the hook
        with_stash(&sb, &repo, |_| {
            sb.write(&repo, "img.jpg", IMG3);
            assert_eq!(sb.status_of(&repo, "img.jpg"), "AM");
        });
        assert_eq!(
            (sb.read(&repo, "img.jpg"), sb.status_of(&repo, "img.jpg")),
            (IMG2.to_vec(), "AM".to_owned())
        );
    }

    /// `sub_staged`: `sub` (rev1 of a two-commit repo) added as a submodule of a repo with `bar`.
    fn sub_staged(sb: &Sandbox) -> (std::path::PathBuf, String, String) {
        let up = sb.init("up");
        sb.write(&up, "foo", b"");
        sb.git(&up, &["add", "foo"]);
        sb.commit(&up, "c1");
        let rev1 = sb.git_text(&up, &["rev-parse", "HEAD"]);
        sb.commit(&up, "c2");
        let rev2 = sb.git_text(&up, &["rev-parse", "HEAD"]);
        let repo = sb.init("r");
        sb.write(&repo, "bar", b"");
        sb.git(&repo, &["add", "bar"]);
        sb.commit(&repo, "bar");
        sb.git(
            &repo,
            &[
                std::ffi::OsStr::new("submodule"),
                "add".as_ref(),
                "-q".as_ref(),
                up.as_os_str(),
                "sub".as_ref(),
            ],
        );
        sb.git(&repo.join("sub"), &["checkout", "-q", &rev1]);
        sb.git(&repo, &["add", "sub"]);
        (repo, rev1, rev2)
    }

    #[test]
    fn t10_sfo_08_submodule_never_stashed() {
        let sb = Sandbox::new();
        let (repo, rev1, rev2) = sub_staged(&sb);
        let head = |sb: &Sandbox| sb.git_text(&repo.join("sub"), &["rev-parse", "HEAD"]);
        with_stash(&sb, &repo, |_| assert_eq!(head(&sb), rev1));
        assert_eq!(sb.status_of(&repo, "sub"), "A");
        sb.git(&repo.join("sub"), &["checkout", "-q", &rev2]);
        assert_eq!(sb.status_of(&repo, "sub"), "AM");
        with_stash(&sb, &repo, |_| {
            assert_eq!(head(&sb), rev2);
            assert_eq!(sb.status_of(&repo, "sub"), "AM");
        });
        assert_eq!(head(&sb), rev2);
        assert_eq!(sb.status_of(&repo, "sub"), "AM");
    }

    #[test]
    fn t10_sfo_09_submodule_changes_kept_also_with_recurse() {
        for recurse in [false, true] {
            let sb = Sandbox::new();
            let (repo, _, _) = sub_staged(&sb);
            if recurse {
                sb.git(&repo, &["config", "submodule.recurse", "1"]);
            }
            sb.write(&repo, "bar", b"unstaged changes");
            sb.write(&repo, "sub/foo", b"foo contents");
            with_stash(&sb, &repo, |_| {
                assert_eq!(sb.read(&repo, "bar"), b"");
                assert_eq!(sb.read(&repo, "sub/foo"), b"foo contents");
            });
            assert_eq!(sb.read(&repo, "bar"), b"unstaged changes");
            assert_eq!(sb.read(&repo, "sub/foo"), b"foo contents");
        }
    }

    #[test]
    fn t10_sfo_10_utf8_and_latin1_changes() {
        for contents in ["\u{2603}".as_bytes(), b"\xfa"] {
            let sb = Sandbox::new();
            let repo = sb.foo_staged("r");
            sb.write(&repo, "foo", contents);
            with_stash(&sb, &repo, |_| foo_state(&sb, &repo, FOO.as_bytes(), "A"));
            foo_state(&sb, &repo, contents, "AM");
        }
    }

    #[test]
    fn t10_sfo_11_non_utf8_conflicting_diff() {
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        sb.write(&repo, "foo", b"\xfa \n");
        let out = with_stash(&sb, &repo, |_| {
            foo_state(&sb, &repo, FOO.as_bytes(), "A");
            sb.write(&repo, "foo", b"");
        });
        foo_state(&sb, &repo, b"\xfa \n", "AM");
        assert!(out.contains("Rolling back fixes..."));
    }

    /// `assert_no_diff`: the worktree matches the index.
    fn assert_no_diff(sb: &Sandbox, repo: &Path) {
        let tree = sb.git_text(repo, &["write-tree"]);
        let (code, _, err) = sb.git_status(repo, &["diff-index", &tree, "--exit-code"]);
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
    }

    fn crlf_case(autocrlf: &str, crlf_before: bool, crlf_after: bool, extra: Option<(&str, &str)>) {
        let sb = Sandbox::new();
        let repo = sb.init("r");
        sb.git(&repo, &["config", "--local", "core.autocrlf", autocrlf]);
        if let Some((k, v)) = extra {
            sb.git(&repo, &["config", "--local", k, v]);
        }
        let conv = |b: &[u8], crlf: bool| {
            if crlf {
                String::from_utf8_lossy(b)
                    .replace('\n', "\r\n")
                    .into_bytes()
            } else {
                b.to_vec()
            }
        };
        sb.write(&repo, "foo", &conv(b"1\n2\n", crlf_before));
        sb.git(&repo, &["add", "foo"]);
        let after = conv(b"3\n4\n\n", crlf_after);
        sb.write(&repo, "foo", &after);
        with_stash(&sb, &repo, |_| assert_no_diff(&sb, &repo));
    }

    #[test]
    fn t10_sfo_12_crlf_matrix() {
        for autocrlf in ["true", "false", "input"] {
            for before in [true, false] {
                for after in [true, false] {
                    crlf_case(autocrlf, before, after, None);
                }
            }
        }
    }

    #[test]
    fn t10_sfo_13_crlf_diff_only_quirk() {
        for autocrlf in ["true", "input"] {
            let sb = Sandbox::new();
            let repo = sb.init("r");
            sb.git(&repo, &["config", "--local", "core.autocrlf", autocrlf]);
            sb.write(&repo, "foo", b"1\r\n2\r\n3\r\n");
            sb.git(&repo, &["add", "foo"]);
            sb.write(&repo, "foo", b"1\n2\n3\n");
            let out = with_stash(&sb, &repo, |g| {
                // rc 1 with a blank patch: no stash, and not "clean" (the baseline must still be taken).
                assert_eq!(g.stashed(), &Stashed::Nothing);
                assert!(!g.diff_index_clean());
            });
            assert_eq!(out, "");
            assert_eq!(sb.read(&repo, "foo"), b"1\n2\n3\n");
        }
    }

    #[test]
    fn t10_sfo_14_whitespace_errors() {
        crlf_case("true", true, true, Some(("apply.whitespace", "error")));
    }

    #[test]
    fn t10_sfo_15_autocrlf_committed_crlf() {
        let sb = Sandbox::new();
        let repo = sb.init("r");
        sb.git(&repo, &["config", "--local", "core.autocrlf", "false"]);
        sb.write(&repo, "foo", b"1\r\n2\r\n");
        sb.git(&repo, &["add", "foo"]);
        sb.commit(&repo, "c");
        sb.git(&repo, &["config", "--local", "core.autocrlf", "true"]);
        sb.write(&repo, "foo", b"1\r\n2\r\n\r\n\r\n\r\n");
        with_stash(&sb, &repo, |_| assert_no_diff(&sb, &repo));
        assert_eq!(sb.read(&repo, "foo"), b"1\r\n2\r\n\r\n\r\n\r\n");
    }

    #[test]
    fn t10_sfo_16_intent_to_add() {
        let sb = Sandbox::new();
        let repo = sb.init("r");
        sb.write(&repo, "foo", b"hello\nworld\n");
        sb.git(&repo, &["add", "--intent-to-add", "foo"]);
        let ita = |sb: &Sandbox| {
            sb.git(
                &repo,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--ignore-submodules",
                    "--diff-filter=A",
                    "--name-only",
                    "-z",
                ],
            )
        };
        assert_eq!(ita(&sb), b"foo\0");
        let out = with_stash(&sb, &repo, |g| {
            assert_no_diff(&sb, &repo);
            assert_eq!(g.stashed(), &Stashed::Nothing);
        });
        assert_eq!(ita(&sb), b"foo\0");
        assert_eq!(out, "[WARNING] Unstaged intent-to-add files detected.\n");
        assert!(!repo.join(".git/snp-stash.journal").exists());
        assert!(sb.patch_files().is_empty(), "{:?}", sb.patch_files());
    }

    #[cfg(unix)]
    #[test]
    fn t10_sfo_17_failed_diff_does_not_discard_changes() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return; // root reads mode-000 files
        }
        let sb = Sandbox::new();
        let repo = sb.init("r");
        for i in 0..3 {
            sb.write(&repo, &i.to_string(), i.to_string().as_bytes());
        }
        sb.git(&repo, &["add", "0", "1", "2"]);
        for i in 0..3 {
            sb.write(&repo, &i.to_string(), b"new contents");
        }
        let tree = sb.git_text(&repo, &["write-tree"]);
        std::fs::set_permissions(repo.join("1"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let (cx, out, _) = sb.ctx(&repo);
        let e = snp_stash::enter(cx).unwrap_err();
        std::fs::set_permissions(repo.join("1"), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(e.exit_code(), 1);
        let (ty, text, hint) = exc(&e);
        assert_eq!(ty, "FatalError");
        assert_eq!(hint, None);
        assert_eq!(
            text,
            format!(
                "pre-commit failed to diff -- perhaps due to permissions?\n\ncommand: ('git', 'diff-index', \
                 '--ignore-submodules', '--binary', '--exit-code', '--no-color', '--no-ext-diff', '{tree}', '--')\n\
                 return code: 128\nstdout: (none)\nstderr:\n    error: open(\"1\"): Permission denied\n    fatal: cannot \
                 hash 1"
            )
        );
        assert_eq!(out.text(), "");
        for i in 0..3 {
            assert_eq!(sb.read(&repo, &i.to_string()), b"new contents");
        }
        // No patch, journal or pointer left behind; the lock is free again.
        assert!(sb.patch_files().is_empty(), "{:?}", sb.patch_files());
        assert!(!repo.join(".git/snp-stash.journal").exists());
        assert!(snp_stash::recovery_scan(&repo.join(".git"), &sb.patches(), &repo).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn x13_st_04_read_only_store_with_unstaged_changes() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        std::fs::create_dir_all(sb.patches()).unwrap();
        std::fs::set_permissions(sb.patches(), std::fs::Permissions::from_mode(0o500)).unwrap();
        // Clean: no store write needed, the run proceeds (parity of success).
        with_stash(&sb, &repo, |g| assert!(g.diff_index_clean()));
        // Unstaged: pre-commit's two lines, then its PermissionError (exit 3); the worktree is untouched.
        sb.write(&repo, "foo", b"herp\n");
        let (cx, out, _) = sb.ctx(&repo);
        let e = snp_stash::enter(cx).unwrap_err();
        std::fs::set_permissions(sb.patches(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(e.exit_code(), 3);
        let (ty, text, _) = exc(&e);
        assert_eq!(ty, "PermissionError");
        let lines: Vec<String> = out.text().lines().map(str::to_owned).collect();
        assert_eq!(lines[0], "[WARNING] Unstaged files detected.");
        let patch = lines[1]
            .strip_prefix("[INFO] Stashing unstaged files to ")
            .and_then(|l| l.strip_suffix('.'))
            .unwrap()
            .to_owned();
        assert_eq!(text, format!("[Errno 13] Permission denied: '{patch}'"));
        foo_state(&sb, &repo, b"herp\n", "AM");
    }

    #[test]
    fn x13_git_09_write_tree_or_checkout_fails() {
        // write-tree fails: CalledProcessError (exit 3), nothing changed.
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        sb.write(&repo, "foo", b"herp\n");
        let spy = Spy::new();
        spy.on(
            &["write-tree"],
            None,
            Reply::Exit(128, Vec::new(), b"fatal: boom\n".to_vec()),
        );
        let (git, info) = sb.discover_with(&repo, Arc::<Spy>::clone(&spy));
        let (cx, out, _) = sb.ctx_with(git, info);
        let e = snp_stash::enter(cx).unwrap_err();
        let (ty, text, hint) = exc(&e);
        assert_eq!(
            (ty.as_str(), e.exit_code(), hint),
            ("CalledProcessError", 3, None)
        );
        assert_eq!(
            text,
            format!(
                "command: ({}, 'write-tree')\nreturn code: 128\nstdout: (none)\nstderr:\n    fatal: boom",
                snp_pycompat::repr::repr_str(&git_exe(&sb, &repo))
            )
        );
        assert_eq!(out.text(), "");
        foo_state(&sb, &repo, b"herp\n", "AM");
    }

    #[test]
    fn x13_stash_01_checkout_fails_while_stashing() {
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        sb.write(&repo, "foo", b"herp\n");
        let spy = Spy::new();
        spy.on(
            &["-c", "submodule.recurse=0", "checkout", "--", "."],
            Some(1),
            Reply::Exit(1, Vec::new(), b"error: boom\n".to_vec()),
        );
        let (git, info) = sb.discover_with(&repo, Arc::<Spy>::clone(&spy));
        let (cx, out, _) = sb.ctx_with(git, info);
        let e = snp_stash::enter(cx).unwrap_err();
        let (ty, text, _) = exc(&e);
        assert_eq!((ty.as_str(), e.exit_code()), ("CalledProcessError", 3));
        assert_eq!(
            text,
            format!(
                "command: ({}, '-c', 'submodule.recurse=0', 'checkout', '--', '.')\nreturn code: 1\nstdout: \
                 (none)\nstderr:\n    error: boom",
                snp_pycompat::repr::repr_str(&git_exe(&sb, &repo))
            )
        );
        // The `finally` still restored: the applies fail (changes still present), rollback, apply.
        assert!(out.text().contains("Rolling back fixes..."));
        assert!(out.text().contains("[INFO] Restored changes from "));
        foo_state(&sb, &repo, b"herp\n", "AM");
    }

    #[test]
    fn x13_stash_02_apply_fails_twice_k_plus() {
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        sb.write(&repo, "foo", b"herp\n");
        let original = sb.unstaged_diff(&repo);
        let spy = Spy::new();
        spy.on(
            &["apply"],
            None,
            Reply::Exit(1, Vec::new(), b"error: nope\n".to_vec()),
        );
        spy.on(
            &["-c", "core.autocrlf=false", "apply"],
            None,
            Reply::Exit(1, Vec::new(), b"error: patch failed: foo:1\n".to_vec()),
        );
        let (git, info) = sb.discover_with(&repo, Arc::<Spy>::clone(&spy));
        let (cx, out, _) = sb.ctx_with(git, info);
        let guard = snp_stash::enter(cx).unwrap();
        let Stashed::Patch(patch) = guard.stashed().clone() else {
            panic!("no patch")
        };
        let e = guard.restore().unwrap_err();
        let (ty, text, hint) = exc(&e);
        assert_eq!((ty.as_str(), e.exit_code()), ("CalledProcessError", 3));
        assert_eq!(
            text,
            format!(
                "command: ({}, '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn', {})\nreturn code: \
                 1\nstdout: (none)\nstderr:\n    error: patch failed: foo:1",
                snp_pycompat::repr::repr_str(&git_exe(&sb, &repo)),
                snp_pycompat::repr::repr_str(&patch.display().to_string())
            )
        );
        assert_eq!(
            hint.unwrap(),
            format!(
                "[ERROR] Your unstaged changes are saved in {0}; restore them with: git apply --whitespace=nowarn \
                 {2}; if git apply reports that the patch does not apply, those files changed after the stash (hook \
                 fixes or later edits): keep a copy with git diff --binary {1} > {3}, put them back with git \
                 --literal-pathspecs restore --source={1} --worktree -- foo, then apply it again",
                patch.display(),
                sb.git_text(&repo, &["write-tree"]),
                // shell words (a Windows path is quoted)
                snp_pycompat::shlex::quote(&patch.display().to_string()),
                snp_pycompat::shlex::quote(&format!("{}.now", patch.display()))
            )
        );
        let lines: Vec<String> = out.text().lines().map(str::to_owned).collect();
        assert_eq!(
            lines[2..],
            ["[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...".to_owned()]
        );
        // The patch holds the only copy and is kept with its journal and pointer (state failed).
        assert_eq!(std::fs::read(&patch).unwrap(), original);
        let found = snp_stash::find_unrestored(&repo.join(".git"), &sb.patches(), &repo);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].patch.clone(), &*found[0].state),
            (patch.clone(), "failed")
        );
        // The hinted command restores the work.
        sb.git(
            &repo,
            &[
                std::ffi::OsStr::new("apply"),
                "--whitespace=nowarn".as_ref(),
                patch.as_os_str(),
            ],
        );
        assert_eq!(sb.read(&repo, "foo"), b"herp\n");
    }

    #[test]
    fn x13_stash_03_epipe_during_restore() {
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        sb.write(&repo, "foo", FOO.replace('1', "a").as_bytes());
        let (git, info) = sb.discover(&repo);
        let (mut cx, _, _) = sb.ctx_with(git, info);
        let err = Buf::default();
        cx.printer = Arc::new(printer(
            Box::new(Broken),
            Box::new(err),
            Brand::PreCommit,
            false,
        ));
        let printer = Arc::clone(&cx.printer);
        let guard = snp_stash::enter(cx).unwrap();
        // A conflicting hook fix: the rollback must still run although the warning cannot be printed.
        sb.write(&repo, "foo", FOO.replace('1', "b").as_bytes());
        guard.restore().unwrap();
        assert!(printer.is_closed());
        foo_state(&sb, &repo, FOO.replace('1', "a").as_bytes(), "AM");
    }

    #[test]
    fn log_lines_are_identical_in_both_brands_and_coloured_like_pre_commit() {
        for (brand, color, warn, info) in [
            (Brand::PreCommit, false, "[WARNING]", "[INFO]"),
            (Brand::Snp, false, "[WARNING]", "[INFO]"),
            (
                Brand::PreCommit,
                true,
                "\x1b[43;30m[WARNING]\x1b[m",
                "[INFO]\x1b[m",
            ),
            (
                Brand::Snp,
                true,
                "\x1b[43;30m[WARNING]\x1b[m",
                "[INFO]\x1b[m",
            ),
        ] {
            let sb = Sandbox::new();
            let repo = sb.foo_staged("r");
            sb.write(&repo, "foo", b"x\n");
            let (mut cx, _, _) = sb.ctx(&repo);
            let out = Buf::default();
            cx.printer = Arc::new(printer(
                Box::new(out.clone()),
                Box::new(Buf::default()),
                brand,
                color,
            ));
            let g = snp_stash::enter(cx).unwrap();
            let Stashed::Patch(p) = g.stashed().clone() else {
                panic!()
            };
            g.restore().unwrap();
            assert_eq!(
                out.text(),
                format!(
                    "{warn} Unstaged files detected.\n{info} Stashing unstaged files to {0}.\n{info} Restored changes \
                     from {0}.\n",
                    p.display()
                )
            );
        }
    }
}
