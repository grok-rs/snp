//! Recovery (ARCHITECTURE §4.6): lock-based liveness, the git-dir pointer with another store, the exact notice, stale
//! journal cleanup, and the next run's stderr notice.

use std::path::{Path, PathBuf};

use snp_core::path::RepoPath;
use snp_stash::{Stashed, Unrestored};

use crate::common::{Reply, Sandbox, Spy};

/// `recovery_scan`, retried while another test thread's freshly forked child may still hold a copy of a just-released
/// lock descriptor (flock locks belong to the open file description; the copy closes at its exec).
fn scan(git_dir: &Path, patches: &Path, root: &Path) -> Vec<Unrestored> {
    for _ in 0..200 {
        let found = snp_stash::recovery_scan(git_dir, patches, root);
        if !found.is_empty() {
            return found;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Vec::new()
}

/// A K+ failure leaves an orphaned `failed` journal; returns the patch.
fn orphan(sb: &Sandbox, repo: &Path) -> PathBuf {
    orphan_of(sb, repo, b"herp\n")
}

/// [`orphan`] with the unstaged contents of `foo`.
fn orphan_of(sb: &Sandbox, repo: &Path, foo: &[u8]) -> PathBuf {
    sb.write(repo, "foo", foo);
    orphan_now(sb, repo)
}

/// A K+ failure of a run over the worktree as it is (the caller made the unstaged changes); returns the patch.
fn orphan_now(sb: &Sandbox, repo: &Path) -> PathBuf {
    let spy = Spy::new();
    spy.on(&["apply"], None, Reply::Exit(1, Vec::new(), Vec::new()));
    spy.on(
        &["-c", "core.autocrlf=false", "apply"],
        None,
        Reply::Exit(1, Vec::new(), Vec::new()),
    );
    let (git, info) = sb.discover_with(repo, spy);
    let (cx, _, _) = sb.ctx_with(git, info);
    let g = snp_stash::enter(cx).unwrap();
    let Stashed::Patch(p) = g.stashed().clone() else {
        panic!()
    };
    assert!(g.restore().is_err());
    p
}

#[test]
fn hint_texts() {
    let d = tempfile::tempdir().unwrap();
    let patch = d.path().join("patch1-2");
    std::fs::write(
        &patch,
        b"diff --git a/foo b/foo\nindex 1..2 100644\n--- a/foo\n+++ b/foo\n",
    )
    .unwrap();
    let u = Unrestored {
        patch: patch.clone(),
        tree: Some("abc".to_owned()),
        worktree: PathBuf::from("/r"),
        pid: 2,
        state: Box::from("stashed"),
        staged_fixed: vec![RepoPath::new(b"x*").unwrap()],
    };
    // A plain, worktree-only apply; the staged paths are reset (literal pathspecs); the fallback names the patch's
    // paths.
    assert_eq!(
        u.hint(),
        format!(
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in {0} \
             (index tree abc). Restore with: git apply --whitespace=nowarn {2} && git --literal-pathspecs reset -q \
             abc -- 'x*'{1}git --literal-pathspecs restore --source=abc --worktree -- foo, then run the command again",
            patch.display(),
            fallback("abc", &patch),
            snp_pycompat::shlex::quote(&patch.display().to_string())
        )
    );
    // An unreadable patch: the fallback covers the whole worktree.
    let u = Unrestored {
        patch: PathBuf::from("/s/patches/patch1-2"),
        staged_fixed: Vec::new(),
        ..u
    };
    assert_eq!(
        u.hint(),
        format!(
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
             /s/patches/patch1-2 (index tree abc). Restore with: git apply --whitespace=nowarn /s/patches/patch1-2{}git \
             restore --source=abc --worktree -- :/, then run the command again",
            fallback("abc", Path::new("/s/patches/patch1-2"))
        )
    );
}

/// The text between a hint's command and the restore command of its fallback.
pub(crate) fn fallback(tree: &str, patch: &Path) -> String {
    format!(
        "; if git apply reports that the patch does not apply, those files changed after the stash (hook fixes or \
         later edits): keep a copy with git diff --binary {tree} > {}, put them back with ",
        snp_pycompat::shlex::quote(&format!("{}.now", patch.display()))
    )
}

#[test]
fn next_run_prints_the_notice_and_keeps_the_orphan() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let tree = sb.git_text(&repo, &["write-tree"]);
    let p = orphan(&sb, &repo);
    let found = scan(&repo.join(".git"), &sb.patches(), &repo);
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].patch.clone(), found[0].tree.clone(), found[0].pid),
        (p.clone(), Some(tree.clone()), std::process::id())
    );
    assert_eq!(found[0].worktree, repo);
    // The next stash prints the notice on stderr (never stdout) and does not touch the orphan.
    sb.git(&repo, &["checkout", "--", "."]);
    let (cx, out, err) = sb.ctx(&repo);
    let g = snp_stash::enter(cx).unwrap();
    g.restore().unwrap();
    assert_eq!(out.text(), "");
    assert_eq!(
        err.text(),
        format!(
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in {0} \
             (index tree {tree}). Restore with: git apply --whitespace=nowarn {2}{1}git --literal-pathspecs restore \
             --source={tree} --worktree -- foo, then run the command again\nsnp: this notice repeats until those \
             changes are back in the worktree; to dismiss it and keep the patch, run: snp doctor --fix --stash\n",
            p.display(),
            fallback(&tree, &p),
            snp_pycompat::shlex::quote(&p.display().to_string())
        )
    );
    assert!(p.is_file());
    assert_eq!(
        snp_stash::find_unrestored(&repo.join(".git"), &sb.patches(), &repo).len(),
        1
    );
}

#[test]
fn the_pointer_finds_the_journal_from_another_store() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let p = orphan(&sb, &repo);
    let other = sb.path("other-store/patches");
    std::fs::create_dir_all(&other).unwrap();
    let found = scan(&repo.join(".git"), &other, &repo);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].patch, p);
    assert_eq!(&*found[0].state, "failed");
    // Without the pointer only the original store sees it.
    std::fs::remove_file(repo.join(".git/snp-stash.journal")).unwrap();
    assert!(snp_stash::recovery_scan(&repo.join(".git"), &other, &repo).is_empty());
    assert_eq!(scan(&repo.join(".git"), &sb.patches(), &repo).len(), 1);
    // Another worktree sharing the store does not see it.
    assert!(
        snp_stash::find_unrestored(&repo.join(".git"), &sb.patches(), &sb.path("elsewhere"))
            .is_empty()
    );
}

#[test]
fn a_held_lock_hides_everything() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    orphan(&sb, &repo);
    sb.git(&repo, &["checkout", "--", "."]);
    let (cx, _, _) = sb.ctx(&repo);
    let g = snp_stash::enter(cx).unwrap();
    assert!(snp_stash::recovery_scan(&repo.join(".git"), &sb.patches(), &repo).is_empty());
    g.restore().unwrap();
    assert_eq!(scan(&repo.join(".git"), &sb.patches(), &repo).len(), 1);
}

#[test]
fn a_second_orphan_gets_its_own_pointer() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let p1 = orphan(&sb, &repo);
    sb.git(&repo, &["checkout", "--", "."]);
    // Other unstaged changes (the same ones would be back in the worktree: the second run would retire the first).
    let p2 = orphan_of(&sb, &repo, b"derp\n");
    assert!(repo.join(".git/snp-stash-1.journal").is_file());
    let other = sb.path("other/patches");
    let mut found: Vec<PathBuf> = scan(&repo.join(".git"), &other, &repo)
        .into_iter()
        .map(|u| u.patch)
        .collect();
    found.sort();
    let mut want = vec![p1, p2];
    want.sort();
    assert_eq!(found, want);
}

#[test]
fn stale_files_are_cleaned_and_garbage_is_ignored() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    std::fs::create_dir_all(sb.patches()).unwrap();
    let patch = sb.patches().join("patch1-1");
    std::fs::write(&patch, b"x").unwrap();
    let journal = |state: &str| {
        format!(
            "{{\"v\":1,\"state\":\"{state}\",\"worktree\":{},\"patch\":{},\"tree\":null}}",
            // JSON strings: a Windows path's `\` is escaped
            serde_json::to_string(&repo).unwrap(),
            serde_json::to_string(&patch).unwrap()
        )
    };
    let restored = sb.patches().join("run-1-aa.snp-journal");
    let started = sb.patches().join("run-2-bb.snp-journal");
    let garbage = sb.patches().join("run-3-cc.snp-journal");
    std::fs::write(&restored, journal("restored")).unwrap();
    std::fs::write(&started, journal("started")).unwrap();
    std::fs::write(&garbage, b"\xff{not json").unwrap();
    std::fs::write(
        repo.join(".git/snp-stash.journal"),
        format!(
            "{{\"v\":1,\"journal\":{},\"state\":\"restored\"}}",
            serde_json::to_string(&restored).unwrap()
        ),
    )
    .unwrap();
    assert!(snp_stash::find_unrestored(&repo.join(".git"), &sb.patches(), &repo).is_empty());
    let (cx, _, err) = sb.ctx(&repo);
    snp_stash::enter(cx).unwrap().restore().unwrap();
    assert_eq!(err.text(), "");
    assert!(
        !restored.exists() && !started.exists() && !repo.join(".git/snp-stash.journal").exists()
    );
    // Foreign or unparsable files are never deleted.
    assert!(garbage.exists() && patch.exists());
}

#[test]
fn a_missing_patch_is_not_reported() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let p = orphan(&sb, &repo);
    std::fs::remove_file(&p).unwrap();
    assert!(snp_stash::find_unrestored(&repo.join(".git"), &sb.patches(), &repo).is_empty());
}

/// ADR-083: once the patch's changes are back in the worktree, the next stash retires the orphan's journal with a
/// one-time notice (the patch is kept); the stash after that is silent.
#[test]
fn an_applied_orphan_is_retired_once() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let p = orphan(&sb, &repo);
    // The K+ failure left the worktree at the staged content; put the unstaged change back by hand.
    sb.git(&repo, &["checkout", "--", "."]);
    sb.git(&repo, &["apply", p.to_str().unwrap()]);
    let (cx, _, err) = sb.ctx(&repo);
    let g = snp_stash::enter(cx).unwrap();
    g.restore().unwrap();
    assert_eq!(
        err.text(),
        format!(
            "snp: the unstaged changes saved in {} are back in the worktree; the recovery notice of that interrupted \
             run is retired\n",
            p.display()
        )
    );
    assert!(p.is_file());
    assert!(snp_stash::find_unrestored(&repo.join(".git"), &sb.patches(), &repo).is_empty());
    assert!(!repo.join(".git/snp-stash.journal").exists());
    let (cx, _, err) = sb.ctx(&repo);
    snp_stash::enter(cx).unwrap().restore().unwrap();
    assert_eq!(err.text(), "");
}

/// ADR-083: `dismiss_unrestored` retires an orphan whose changes are not back, keeps the patch (with a fresh
/// modification time, so gc's 30 days count from the dismissal) and later stashes are silent.
#[test]
fn dismissal_keeps_the_patch_and_silences_the_notice() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let p = orphan(&sb, &repo);
    sb.git(&repo, &["checkout", "--", "."]);
    let old = std::time::SystemTime::now() - std::time::Duration::from_hours(40 * 24);
    std::fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let (git, _info) = sb.discover(&repo);
    let gone = snp_stash::dismiss_unrestored(&git, &repo.join(".git"), &sb.patches(), &repo);
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].patch, p);
    assert!(!gone[0].present);
    assert!(p.is_file());
    let age = std::fs::metadata(&p).unwrap().modified().unwrap();
    assert!(age > old + std::time::Duration::from_hours(24));
    assert!(snp_stash::find_unrestored(&repo.join(".git"), &sb.patches(), &repo).is_empty());
    let (cx, _, err) = sb.ctx(&repo);
    snp_stash::enter(cx).unwrap().restore().unwrap();
    assert_eq!(err.text(), "");
}

/// Runs the command of a notice (after `Restore with: `, before its fallback) with `sh -c`: (success, stderr).
fn run_notice(sb: &Sandbox, repo: &Path, notice: &str) -> (bool, String) {
    let cmd = notice.split("Restore with: ").nth(1).unwrap();
    let cmd = cmd.split("; if git apply reports").next().unwrap();
    let cmd = cmd.lines().next().unwrap();
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(repo)
        .env_clear()
        .envs(sb.env.iter())
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The stderr of one recovery pass (a clean stash and restore).
fn pass(sb: &Sandbox, repo: &Path) -> String {
    let (cx, _, err) = sb.ctx(repo);
    snp_stash::enter(cx).unwrap().restore().unwrap();
    err.text()
}

/// ADR-084: work done after the kill in a file the patch touches is never overwritten by the printed command: it
/// either applies around the new work or refuses and changes nothing; the fallback's copy keeps it too.
#[test]
fn the_notice_never_overwrites_work_done_after_the_kill() {
    for (at_top, applies) in [(true, true), (false, false)] {
        let sb = Sandbox::new();
        let repo = sb.foo_staged("r");
        let staged = sb.read(&repo, "foo");
        let p = orphan_of(&sb, &repo, &[staged.as_slice(), b"USER\n"].concat());
        // The K+ rollback left the staged contents; the user goes on working in the same file: far from the patch's
        // hunk (the apply goes around it), or right where it applies (the apply refuses).
        sb.git(&repo, &["checkout", "--", "."]);
        let after_kill = if at_top {
            [b"NEW WORK AFTER KILL\n".as_slice(), &staged].concat()
        } else {
            [staged.as_slice(), b"NEW WORK AFTER KILL\n"].concat()
        };
        sb.write(&repo, "foo", &after_kill);
        let notice = u_hint(&sb, &repo, &p);
        let (ok, stderr) = run_notice(&sb, &repo, &notice);
        let now = sb.read(&repo, "foo");
        let now = String::from_utf8_lossy(&now);
        assert!(now.contains("NEW WORK AFTER KILL"), "{notice}\n{now}");
        assert_eq!(ok, applies, "{stderr}");
        if ok {
            assert!(now.contains("USER"));
        } else {
            // Refused: nothing changed. The fallback keeps a copy before it restores the file.
            assert_eq!(sb.read(&repo, "foo"), after_kill);
            assert!(notice.contains("git diff --binary "));
        }
    }
}

/// The notice of the single orphan of `repo` with patch `p`.
fn u_hint(sb: &Sandbox, repo: &Path, p: &Path) -> String {
    let found = snp_stash::find_unrestored(&repo.join(".git"), &sb.patches(), repo);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].patch, p);
    found[0].hint()
}

/// ADR-084: the long form (more than 64 paths) restores the pre-run state when run exactly as printed (it used to
/// fail with `does not match index` and overwrite every tracked file first).
#[test]
fn the_long_notice_restores_more_than_64_paths() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    for i in 0..70 {
        sb.write(&repo, &format!("f{i}"), b"base\n");
    }
    sb.git(&repo, &["add", "."]);
    sb.git(&repo, &["commit", "-qm", "files"]);
    sb.write(&repo, "foo", b"staged\n");
    sb.git(&repo, &["add", "foo"]);
    for i in 0..70 {
        sb.write(&repo, &format!("f{i}"), format!("user {i}\n").as_bytes());
    }
    let index_before = sb.index(&repo);
    let diff_before = sb.unstaged_diff(&repo);
    let p = orphan_now(&sb, &repo);
    sb.git(&repo, &["checkout", "--", "."]);
    let notice = u_hint(&sb, &repo, &p);
    assert!(notice.contains("-- :/"), "{notice}");
    let (ok, stderr) = run_notice(&sb, &repo, &notice);
    assert!(ok, "{stderr}");
    assert_eq!(sb.index(&repo), index_before);
    assert_eq!(sb.unstaged_diff(&repo), diff_before);
    // And the next pass retires it.
    assert!(pass(&sb, &repo).contains("is retired"));
}

/// ADR-084: a lost mode change (`chmod +x`) is not "back in the worktree": git's reverse check only warns about the
/// mode, so the orphan was retired and the change lost without a word.
#[cfg(unix)]
#[test]
fn a_lost_mode_change_is_not_retired() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let foo = repo.join("foo");
    std::fs::set_permissions(&foo, std::fs::Permissions::from_mode(0o755)).unwrap();
    let p = orphan_now(&sb, &repo);
    sb.git(&repo, &["checkout", "--", "."]);
    assert_eq!(
        std::fs::metadata(&foo).unwrap().permissions().mode() & 0o111,
        0
    );
    let err = pass(&sb, &repo);
    assert!(err.contains("Restore with: "), "{err}");
    assert!(!err.contains("retired"), "{err}");
    // The printed command restores the mode, and then the orphan is retired.
    let (ok, stderr) = run_notice(&sb, &repo, &u_hint(&sb, &repo, &p));
    assert!(ok, "{stderr}");
    assert_ne!(
        std::fs::metadata(&foo).unwrap().permissions().mode() & 0o100,
        0
    );
    assert!(pass(&sb, &repo).contains("is retired"));
}

/// ADR-084: with `apply.whitespace=fix` git's reverse check matched a missing trailing-whitespace change.
#[test]
fn a_lost_whitespace_change_is_not_retired_under_apply_whitespace_fix() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let staged = sb.read(&repo, "foo");
    let text = String::from_utf8(staged).unwrap();
    let spaced = text.replacen('\n', "   \n", 1);
    orphan_of(&sb, &repo, spaced.as_bytes());
    sb.git(&repo, &["checkout", "--", "."]);
    sb.git(&repo, &["config", "apply.whitespace", "fix"]);
    sb.git(&repo, &["config", "apply.ignoreWhitespace", "change"]);
    let err = pass(&sb, &repo);
    assert!(err.contains("Restore with: "), "{err}");
    assert!(!err.contains("retired"), "{err}");
}

/// ADR-084: a long intent-to-add list outlives the journal: the notice of the retired run names a kept list file,
/// and its command re-adds every entry.
#[test]
fn a_long_intent_to_add_list_survives_the_retirement() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let names: Vec<String> = (0..70).map(|i| format!("new_{i}.txt")).collect();
    for n in &names {
        sb.write(&repo, n, b"n\n");
    }
    let mut add = vec!["add".to_owned(), "-N".to_owned(), "--".to_owned()];
    add.extend(names.iter().cloned());
    sb.git(&repo, &add);
    let staged = sb.read(&repo, "foo");
    let p = orphan_of(&sb, &repo, &[staged.as_slice(), b"USER\n"].concat());
    sb.git(&repo, &["checkout", "--", "foo"]);
    let (ok, stderr) = run_notice(&sb, &repo, &u_hint(&sb, &repo, &p));
    assert!(ok, "{stderr}");
    let err = pass(&sb, &repo);
    let ita = err
        .lines()
        .find(|l| l.contains("re-added its intent-to-add files"))
        .unwrap_or_else(|| panic!("{err}"));
    // the shell word may be quoted (a Windows path is) and the notice may continue after it
    let file = err
        .split("--pathspec-from-file=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .map_or_else(
            || panic!("{err}"),
            |w| w.trim_matches(|c| c == '\'' || c == ';'),
        );
    assert!(Path::new(file).is_file(), "{ita}");
    // The journal is gone; running the notice re-adds every entry.
    assert!(snp_stash::find_unrestored(&repo.join(".git"), &sb.patches(), &repo).is_empty());
    let (ok, stderr) = run_notice(&sb, &repo, ita);
    assert!(ok, "{stderr}");
    let listed = String::from_utf8(sb.git(&repo, &["ls-files", "--", "new_*"])).unwrap();
    assert_eq!(listed.lines().count(), 70);
}
