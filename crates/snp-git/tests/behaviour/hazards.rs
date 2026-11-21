//! The data-safety hazards of 02 G.0 item 5, each a regression test that leaves the user repository unchanged:
//! (a) `GIT_DIFF_OPTS` (D-DATA-11), (b) intent-to-add + rename (D-DATA-12), (c) `GIT_DIR` during autoupdate
//! (D-SEC-05), (d) SHA-256 object formats (D-SEC-06).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use snp_core::env::EnvPatch;
use snp_core::failure::Failure;
use snp_git::{CloneMode, DiffIndex, Git, OwnedRepo};
use snp_proc::spawn::Group;

use crate::common::{Sandbox, zl};

/// Worktree bytes (excluding `.git`), the porcelain status and the index of `repo`.
fn snapshot(sb: &Sandbox, repo: &Path) -> (BTreeMap<PathBuf, Vec<u8>>, Vec<u8>, Vec<u8>) {
    fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.file_name() == Some(OsStr::new(".git")) {
                continue;
            }
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                out.insert(
                    p.strip_prefix(base).unwrap().to_path_buf(),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(repo, repo, &mut files);
    let status = sb.git(
        repo,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    );
    let index = sb.git(repo, &["ls-files", "-s", "-z"]);
    (files, status, index)
}

/// pre-commit's `staged_files_only` (`staged_files_only.py:35-113`) through snp-git's calls, `hook` running while
/// the unstaged changes are stashed.
fn stash_cycle(git: &Git, patch: &Path, hook: impl FnOnce()) -> Result<(), Failure> {
    let ita = git.intent_to_add()?;
    if !ita.is_empty() {
        git.rm_cached(&ita, Group::Inherit)?;
    }
    let tree = git.write_tree()?;
    let mut f = std::fs::File::create(patch).map_err(|e| Failure::from_io(&e, None, None))?;
    let result = git.diff_index_patch(&tree, &mut f)?;
    drop(f);
    if let DiffIndex::Patch { .. } = result {
        git.checkout_paths(&git.patch_paths(patch)?, true, Group::Inherit)?;
        hook();
        if !git.apply(patch, false, Group::Inherit)?
            && git.apply(patch, true, Group::Inherit).is_err()
        {
            git.checkout_paths(&git.rollback_paths(patch)?, true, Group::Inherit)?;
            if !git.apply(patch, false, Group::Inherit)? {
                git.apply(patch, true, Group::Inherit)?;
            }
        }
    } else {
        hook();
    }
    if !ita.is_empty() {
        git.add_intent_to_add(&ita, Group::Inherit)?;
    }
    Ok(())
}

#[test]
fn hazard_a_git_diff_opts_cannot_break_the_restore() {
    let sb = Sandbox::new();
    let repo = sb.init("repo");
    let lines: String = (1..=20).fold(String::new(), |s, i| s + &format!("line {i}\n"));
    sb.write(&repo, "a.txt", lines.as_bytes());
    sb.git(&repo, &["add", "a.txt"]);
    sb.commit(&repo, "base");
    sb.write(
        &repo,
        "a.txt",
        lines.replace("line 10\n", "line ten\n").as_bytes(),
    );
    sb.write(&repo, "b.txt", b"staged\n");
    sb.git(&repo, &["add", "b.txt"]);
    let before = snapshot(&sb, &repo);

    let (git, _) = sb.discover(&repo);
    let git = git.with_env(
        sb.env
            .patched(&EnvPatch::default().set("GIT_DIFF_OPTS", "--unified=0")),
    );
    let patch = sb.path("patch");
    stash_cycle(&git, &patch, || {}).unwrap();
    assert_eq!(snapshot(&sb, &repo), before);
    let text = String::from_utf8(std::fs::read(&patch).unwrap()).unwrap();
    assert!(
        text.contains("@@ -7,7 +7,7 @@"),
        "the stash patch keeps its context lines:\n{text}"
    );
    // the env of every other call is untouched: the fingerprint diff still sees GIT_DIFF_OPTS (02 G.7 row 2)
    let zero_context = sb.git_with(
        &repo,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules",
        ],
        &[("GIT_DIFF_OPTS", "--unified=0")],
    );
    assert!(String::from_utf8_lossy(&zero_context).contains("@@ -10 +10 @@"));
    assert_eq!(
        git.diff_fingerprint().unwrap().digest,
        *blake3::hash(&zero_context).as_bytes()
    );
}

#[test]
fn hazard_b_an_intent_to_add_rename_pair_is_restored() {
    let sb = Sandbox::new();
    let repo = sb.init("repo");
    sb.write(&repo, "old.txt", b"same content\nfor rename detection\n");
    sb.write(&repo, "other.txt", b"1\n");
    sb.git(&repo, &["add", "."]);
    sb.commit(&repo, "base");
    std::fs::rename(repo.join("old.txt"), repo.join("new.txt")).unwrap();
    sb.git(&repo, &["add", "-N", "new.txt"]);
    sb.write(&repo, "other.txt", b"2\n");
    sb.git(&repo, &["add", "other.txt"]);
    let before = snapshot(&sb, &repo);

    let (git, _) = sb.discover(&repo);
    // pre-commit's query misses it: the pair shows as a rename
    let pc = sb.git(
        &repo,
        &[
            "diff",
            "--no-ext-diff",
            "--ignore-submodules",
            "--diff-filter=A",
            "--name-only",
            "-z",
        ],
    );
    assert_eq!(pc, b"");
    assert_eq!(zl(&git.intent_to_add().unwrap()), vec![b"new.txt".to_vec()]);
    let patch = sb.path("patch");
    stash_cycle(&git, &patch, || {}).unwrap();
    assert_eq!(
        snapshot(&sb, &repo),
        before,
        "the deletion of old.txt survives"
    );
}

#[test]
fn hazard_c_git_dir_never_redirects_autoupdate_into_the_user_repo() {
    let sb = Sandbox::new();
    let hooks = sb.init("hooks");
    sb.write(
        &hooks,
        ".pre-commit-hooks.yaml",
        b"-   id: x\n    name: x\n    entry: x\n    language: system\n",
    );
    sb.git(&hooks, &["add", "."]);
    sb.commit(&hooks, "v1");
    sb.git(&hooks, &["tag", "v1.0.0"]);
    let user = sb.init("user");
    sb.write(&user, "f", b"f");
    sb.git(&user, &["add", "f"]);
    sb.commit(&user, "user");
    let config_before = std::fs::read(user.join(".git/config")).unwrap();
    let before = snapshot(&sb, &user);

    // a shell that exported GIT_DIR (dotfile setups, `git --git-dir` wrappers, hooks)
    let base = sb.env.patched(
        &EnvPatch::default()
            .set("GIT_DIR", user.join(".git"))
            .set("GIT_WORK_TREE", user.as_os_str())
            .set("GIT_INDEX_FILE", user.join(".git/index")),
    );
    let tmp = sb.path("autoupdate-tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let owned = OwnedRepo::open(sb.exe(), &base, &tmp, sb.backend());
    let info = owned
        .rev_info(hooks.to_str().unwrap(), true, false)
        .unwrap();
    assert_eq!(info.rev, "v1.0.0");
    assert!(
        tmp.join(".pre-commit-hooks.yaml").is_file(),
        "the manifest is checked out for the caller"
    );
    assert_eq!(
        std::fs::read(user.join(".git/config")).unwrap(),
        config_before
    );
    assert_eq!(snapshot(&sb, &user), before);
}

#[test]
fn hazard_d_object_format_mismatch_is_retried_once() {
    let sb = Sandbox::new();
    let up = sb.path("sha256-upstream");
    std::fs::create_dir_all(&up).unwrap();
    sb.git(&up, &["init", "-q", "--object-format=sha256", "."]);
    sb.write(&up, "f", b"f");
    sb.git(&up, &["add", "f"]);
    sb.commit(&up, "c");
    let rev = sb.head(&up);
    assert_eq!(rev.len(), 64);
    let before = snapshot(&sb, &up);

    let dest = sb.path("clone");
    std::fs::create_dir_all(&dest).unwrap();
    OwnedRepo::clone_rev(
        sb.exe(),
        &sb.env,
        &dest,
        up.to_str().unwrap(),
        &rev,
        CloneMode::Foreground,
        sb.backend(),
    )
    .unwrap();
    assert_eq!(sb.head(&dest), rev);
    assert_eq!(
        sb.git_text(&dest, &["rev-parse", "--show-object-format"]),
        "sha256"
    );
    assert_eq!(snapshot(&sb, &up), before);
}

/// ADR-151: `git apply --numstat` fed the patch on stdin while it is written lists exactly what the file-based
/// listing of D-DATA-19 lists, byte for byte and in order, for a patch with a text rewrite, a binary change, a
/// deletion, a mode change, an intent-to-add file and a name git quotes, whatever the write sizes; a patch git
/// cannot parse is no answer (the caller then runs the file-based listing, whose error is the canonical one).
#[test]
fn the_fed_numstat_lists_exactly_what_the_file_based_one_lists() {
    let sb = Sandbox::new();
    let repo = sb.init("repo");
    let lines: String = (1..=4000).fold(String::new(), |s, i| s + &format!("line {i}\n"));
    let quoted = if cfg!(windows) {
        "sp ace and\u{e9}.txt"
    } else {
        "sp ace\tand\"quote\u{e9}.txt"
    };
    sb.write(&repo, "big.txt", lines.as_bytes());
    sb.write(&repo, "bin.dat", &[0_u8, 1, 2, 3, 255, 0, 7]);
    sb.write(&repo, "gone.txt", b"bye\n");
    sb.write(&repo, "mode.sh", b"echo\n");
    sb.write(&repo, quoted, b"a\n");
    sb.git(&repo, &["add", "."]);
    sb.commit(&repo, "base");
    sb.write(&repo, "big.txt", lines.replace("line", "LINE").as_bytes());
    sb.write(&repo, "bin.dat", &[9_u8, 0, 8, 0, 7]);
    std::fs::remove_file(repo.join("gone.txt")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(repo.join("mode.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    sb.write(&repo, quoted, b"b\n");
    sb.write(&repo, "new.txt", b"intent\n");
    sb.git(&repo, &["add", "-N", "new.txt"]);

    let (git, _) = sb.discover(&repo);
    let tree = git.write_tree().unwrap();
    let mut patch_bytes = Vec::new();
    assert!(matches!(
        git.diff_index_patch(&tree, &mut patch_bytes).unwrap(),
        DiffIndex::Patch { .. }
    ));
    let patch = sb.path("patch");
    std::fs::write(&patch, &patch_bytes).unwrap();
    let from_file = git.patch_paths(&patch).unwrap();
    assert!(zl(&from_file).len() >= 5, "{:?}", zl(&from_file));
    // the child sees one byte stream whatever the chunking (one byte at a time for the first 300 bytes)
    for step in [1_usize, 4093, 64 << 10, usize::MAX] {
        let mut feed = git.patch_paths_feed().expect("the numstat child starts");
        let mut rest = patch_bytes.as_slice();
        let mut writes = 0;
        while !rest.is_empty() {
            let n = if step == 1 && writes >= 300 {
                rest.len()
            } else {
                step.min(rest.len())
            };
            let (now, later) = rest.split_at_checked(n).unwrap();
            std::io::Write::write_all(&mut feed, now).unwrap();
            rest = later;
            writes += 1;
        }
        let fed = feed.finish().expect("the fed listing answers");
        assert_eq!(zl(&fed), zl(&from_file), "step {step}");
    }
    // a corrupt patch: no answer from the fed run, an error from the file-based one
    let corrupt = b"diff --git a/x b/x\n@@ garbage\n";
    let mut feed = git.patch_paths_feed().unwrap();
    std::io::Write::write_all(&mut feed, corrupt).unwrap();
    assert!(feed.finish().is_none());
    std::fs::write(&patch, corrupt).unwrap();
    assert!(git.patch_paths(&patch).is_err());
    // an empty input is refused either way ("No valid patches in input")
    let feed = git.patch_paths_feed().unwrap();
    assert!(feed.finish().is_none());
    std::fs::write(&patch, b"").unwrap();
    assert!(git.patch_paths(&patch).is_err());
}
