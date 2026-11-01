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
    sb.write(repo, "foo", b"herp\n");
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
    // The patch's paths are checked out and reset; the staged paths are reset too (literal pathspecs).
    assert_eq!(
        u.hint(),
        format!(
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in {0} \
             (index tree abc). Restore with: git --literal-pathspecs checkout abc -- foo && git apply --3way {0} && \
             git --literal-pathspecs reset -q abc -- foo 'x*'",
            patch.display()
        )
    );
    // An unreadable patch: the whole-index form.
    let u = Unrestored {
        patch: PathBuf::from("/s/patches/patch1-2"),
        ..u
    };
    assert_eq!(
        u.hint(),
        "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
         /s/patches/patch1-2 (index tree abc). Restore with: git read-tree abc && git checkout-index -f -a && git \
         apply --3way /s/patches/patch1-2 && git read-tree abc"
    );
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
             (index tree {tree}). Restore with: git --literal-pathspecs checkout {tree} -- foo && git apply --3way \
             {0} && git --literal-pathspecs reset -q {tree} -- foo\nsnp: this notice repeats until those changes are \
             back in the worktree; to dismiss it and keep the patch, run: snp doctor --fix --stash\n",
            p.display()
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
    let p2 = orphan(&sb, &repo);
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
            "{{\"v\":1,\"state\":\"{state}\",\"worktree\":\"{}\",\"patch\":\"{}\",\"tree\":null}}",
            repo.display(),
            patch.display()
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
            "{{\"v\":1,\"journal\":\"{}\",\"state\":\"restored\"}}",
            restored.display()
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
