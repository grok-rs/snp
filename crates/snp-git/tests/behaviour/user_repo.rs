//! The rest of the user-repository API against real git: preflight queries, index entries, config readers,
//! `stage_fixed` helpers, the `--deduplicate` fallback (D-CRASH-26) and the pre-commit-hooks facts.

use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use snp_core::env::EnvPatch;
use snp_core::path::RepoPath;
use snp_git::pch_facts::PchFacts;
use snp_git::{Git, GitCaps, GitExe, GitVersion};
use snp_proc::spawn::Group;

use crate::common::{Sandbox, rep, sorted, strs, zl};

#[test]
fn preflight_queries() {
    let sb = Sandbox::new();
    let repo = sb.merge_conflict("conflict");
    let (git, _) = sb.discover(&repo);
    assert!(git.has_unmerged().unwrap());
    sb.resolve_conflict(&repo);
    assert!(!git.has_unmerged().unwrap());

    let cfg = OsStr::new(".pre-commit-config.yaml");
    assert!(!git.config_is_unstaged(cfg).unwrap());
    sb.write(&repo, ".pre-commit-config.yaml", b"repos: [] # changed\n");
    assert!(git.config_is_unstaged(cfg).unwrap());
    // a missing path is rc 128, not "unstaged"
    assert!(!git.config_is_unstaged(OsStr::new("nope.yaml")).unwrap());
}

#[test]
fn index_entries_match_ls_files_and_the_header_count() {
    let sb = Sandbox::new();
    let repo = sb.init("repo");
    sb.write(&repo, "a b.txt", b"a");
    sb.write(&repo, "dir/x.sh", b"#!/bin/sh\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            repo.join("dir/x.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    sb.git(&repo, &["add", "."]);
    sb.git(&repo, &["update-index", "--chmod=+x", "dir/x.sh"]);
    sb.git(&repo, &["commit", "-q", "-m", "base"]);
    let (git, info) = sb.discover(&repo);
    let tracked = git.tracked_with_modes().unwrap();
    assert_eq!(zl(&tracked.paths), strs(&["a b.txt", "dir/x.sh"]));
    assert_eq!(&*tracked.modes, &[0o100_644, 0o100_755]);
    assert_eq!(git.index_entry_count(&info), Some(2));

    sb.write(&repo, "new.txt", b"n");
    sb.write(&repo, "a b.txt", b"changed");
    sb.git(&repo, &["add", "."]);
    sb.git(&repo, &["rm", "-q", "--cached", "dir/x.sh"]);
    let staged = git.staged_with_modes().unwrap();
    assert_eq!(zl(&staged.paths), strs(&["a b.txt", "new.txt"]));
    assert_eq!(&*staged.modes, &[0o100_644, 0o100_644]);
    assert_eq!(git.index_entry_count(&info), Some(2));

    let empty = sb.init("empty");
    let (git, info) = sb.discover(&empty);
    assert_eq!(git.index_entry_count(&info), None, "no index file yet");
    std::fs::write(&info.index_file, b"NOPE\0\0\0\x02\0\0\0\x01").unwrap();
    assert_eq!(git.index_entry_count(&info), None);
}

#[test]
fn config_readers() {
    let sb = Sandbox::new();
    let repo = sb.init("repo");
    let (git, _) = sb.discover(&repo);
    assert_eq!(git.hooks_path().unwrap(), None);
    sb.git(&repo, &["config", "core.hooksPath", " .githooks "]);
    assert_eq!(git.hooks_path().unwrap(), Some(OsString::from(".githooks")));
    assert_eq!(
        git.config_get("core.hooksPath").unwrap(),
        Some(OsString::from(" .githooks "))
    );
    assert_eq!(git.config_get("core.nosuchkey").unwrap(), None);

    sb.git(
        &repo,
        &[
            "config",
            "--add",
            "hook.pre-commit.pre-commit.event",
            "pre-commit",
        ],
    );
    sb.git(
        &repo,
        &[
            "config",
            "--add",
            "hook.pre-commit.pre-push.event",
            "pre-push",
        ],
    );
    std::fs::write(
        repo.join(".git/config"),
        [
            std::fs::read(repo.join(".git/config")).unwrap(),
            b"[flag]\n\tbare\n".to_vec(),
        ]
        .concat(),
    )
    .unwrap();
    assert_eq!(
        git.config_get_regexp(r"^hook\.pre-commit\..*\.event$")
            .unwrap(),
        vec![
            (
                OsString::from("hook.pre-commit.pre-commit.event"),
                OsString::from("pre-commit")
            ),
            (
                OsString::from("hook.pre-commit.pre-push.event"),
                OsString::from("pre-push")
            ),
        ]
    );
    assert_eq!(
        git.config_get_regexp("^flag\\.").unwrap(),
        vec![(OsString::from("flag.bare"), OsString::new())]
    );
    assert_eq!(git.config_get_regexp("^nothing$").unwrap(), Vec::new());

    let origins = git.config_origins().unwrap();
    assert_eq!(
        origins,
        vec![sb.root.join("home/.gitconfig"), repo.join(".git/config")],
        "global then local, deduplicated"
    );
}

#[test]
fn stage_fixed_helpers_and_raw_calls() {
    let sb = Sandbox::new();
    let repo = sb.init("repo");
    sb.write(&repo, "[ab].txt", b"1\n");
    sb.write(&repo, "a.txt", b"1\n");
    sb.git(&repo, &["add", "."]);
    sb.commit(&repo, "base");
    sb.write(&repo, "[ab].txt", b"2\n");
    sb.write(&repo, "a.txt", b"2\n");
    let (git, _) = sb.discover(&repo);
    // literal pathspecs: `[ab].txt` is not a glob that would stage a.txt
    git.add_literal(&[RepoPath::new(b"[ab].txt").unwrap()], Group::Isolated)
        .unwrap();
    assert_eq!(zl(&git.staged_files().unwrap()), strs(&["[ab].txt"]));
    let e = git
        .add_literal(&[RepoPath::new(b"missing.txt").unwrap()], Group::Isolated)
        .unwrap_err();
    assert_eq!(crate::common::exc(&e).0, "CalledProcessError");

    // the user's unstaged change as a patch, the worktree reset to the index (the stash's state during hooks)
    let patch = sb.path("p.patch");
    std::fs::write(&patch, sb.git(&repo, &["diff", "--binary", "--", "a.txt"])).unwrap();
    sb.git(&repo, &["checkout", "--", "a.txt"]);
    assert!(git.apply_check(&patch).unwrap());
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"1\n",
        "--check changes nothing"
    );
    // a hook fix that conflicts with the patch
    sb.write(&repo, "a.txt", b"fixed by a hook\n");
    assert!(!git.apply_check(&patch).unwrap());

    let c = git
        .run_raw(&[OsStr::new("rev-parse"), OsStr::new("--is-inside-work-tree")])
        .unwrap();
    assert_eq!((c.status.0, c.stdout.as_slice()), (0, &b"true\n"[..]));
    git.show_diff(false).unwrap();
}

#[test]
fn dedup_fallback_equals_ls_files_deduplicate() {
    // D-CRASH-26: on git < 2.31 `ls-files -z` + adjacent dedup gives the `--deduplicate` list byte for byte
    let sb = Sandbox::new();
    let repo = sb.merge_conflict("conflict");
    sb.write(&repo, "интервью", b"");
    sb.git(&repo, &["add", "интервью"]);
    let real = sb.exe();
    let (new_git, info) = sb.discover(&repo);
    let old_exe = Arc::new(GitExe {
        argv0: real.argv0.clone(),
        caps: GitCaps {
            version: Some(GitVersion {
                major: 2,
                minor: 30,
                patch: 2,
            }),
            ls_files_dedup: false,
            path_format: false,
            config_env: false,
            init_object_format: true,
            config_hooks: false,
            pathspec_from_file: true,
            diff_no_relative: true,
        },
    });
    let (old_git, old_info) = Git::discover(old_exe, &sb.env, &repo, sb.backend()).unwrap();
    // the separate-call discovery of the warning band finds the same repository
    assert_eq!(
        (&old_info.root, &old_info.git_dir),
        (&info.root, &info.git_dir)
    );
    let raw = sb.git(&repo, &["ls-files", "-z"]);
    let dupes = raw
        .split(|&b| b == 0)
        .filter(|r| *r == b"conflict_file")
        .count();
    assert_eq!(
        dupes, 2,
        "unmerged entries appear once per stage (ours and theirs: no common base)"
    );
    let with_flag = sb.git(&repo, &["ls-files", "-z", "--deduplicate"]);
    let fallback = old_git.all_files().unwrap();
    assert_eq!(zl(&fallback), zl(&new_git.all_files().unwrap()));
    let joined: Vec<u8> = fallback.iter().flat_map(|r| [r, b"\0"].concat()).collect();
    assert_eq!(joined, with_flag);
}

/// D-CRASH-26 on a real git 2.30 (WBS WP-18 acceptance): `crates/snp-git/tests/old-git/git` runs Debian 11's git
/// 2.30.2 in a container. snp-git resolves it, gets the warning-band capabilities, discovers the repository with
/// separate `rev-parse` calls and lists the files with the fallback, which equals the host git's
/// `ls-files -z --deduplicate` byte for byte.
#[test]
#[ignore = "needs docker: SNP_TEST_OLD_GIT_DIR=crates/snp-git/tests/old-git cargo test -- --ignored old_git"]
fn old_git_dedup_fallback_on_a_git_2_30_container() {
    let process = snp_core::env::Env::capture_process();
    let Some(dir) = process.get("SNP_TEST_OLD_GIT_DIR") else {
        eprintln!("SNP_TEST_OLD_GIT_DIR unset: skipped");
        return;
    };
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(dir);
    let sb = Sandbox::new();
    let repo = sb.merge_conflict("conflict");
    sb.write(&repo, "sp ace", b"");
    sb.git(&repo, &["add", "sp ace"]);
    let mut path = dir.into_os_string();
    path.push(":");
    path.push(sb.env.get("PATH").unwrap());
    let old_env = sb.env.patched(&EnvPatch::default().set("PATH", path));
    let old = GitExe::resolve(&old_env, &repo, None).unwrap();
    let v = old.caps.version.unwrap();
    assert!(
        v < GitVersion {
            major: 2,
            minor: 31,
            patch: 0
        },
        "{v}"
    );
    assert!(!old.caps.ls_files_dedup && !old.caps.path_format);
    let (git, info) = Git::discover(Arc::new(old), &sb.env, &repo, sb.backend()).unwrap();
    assert_eq!(info.root, repo);
    let fallback = git.all_files().unwrap();
    let joined: Vec<u8> = fallback.iter().flat_map(|r| [r, b"\0"].concat()).collect();
    assert_eq!(joined, sb.git(&repo, &["ls-files", "-z", "--deduplicate"]));
    // and the real flag fails on that git exactly as pre-commit 4.7.0 does (rc 129)
    let raw = git
        .run_raw(&[
            OsStr::new("ls-files"),
            OsStr::new("-z"),
            OsStr::new("--deduplicate"),
        ])
        .unwrap();
    assert_eq!(raw.status.0, 129);
}

#[test]
fn pch_facts_use_upstream_argv_and_invalidate_on_index_changes() {
    let sb = Sandbox::new();
    let repo = sb.init("repo");
    sb.write(&repo, "kept.txt", b"k");
    sb.write(&repo, "Café.txt", b"c");
    sb.git(&repo, &["add", "."]);
    sb.commit(&repo, "base");
    sb.git(&repo, &["checkout", "-q", "-b", "feature/x"]);
    sb.write(&repo, "added.txt", b"a");
    sb.write(&repo, "café-new.txt", b"c");
    sb.write(&repo, "big.dat", b"lfs");
    sb.write(&repo, ".gitattributes", b"*.dat filter=lfs\n");
    sb.git(&repo, &["add", "."]);
    let (git, _) = sb.discover(&repo);
    let facts = PchFacts::new(git);
    // quotePath honoured (bug-compat): non-ASCII names are C-quoted and never match a file name
    assert_eq!(
        sorted(facts.added_files().unwrap().to_vec()),
        sorted(strs(&[
            ".gitattributes",
            "added.txt",
            "big.dat",
            "\"caf\\303\\251-new.txt\""
        ]))
    );
    assert_eq!(
        sorted(facts.tracked_files().unwrap().to_vec()),
        sorted(strs(&[
            ".gitattributes",
            "\"Caf\\303\\251.txt\"",
            "added.txt",
            "big.dat",
            "\"caf\\303\\251-new.txt\"",
            "kept.txt"
        ]))
    );
    let modes = facts.index_modes().unwrap();
    assert!(modes.iter().all(|(m, _)| *m == 0o100_644));
    assert_eq!(modes.len(), 6);
    assert_eq!(
        facts.current_branch().unwrap().as_deref(),
        Some("feature/x")
    );
    assert_eq!(
        facts
            .lfs_files(&[b"big.dat".as_slice(), b"kept.txt"])
            .unwrap(),
        vec![b"big.dat".to_vec()]
    );
    assert_eq!(facts.lfs_files(&[]).unwrap(), Vec::<Vec<u8>>::new());

    // another hook stages a file: the cached facts follow the index
    sb.write(&repo, "later.txt", b"l");
    sb.git(&repo, &["add", "later.txt"]);
    assert!(
        facts
            .added_files()
            .unwrap()
            .contains(&b"later.txt".to_vec())
    );
    // a detached HEAD is "not on a branch"
    let head = sb.head(&repo);
    sb.git(&repo, &["commit", "-q", "-m", "x"]);
    sb.git(&repo, &["checkout", "-q", head.as_str()]);
    assert_eq!(facts.current_branch().unwrap(), None);
    let raw = facts
        .raw(&[OsStr::new("config"), OsStr::new("core.fileMode")])
        .unwrap();
    assert_eq!(raw.status.0, 0);

    // the hook env is the Git's env
    let env = sb
        .env
        .patched(&EnvPatch::default().set("GIT_INDEX_FILE", repo.join(".git/other-index")));
    let (git, _) = sb.discover(&repo);
    let facts = PchFacts::new(git.with_env(env));
    assert!(facts.tracked_files().unwrap().is_empty());
}

/// ADR-084: `config core.fileMode` is asked once per state of the repository's config (one spawn for all
/// partitions of `check-executables-have-shebangs`), and a config change is seen.
#[test]
fn file_mode_is_memoised_until_the_config_changes() {
    let sb = Sandbox::new();
    let repo = sb.init("filemode");
    // `git init` writes `filemode = false` where the file system has no executable bit (Windows).
    sb.git(&repo, &["config", "core.fileMode", "true"]);
    let (git, _) = sb.discover(&repo);
    let facts = PchFacts::new(git);
    let ask = || {
        let c = facts
            .raw(&[OsStr::new("config"), OsStr::new("core.fileMode")])
            .unwrap();
        (c.status.0, String::from_utf8(c.stdout).unwrap())
    };
    assert_eq!(ask(), (0, "true\n".to_owned()));
    // A spawn would see this edit made behind git's back with the same size: the memo answers instead.
    let config = repo.join(".git/config");
    let before = std::fs::metadata(&config).unwrap().modified().unwrap();
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, text.replace("filemode = true", "filemode = fals")).unwrap(); // spellchecker:disable-line
    std::fs::File::options()
        .write(true)
        .open(&config)
        .unwrap()
        .set_modified(before)
        .unwrap();
    assert_eq!(ask(), (0, "true\n".to_owned()));
    // `git config` rewrites the file (new inode, new stamp): the next ask spawns again.
    std::fs::write(&config, text).unwrap();
    sb.git(&repo, &["config", "core.fileMode", "false"]);
    assert_eq!(ask(), (0, "false\n".to_owned()));
    sb.git(&repo, &["config", "--unset", "core.fileMode"]);
    assert_eq!(ask().0, 1);
}

/// ADR-082: from the second call on, `lfs_files` answers index paths from one whole-index `check-attr`; the answer
/// equals the direct call's, follows `.gitattributes` edits, keeps input order and repeats, and asks git directly
/// for paths outside the index.
#[test]
fn lfs_files_memo_agrees_with_check_attr() {
    let sb = Sandbox::new();
    let repo = sb.init("lfs-memo");
    sb.write(&repo, ".gitattributes", b"*.dat filter=lfs\n");
    sb.write(&repo, "a.dat", b"a");
    sb.write(&repo, "a.txt", b"a");
    sb.write(&repo, "sub/b.dat", b"b");
    sb.write(&repo, "sub/b.bin", b"b");
    sb.git(&repo, &["add", "."]);
    let (git, _) = sb.discover(&repo);
    let facts = PchFacts::new(git.clone());
    let all: [&[u8]; 4] = [b"a.dat", b"a.txt", b"sub/b.dat", b"sub/b.bin"];
    let first = facts.lfs_files(&all[..2]).unwrap();
    assert_eq!(first, vec![b"a.dat".to_vec()]);
    // the memo (second call) and repeats in input order, as git echoes them
    let again = facts
        .lfs_files(&[b"sub/b.dat", b"a.dat", b"sub/b.dat", b"a.txt"])
        .unwrap();
    assert_eq!(
        again,
        vec![
            b"sub/b.dat".to_vec(),
            b"a.dat".to_vec(),
            b"sub/b.dat".to_vec()
        ]
    );
    // a nested `.gitattributes` written by an earlier hook is honoured
    sb.write(&repo, "sub/.gitattributes", b"*.bin filter=lfs\n");
    assert_eq!(
        facts.lfs_files(&all[2..]).unwrap(),
        vec![b"sub/b.dat".to_vec(), b"sub/b.bin".to_vec()]
    );
    assert_eq!(
        facts.lfs_files(&all).unwrap(),
        PchFacts::new(git).lfs_files(&all).unwrap()
    );
    // an untracked path is asked directly
    sb.write(&repo, "new.dat", b"n");
    assert_eq!(
        facts.lfs_files(&[b"new.dat", b"a.txt"]).unwrap(),
        vec![b"new.dat".to_vec()]
    );
    // `rev-parse --git-dir` is memoised, with the same answer
    let args = [OsStr::new("rev-parse"), OsStr::new("--git-dir")];
    let one = facts.raw(&args).unwrap();
    let two = facts.raw(&args).unwrap();
    assert_eq!((one.status.0, &one.stdout), (two.status.0, &two.stdout));
    assert_eq!(one.stdout, b".git\n");
}

/// D-DATA-19 on git < 2.25 (no `checkout --pathspec-from-file`): the literal pathspecs go on the command line in
/// chunks; together the chunks rewrite exactly the listed files (glob characters and spaces taken literally), and a
/// change `git diff` cannot see (an `--assume-unchanged` edit) is left alone.
#[test]
fn checkout_paths_without_pathspec_from_file_chunks_literal_arguments() {
    let sb = Sandbox::new();
    let repo = sb.init("chunks");
    // 600 names of ~100 bytes (short enough for Windows' MAX_PATH under a temp dir) need several 24 KiB chunks.
    let long = rep('n', 96);
    let mut names: Vec<String> = (0..600).map(|i| format!("{long}{i}")).collect();
    names.extend(["[ab]".to_owned(), "with space".to_owned()]);
    if cfg!(unix) {
        // Not a valid Windows file name.
        names.push("*".to_owned());
    }
    for n in &names {
        sb.write(&repo, n, b"base\n");
    }
    sb.write(&repo, "keep", b"base\n");
    sb.git(&repo, &["add", "."]);
    sb.commit(&repo, "base");
    sb.git(&repo, &["update-index", "--assume-unchanged", "keep"]);
    for n in &names {
        sb.write(&repo, n, b"changed\n");
    }
    sb.write(&repo, "keep", b"local edit\n");
    let real = sb.exe();
    let old_exe = Arc::new(GitExe {
        argv0: real.argv0.clone(),
        caps: GitCaps {
            version: Some(GitVersion {
                major: 2,
                minor: 24,
                patch: 0,
            }),
            ls_files_dedup: false,
            path_format: false,
            config_env: false,
            init_object_format: false,
            config_hooks: false,
            pathspec_from_file: false,
            diff_no_relative: false,
        },
    });
    let (git, _) = Git::discover(old_exe, &sb.env, &repo, sb.backend()).unwrap();
    let paths = git.changed_paths().unwrap();
    assert_eq!(
        paths.len(),
        names.len(),
        "the assume-unchanged edit is not listed"
    );
    let calls = git.checkout_path_calls(&paths).unwrap();
    assert!(calls.len() > 1, "{} names need several chunks", names.len());
    for c in &calls {
        let first: Vec<&str> = c.args.iter().take(4).map(|a| a.to_str().unwrap()).collect();
        assert_eq!(first, ["-c", "submodule.recurse=0", "checkout", "--"]);
        assert_eq!(c.stdin, b"");
    }
    git.checkout_paths(&paths, true, Group::Inherit).unwrap();
    for n in &names {
        assert_eq!(std::fs::read(repo.join(n)).unwrap(), b"base\n", "{n}");
    }
    assert_eq!(std::fs::read(repo.join("keep")).unwrap(), b"local edit\n");
}
