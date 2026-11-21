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
            attr_system: false,
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

/// ADR-106: `ls-files -s -v -z` folded by `all_files_tagged` is `ls-files -z --deduplicate` byte for byte, in every
/// index shape, and `plain` holds exactly when no entry is flagged or unmerged.
#[test]
fn all_files_tagged_equals_ls_files_deduplicate() {
    let sb = Sandbox::new();
    let check = |repo: &std::path::Path, plain: bool| {
        let (git, _) = sb.discover(repo);
        let tagged = git.all_files_tagged().unwrap();
        assert_eq!(zl(&tagged.paths), zl(&git.all_files().unwrap()), "{repo:?}");
        let joined: Vec<u8> = tagged
            .paths
            .iter()
            .flat_map(|r| [r, b"\0"].concat())
            .collect();
        // `--deduplicate` is git >= 2.31; older git is covered by the all_files() comparison above
        if sb.exe().caps.ls_files_dedup {
            assert_eq!(joined, sb.git(repo, &["ls-files", "-z", "--deduplicate"]));
        }
        assert_eq!(tagged.modes.len(), tagged.paths.len());
        assert_eq!(tagged.plain, plain, "{repo:?}");
        tagged
    };
    // a plain index: odd names, an executable, a symlink, an empty file (mode 0: proves nothing), a gitlink
    let repo = sb.init("plain");
    sb.write(&repo, "a b.txt", b"a");
    #[cfg(unix)] // not valid Windows file names
    {
        sb.write(&repo, "tab\there", b"t");
        sb.write(&repo, "nl\nhere", b"n");
    }
    sb.write(&repo, "интервью", b"i");
    sb.write(&repo, "x.sh", b"#!/bin/sh\n");
    sb.write(&repo, "empty", b"");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(repo.join("x.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::os::unix::fs::symlink("a b.txt", repo.join("link")).unwrap();
    }
    sb.git(&repo, &["add", "."]);
    // an unpopulated submodule: an empty directory at the gitlink's path
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    let oid = sb.git_text(&repo, &["hash-object", "x.sh"]);
    sb.git(
        &repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},sub", oid.trim()),
        ],
    );
    sb.commit(&repo, "init");
    let t = check(&repo, true);
    let mode = |name: &[u8]| {
        let i = t.paths.iter().position(|p| p == name).unwrap();
        t.modes[i]
    };
    assert_eq!(mode(b"a b.txt"), 0o100_644);
    assert_eq!(mode(b"empty"), 0, "the empty blob proves nothing");
    assert_eq!(mode(b"sub"), 0o160_000);
    #[cfg(unix)]
    {
        assert_eq!(mode(b"x.sh"), 0o100_755);
        assert_eq!(mode(b"link"), 0o120_000);
    }
    // assume-unchanged, skip-worktree: flagged
    sb.git(&repo, &["update-index", "--assume-unchanged", "a b.txt"]);
    check(&repo, false);
    sb.git(&repo, &["update-index", "--no-assume-unchanged", "a b.txt"]);
    sb.git(&repo, &["update-index", "--skip-worktree", "a b.txt"]);
    check(&repo, false);
    sb.git(&repo, &["update-index", "--no-skip-worktree", "a b.txt"]);
    check(&repo, true);
    // an intent-to-add entry is a plain cached entry (only the empty `git diff` excludes it)
    sb.write(&repo, "ita.txt", b"i");
    sb.git(&repo, &["add", "-N", "ita.txt"]);
    check(&repo, true);
    // an unmerged index: every stage once, not plain
    let repo = sb.merge_conflict("conflict");
    check(&repo, false);
}

/// ADR-106: git turns fsmonitor on from the environment when `core.fsmonitor` is unset (`GIT_TEST_FSMONITOR`, named
/// `GIT_FSMONITOR_TEST` before git 2.20), which no configuration query can see, and `git diff` then skips the
/// `lstat` of every fsmonitor-valid entry: with a hook that reports no change, a deleted file stays out of the diff.
/// `-f` lists those entries in lowercase, so the listing is not plain, and the gate refuses on the variable itself.
#[cfg(unix)]
#[test]
fn fsmonitor_valid_entries_are_flagged() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let repo = sb.init("repo");
    sb.write(&repo, "f.txt", b"f\n");
    sb.write(&repo, "g.txt", b"g\n");
    sb.git(&repo, &["add", "."]);
    sb.commit(&repo, "init");
    // protocol 2 (git ≥ 2.26): a token and no path; protocol 1: no path
    let hook = sb.path("fsmonitor-none");
    std::fs::write(
        &hook,
        "#!/bin/sh\nif test \"$1\" = 2; then printf 'last_update_token\\0'; fi\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let hook = hook.to_str().unwrap();
    let vars = [("GIT_TEST_FSMONITOR", hook), ("GIT_FSMONITOR_TEST", hook)];
    // the first refresh writes the fsmonitor extension, the second marks the entries fsmonitor-valid
    for _ in 0..2 {
        sb.git_with(&repo, &["update-index", "-q", "--refresh"], &vars);
    }
    std::fs::remove_file(repo.join("f.txt")).unwrap();
    let (git, _) = sb.discover(&repo);
    let patch = vars
        .iter()
        .fold(EnvPatch::default(), |p, &(k, v)| p.set(k, v));
    let fsm = git.with_env(sb.env.patched(&patch));
    let tagged = fsm.all_files_tagged().unwrap();
    assert_eq!(zl(&tagged.paths), strs(&["f.txt", "g.txt"]));
    assert!(!tagged.plain, "fsmonitor-valid entries are tagged `h`");
    assert!(!fsm.trusts_index_modes());
    // the same index without fsmonitor: every entry `H`, and `git diff` sees the deletion
    assert!(git.all_files_tagged().unwrap().plain);
    assert!(git.trusts_index_modes());
    assert!(git.diff_fingerprint_clean().unwrap().0.non_empty);
}

/// ADR-106: `diff_fingerprint_clean` keeps the stdout-only fingerprint and reports whether git exited 0 with an empty
/// stderr. `git diff` reports an unsearchable directory on stderr only and exits 0: not clean.
#[cfg(unix)]
#[test]
fn diff_fingerprint_clean_needs_status_and_stderr() {
    use std::os::unix::fs::PermissionsExt;

    use snp_git::DiffFingerprint;
    let sb = Sandbox::new();
    let repo = sb.init("repo");
    sb.write(&repo, "sub/a.txt", b"a\n");
    sb.write(&repo, "b.txt", b"b\n");
    sb.git(&repo, &["add", "."]);
    sb.commit(&repo, "init");
    let (git, _) = sb.discover(&repo);
    let (fp, clean) = git.diff_fingerprint_clean().unwrap();
    assert_eq!((fp, clean), (DiffFingerprint::EMPTY, true));
    assert_eq!(git.diff_fingerprint().unwrap(), fp);
    sb.write(&repo, "b.txt", b"changed\n");
    let (fp, clean) = git.diff_fingerprint_clean().unwrap();
    assert!(fp.non_empty && clean);
    sb.git(&repo, &["checkout", "--", "b.txt"]);
    if !rustix::process::geteuid().is_root() {
        std::fs::set_permissions(repo.join("sub"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let (fp, clean) = git.diff_fingerprint_clean().unwrap();
        std::fs::set_permissions(repo.join("sub"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(fp, DiffFingerprint::EMPTY, "stdout stays empty");
        assert!(!clean, "the EACCES on stderr makes it unclean");
    }
}

/// ADR-106: the configuration gate, as git resolves it (repository config, `-c`, `GIT_CONFIG_COUNT`).
#[test]
fn trusts_index_modes_follows_the_effective_config() {
    let sb = Sandbox::new();
    let repo = sb.init("repo");
    let (git, _) = sb.discover(&repo);
    let unset = |key: &str| {
        drop(sb.git_status(&repo, &["config", "--unset-all", key]));
    };
    for key in ["core.filemode", "core.symlinks"] {
        unset(key);
    }
    assert!(git.trusts_index_modes(), "unset counts as git's default");
    for (key, value, trusted) in [
        ("core.fileMode", "false", false),
        ("core.fileMode", "true", true),
        ("core.symlinks", "false", false),
        ("core.symlinks", "yes", true),
        ("core.fsmonitor", "true", false),
        ("core.fsmonitor", "/path/to/hook", false),
        ("core.fsmonitor", "false", true),
        ("core.useBuiltinFSMonitor", "true", false),
        ("core.ignoreStat", "true", false),
        ("core.sparseCheckout", "true", false),
        ("index.sparse", "true", false),
        ("index.sparse", "false", true),
    ] {
        sb.git(&repo, &["config", key, value]);
        assert_eq!(git.trusts_index_modes(), trusted, "{key}={value}");
        unset(key);
    }
    // `-c` and `GIT_CONFIG_COUNT` are git's own business: both count (`GIT_CONFIG_PARAMETERS` in the pre-2.31
    // `'key=value'` form, which every git reads)
    let with = |vars: &[(&'static str, &'static str)]| {
        let patch = vars
            .iter()
            .fold(EnvPatch::default(), |p, &(k, v)| p.set(k, v));
        git.with_env(sb.env.patched(&patch)).trusts_index_modes()
    };
    assert!(!with(&[("GIT_CONFIG_PARAMETERS", "'core.fsmonitor=true'")]));
    // `GIT_CONFIG_COUNT` is git >= 2.31 (the same release as `ls-files --deduplicate`); older git ignores it, and
    // so does its `git config` answer
    if sb.exe().caps.ls_files_dedup {
        assert!(!with(&[
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "core.symlinks"),
            ("GIT_CONFIG_VALUE_0", "false"),
        ]));
    }
    assert!(with(&[("GIT_CONFIG_PARAMETERS", "'core.fsmonitor=false'")]));
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
        PchFacts::new(git.clone()).lfs_files(&all).unwrap()
    );
    // `<git dir>/info/attributes` and the root `.gitattributes` are stamped too
    sb.write(&repo, ".git/info/attributes", b"a.txt filter=lfs\n");
    assert_eq!(
        facts.lfs_files(&all[..2]).unwrap(),
        vec![b"a.dat".to_vec(), b"a.txt".to_vec()]
    );
    sb.write(&repo, ".git/info/attributes", b"");
    sb.write(
        &repo,
        ".gitattributes",
        b"*.dat  filter=lfs\n*.txt filter=lfs\n",
    );
    assert_eq!(
        facts.lfs_files(&all).unwrap(),
        PchFacts::new(git).lfs_files(&all).unwrap()
    );
    assert_eq!(
        facts.lfs_files(&all[..2]).unwrap(),
        vec![b"a.dat".to_vec(), b"a.txt".to_vec()]
    );
    sb.write(&repo, ".gitattributes", b"*.dat filter=lfs\n");
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
            attr_system: false,
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

/// ADR-133: query F's `-v` tags against real git: `H` cached, lowercase when assume-unchanged, `S` skip-worktree,
/// `M` per stage of an unmerged path; one tag per entry, in step with the paths.
#[test]
fn query_f_tags_assume_unchanged_and_skip_worktree() {
    let sb = Sandbox::new();
    let repo = sb.init("tags");
    for f in ["a", "b", "c"] {
        sb.write(&repo, f, b"x\n");
    }
    sb.git(&repo, &["add", "."]);
    sb.git(&repo, &["update-index", "--assume-unchanged", "b"]);
    sb.git(&repo, &["update-index", "--skip-worktree", "c"]);
    let (git, _) = sb.discover(&repo);
    let e = git.tracked_with_modes().unwrap();
    assert_eq!(zl(&e.paths), [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    assert_eq!(&*e.tags, b"HhS");
    let (staged, unmerged) = git.tracked_with_stages().unwrap();
    assert!(!unmerged);
    assert_eq!(staged.tags, e.tags);
    let uu = sb.merge_conflict("uu-tags");
    let (git, _) = sb.discover(&uu);
    let (e, unmerged) = git.tracked_with_stages().unwrap();
    assert!(unmerged);
    assert_eq!(e.tags.len(), e.paths.len());
    assert!(e.tags.contains(&b'M'));
}

/// ADR-133: `git var GIT_ATTR_SYSTEM` names an absolute file on git >= 2.42 and nothing under `GIT_ATTR_NOSYSTEM`;
/// on an older git it is not even asked.
#[test]
fn system_attributes_from_git_var() {
    let sb = Sandbox::new();
    let repo = sb.init("var");
    let (git, _) = sb.discover(&repo);
    let new = sb.exe().caps.attr_system;
    match git.system_attributes() {
        Some(snp_git::SystemAttributes::File(p)) => assert!(new && p.is_absolute(), "{p:?}"),
        other => assert!(!new && other.is_none(), "{other:?}"),
    }
    let env = sb
        .env
        .patched(&EnvPatch::default().set("GIT_ATTR_NOSYSTEM", "1"));
    let (git, _) = Git::discover(sb.exe(), &env, &repo, sb.backend()).unwrap();
    let want = new.then_some(snp_git::SystemAttributes::Disabled);
    assert_eq!(git.system_attributes(), want);
    // a value git cannot read as a boolean makes `git var` die: no answer, never "disabled"
    let env = sb
        .env
        .patched(&EnvPatch::default().set("GIT_ATTR_NOSYSTEM", "maybe"));
    let (git, _) = Git::discover(sb.exe(), &env, &repo, sb.backend()).unwrap();
    assert_eq!(git.system_attributes(), None);
}

/// COMPATIBILITY §6.2 equivalence (ADR-104): the unmerged flag of `tracked_with_stages` (one `ls-files -s -v -z`) equals
/// `has_unmerged` (`ls-files --unmerged`, pre-commit's `_has_unmerged_paths`) in every index shape a merge can leave.
#[test]
fn unmerged_from_ls_files_stage_matches_ls_files_unmerged() {
    let sb = Sandbox::new();
    let check = |repo: &std::path::Path, expected: bool, what: &str| {
        let (git, _) = sb.discover(repo);
        let (entries, derived) = git.tracked_with_stages().unwrap();
        assert_eq!(
            derived,
            git.has_unmerged().unwrap(),
            "{what}: derived vs asked"
        );
        assert_eq!(derived, expected, "{what}");
        let modes = git.tracked_with_modes().unwrap();
        assert_eq!(zl(&entries.paths), zl(&modes.paths), "{what}: same entries");
        assert_eq!(entries.modes, modes.modes, "{what}: same modes");
        assert_eq!(entries.tags, modes.tags, "{what}: same tags");
        assert_eq!(
            entries.tags.len(),
            entries.modes.len(),
            "{what}: a tag per entry"
        );
    };
    // both sides modified (UU)
    let uu = sb.merge_conflict("uu");
    check(&uu, true, "both modified");
    sb.resolve_conflict(&uu);
    check(&uu, false, "resolved: stage 0 only");

    // deleted by us (DU) and deleted by them (UD), in one merge
    let du = sb.init("du");
    sb.write(&du, "ours-deletes", b"base\n");
    sb.write(&du, "theirs-deletes", b"base\n");
    sb.git(&du, &["add", "."]);
    sb.commit(&du, "base");
    sb.git(&du, &["checkout", "-q", "-b", "other"]);
    sb.write(&du, "ours-deletes", b"theirs\n");
    sb.git(&du, &["rm", "-q", "theirs-deletes"]);
    sb.commit(&du, "other");
    sb.git(&du, &["checkout", "-q", "-"]);
    sb.git(&du, &["rm", "-q", "ours-deletes"]);
    sb.write(&du, "theirs-deletes", b"ours\n");
    sb.commit(&du, "mine");
    let _conflicted = sb.git_status(&du, &["merge", "-q", "other"]);
    check(&du, true, "deleted by us and by them");
    sb.git(&du, &["rm", "-q", "ours-deletes"]);
    check(&du, true, "deleted by them only");
    sb.git(&du, &["add", "theirs-deletes"]);
    check(&du, false, "both resolved");

    // a gitlink conflict (stages 1–3 of a submodule entry)
    let gl = sb.init("gitlink");
    sb.commit(&gl, "base");
    let oid = sb.git_text(&gl, &["rev-parse", "HEAD"]);
    let info = format!("160000 {oid} 1\tsub\n160000 {oid} 2\tsub\n160000 {oid} 3\tsub\n");
    let args = [OsStr::new("update-index"), OsStr::new("--index-info")];
    let mut child = sb
        .command(&gl, &args)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), info.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
    check(&gl, true, "gitlink conflict");

    // an empty index, and no index file at all
    let empty = sb.init("empty-index");
    sb.git(&empty, &["read-tree", "--empty"]);
    check(&empty, false, "empty index");
    let missing = sb.init("no-index");
    check(&missing, false, "missing index");
}

/// The index stamp changes when git rewrites the index, and only then (ADR-104).
#[cfg(unix)]
#[test]
fn index_stamp_follows_index_rewrites() {
    let sb = Sandbox::new();
    let repo = sb.init("stamp");
    let (git, info) = sb.discover(&repo);
    assert_eq!(git.index_stamp(&info), None, "no index file yet");
    sb.write(&repo, "a", b"a\n");
    sb.git(&repo, &["add", "a"]);
    sb.commit(&repo, "a");
    let before = git.index_stamp(&info);
    assert!(before.is_some());
    assert_eq!(git.index_stamp(&info), before, "a read changes nothing");
    let _listed = git.tracked_with_stages().unwrap();
    assert_eq!(git.index_stamp(&info), before, "ls-files does not write");
    sb.write(&repo, "b", b"b\n");
    sb.git(&repo, &["add", "b"]);
    assert_ne!(git.index_stamp(&info), before, "git add rewrote the index");
}

/// ADR-154: the intent-to-add proof holds exactly on a SHA-1 index whose every entry the strict reader parses
/// without the intent-to-add flag, and agrees with `git diff --diff-filter=A` wherever it holds: v2, a v3 index with
/// skip-worktree, v4; refused with an intent-to-add entry (also when its file is deleted, where git lists nothing),
/// for a split index, a missing index, a config that names an object format and an index above 16 384 entries.
#[test]
fn intent_to_add_proof_holds_only_without_such_entries() {
    let sb = Sandbox::new();
    let repo = sb.init("ita");
    let (git, info) = sb.discover(&repo);
    assert!(!git.index_without_intent_to_add(&info), "no index file yet");
    sb.write(&repo, "a", b"a\n");
    sb.write(&repo, "b", b"b\n");
    sb.git(&repo, &["add", "a", "b"]);
    sb.commit(&repo, "ab");
    let ita_listed = |git: &Git| !git.intent_to_add().unwrap().is_empty();
    assert!(git.index_without_intent_to_add(&info), "v2");
    assert!(!ita_listed(&git));
    sb.git(&repo, &["update-index", "--skip-worktree", "b"]);
    assert!(git.index_without_intent_to_add(&info), "v3, skip-worktree");
    sb.git(&repo, &["update-index", "--no-skip-worktree", "b"]);
    sb.git(&repo, &["update-index", "--index-version", "4"]);
    assert!(git.index_without_intent_to_add(&info), "v4");
    assert!(!ita_listed(&git));
    sb.git(&repo, &["update-index", "--index-version", "2"]);
    sb.write(&repo, "n", b"n\n");
    sb.git(&repo, &["add", "-N", "n"]);
    // git >= 2.19 lists the entry as added; 2.18 as modified (diff.ita-invisible-in-index): either way the entry
    // refuses the proof
    assert!(
        !git.index_without_intent_to_add(&info),
        "intent-to-add entry"
    );
    std::fs::remove_file(repo.join("n")).unwrap();
    assert!(
        !git.index_without_intent_to_add(&info),
        "the entry decides, not the worktree"
    );
    sb.git(&repo, &["rm", "-q", "--cached", "n"]);
    assert!(git.index_without_intent_to_add(&info));
    sb.git(&repo, &["update-index", "--split-index"]);
    assert!(!git.index_without_intent_to_add(&info), "split index");
    sb.git(&repo, &["update-index", "--no-split-index"]);
    assert!(git.index_without_intent_to_add(&info));
    let config = info.common_dir.join("config");
    let saved = std::fs::read(&config).unwrap();
    let mut named = saved.clone();
    named.extend_from_slice(b"[extensions]\n\tObjectFormat = sha1\n");
    std::fs::write(&config, &named).unwrap();
    assert!(
        !git.index_without_intent_to_add(&info),
        "an object format named"
    );
    std::fs::write(&config, &saved).unwrap();
    assert!(git.index_without_intent_to_add(&info));
    // the header's count above the bound refuses before the entries are read
    let index = std::fs::read(&info.index_file).unwrap();
    let mut big = index.clone();
    big.splice(8..12, 16_385_u32.to_be_bytes());
    std::fs::write(&info.index_file, &big).unwrap();
    assert!(!git.index_without_intent_to_add(&info), "too many entries");
    std::fs::write(&info.index_file, &index).unwrap();
    assert!(git.index_without_intent_to_add(&info));
}
