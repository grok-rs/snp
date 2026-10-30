//! The guard beyond the catalog: worktree lock and nested token, journal and pointer ordering, re-verification,
//! patch naming, git < 2.19, the tty stop, `SNP_STASH_VERIFY`, the panic-path restore and the 02 G.0 hazards.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use snp_core::env::EnvPatch;
use snp_stash::{StashGuard, Stashed};

use crate::common::{FOO, FixedClock, Reply, Sandbox, Spy, exc};

fn pointer(repo: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(repo.join(".git/snp-stash.journal")).unwrap()).unwrap()
}

fn patch_of(g: &StashGuard) -> std::path::PathBuf {
    match g.stashed() {
        Stashed::Patch(p) => p.clone(),
        other => panic!("no patch: {other:?}"),
    }
}

#[test]
fn concurrent_second_run_waits_once_then_times_out() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    sb.write(&repo, "foo", b"herp\n");
    let (cx, _, _) = sb.ctx(&repo);
    let first = snp_stash::enter(cx).unwrap();
    let (mut cx2, out2, err2) = sb.ctx(&repo);
    cx2.lock_timeout = Duration::from_millis(300);
    let e = snp_stash::enter(cx2).unwrap_err();
    let (ty, text, _) = exc(&e);
    assert_eq!((ty.as_str(), e.exit_code()), ("FatalError", 1));
    assert_eq!(
        text,
        format!(
            "snp: another snp run in this worktree still holds {} after 0 s; set SNP_STASH_LOCK_TIMEOUT (seconds, \
             at most 3600) to wait longer",
            repo.join(".git/snp-stash.lock").display()
        )
    );
    assert_eq!(
        err2.text(),
        "snp: waiting for another snp run in this worktree\u{2026}\n"
    );
    assert_eq!(out2.text(), "");
    // The first run is unaffected.
    assert_eq!(sb.read(&repo, "foo"), FOO.as_bytes());
    first.restore().unwrap();
    assert_eq!(sb.read(&repo, "foo"), b"herp\n");
}

#[test]
fn second_run_proceeds_when_the_first_releases() {
    let sb = Arc::new(Sandbox::new());
    let repo = sb.foo_staged("r");
    sb.write(&repo, "foo", b"herp\n");
    let (cx, _, _) = sb.ctx(&repo);
    let first = snp_stash::enter(cx).unwrap();
    let (cx2, _, err2) = sb.ctx(&repo);
    let t = std::thread::spawn(move || {
        let g = snp_stash::enter(cx2).unwrap();
        g.restore().unwrap();
    });
    std::thread::sleep(Duration::from_millis(300));
    first.restore().unwrap();
    t.join().unwrap();
    assert_eq!(
        err2.text(),
        "snp: waiting for another snp run in this worktree\u{2026}\n"
    );
    assert_eq!(sb.read(&repo, "foo"), b"herp\n");
}

#[test]
fn cancelled_lock_wait_is_interrupted() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let (cx, _, _) = sb.ctx(&repo);
    let first = snp_stash::enter(cx).unwrap();
    let (cx2, _, _) = sb.ctx(&repo);
    cx2.cancel.cancel(snp_core::cancel::CancelReason::Interrupt);
    let e = snp_stash::enter(cx2).unwrap_err();
    assert_eq!(e.exit_code(), 130);
    first.restore().unwrap();
}

#[test]
fn nested_run_with_the_token_proceeds_while_the_lock_is_held() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    sb.write(&repo, "foo", b"herp\n");
    let (cx, _, _) = sb.ctx(&repo);
    let outer = snp_stash::enter(cx).unwrap();
    let (k, token) = outer.child_env_token().unwrap();
    assert_eq!(k, "_SNP_STASH_LOCK");
    assert_eq!(token.len(), 32);
    // The token is stored in the held lock file.
    if cfg!(unix) {
        assert_eq!(
            std::fs::read(repo.join(".git/snp-stash.lock")).unwrap(),
            token.as_encoded_bytes()
        );
    }
    // A hook running snp: same worktree, the token in its env.
    let (git, info) = sb.discover(&repo);
    let git = git.with_env(
        git.env()
            .patched(&EnvPatch::default().set(k, token.clone())),
    );
    let (mut cx2, out2, err2) = sb.ctx_with(git, info);
    cx2.lock_timeout = Duration::ZERO;
    let nested = snp_stash::enter(cx2).unwrap();
    // pre-commit's nested run sees a clean worktree.
    assert!(nested.diff_index_clean());
    assert_eq!(nested.child_env_token().unwrap().1, token);
    nested.restore().unwrap();
    assert_eq!((out2.text(), err2.text()), (String::new(), String::new()));
    // The outer run's journal survives the nested one, and no notice was printed for it.
    assert_eq!(pointer(&repo)["state"], "stashed");
    outer.restore().unwrap();
    assert_eq!(sb.read(&repo, "foo"), b"herp\n");
    // Release truncates the token.
    if cfg!(unix) {
        assert_eq!(
            std::fs::read(repo.join(".git/snp-stash.lock")).unwrap(),
            b""
        );
    }
}

#[test]
fn stale_token_in_a_released_lock_file_exempts_nobody() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let (cx, _, _) = sb.ctx(&repo);
    let g = snp_stash::enter(cx).unwrap();
    let (k, token) = g.child_env_token().unwrap();
    g.restore().unwrap();
    // A crash left the token in the (released) lock file.
    std::fs::write(repo.join(".git/snp-stash.lock"), token.as_encoded_bytes()).unwrap();
    let env_with = |sb: &Sandbox| {
        let (git, info) = sb.discover(&repo);
        let git = git.with_env(
            git.env()
                .patched(&EnvPatch::default().set(k, token.clone())),
        );
        sb.ctx_with(git, info)
    };
    // Free lock: the run takes it (fresh token) instead of skipping it.
    let (cx, _, _) = env_with(&sb);
    let mine = snp_stash::enter(cx).unwrap();
    let fresh = mine.child_env_token().unwrap().1;
    assert_ne!(fresh, token);
    // Held by `mine` with the fresh token: the stale token waits and times out.
    let (mut cx, _, err) = env_with(&sb);
    cx.lock_timeout = Duration::from_millis(100);
    assert_eq!(snp_stash::enter(cx).unwrap_err().exit_code(), 1);
    assert!(err.text().starts_with("snp: waiting"));
    mine.restore().unwrap();
}

#[test]
fn journal_and_pointer_are_durable_before_the_checkout() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    sb.write(&repo, "foo", b"herp\n");
    let seen = Arc::new(Mutex::new(None));
    let spy = Spy::new();
    {
        let seen = Arc::clone(&seen);
        let repo = repo.clone();
        let patches = sb.patches();
        spy.before(
            &["-c", "submodule.recurse=0", "checkout"],
            Some(1),
            Box::new(move |_| {
                let p = pointer(&repo);
                let journal = p["journal"].as_str().unwrap().to_owned();
                let j: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&journal).unwrap()).unwrap();
                let patch = std::path::PathBuf::from(j["patch"].as_str().unwrap());
                *seen.lock().unwrap() = Some((
                    p["state"].clone(),
                    j["state"].clone(),
                    j["tree"].clone(),
                    std::fs::read(&patch).unwrap(),
                    journal.starts_with(&*patches.to_string_lossy()),
                ));
            }),
        );
    }
    let (git, info) = sb.discover_with(&repo, spy.clone());
    let (cx, _, _) = sb.ctx_with(git, info);
    let original = sb.unstaged_diff(&repo);
    let tree = sb.git_text(&repo, &["write-tree"]);
    let g = snp_stash::enter(cx).unwrap();
    let (ps, js, jt, patch, in_store) = seen.lock().unwrap().take().unwrap();
    assert_eq!(
        (ps, js, jt),
        (
            "stashed".into(),
            "stashed".into(),
            serde_json::Value::from(tree)
        )
    );
    assert_eq!(patch, original);
    assert!(in_store);
    // The command sequence is pre-commit's (02 §7) plus the re-verification diff-index.
    let calls = spy.calls();
    let tree_arg = sb.git_text(&repo, &["write-tree"]);
    assert_eq!(
        calls,
        vec![
            "diff --no-renames --no-ext-diff --ignore-submodules --diff-filter=A --name-only -z"
                .to_owned(),
            "write-tree".to_owned(),
            format!(
                "diff-index --ignore-submodules --binary --exit-code --no-color --no-ext-diff {tree_arg} --"
            ),
            format!(
                "diff-index --ignore-submodules --binary --exit-code --no-color --no-ext-diff {tree_arg} --"
            ),
            "-c submodule.recurse=0 checkout -- .".to_owned(),
        ]
    );
    let p = patch_of(&g);
    g.restore().unwrap();
    assert_eq!(
        spy.calls()[5],
        format!("apply --whitespace=nowarn {}", p.display())
    );
    assert_eq!(spy.calls().len(), 6);
    assert!(!repo.join(".git/snp-stash.journal").exists());
    assert_eq!(
        sb.patch_files().len(),
        1,
        "only the patch is kept: {:?}",
        sb.patch_files()
    );
}

#[test]
fn autosave_between_diff_index_and_checkout_is_caught() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    sb.write(&repo, "foo", b"herp\n");
    let spy = Spy::new();
    {
        let repo = repo.clone();
        spy.before(
            &["diff-index"],
            Some(2),
            Box::new(move |_| std::fs::write(repo.join("foo"), b"herp\nautosaved\n").unwrap()),
        );
    }
    let (git, info) = sb.discover_with(&repo, spy.clone());
    let (cx, _, _) = sb.ctx_with(git, info);
    let g = snp_stash::enter(cx).unwrap();
    let p = patch_of(&g);
    assert_eq!(sb.read(&repo, "foo"), FOO.as_bytes());
    // The patch was rewritten with the autosave and verified again.
    assert_eq!(
        spy.calls()
            .iter()
            .filter(|c| c.starts_with("diff-index"))
            .count(),
        4
    );
    assert!(String::from_utf8_lossy(&std::fs::read(&p).unwrap()).contains("+autosaved"));
    g.restore().unwrap();
    assert_eq!(sb.read(&repo, "foo"), b"herp\nautosaved\n");
}

#[test]
fn files_that_keep_changing_abort_before_any_mutation() {
    let sb = Sandbox::new();
    let repo = sb.init("r");
    sb.write(&repo, "foo", FOO.as_bytes());
    sb.write(&repo, "n", b"x\n");
    sb.git(&repo, &["add", "foo"]);
    sb.git(&repo, &["add", "--intent-to-add", "n"]);
    sb.write(&repo, "foo", b"edit 0\n");
    let spy = Spy::new();
    {
        let repo = repo.clone();
        let n = Arc::new(Mutex::new(0_u32));
        spy.before(
            &["diff-index"],
            None,
            Box::new(move |_| {
                let mut n = n.lock().unwrap();
                *n += 1;
                std::fs::write(repo.join("foo"), format!("edit {n}\n")).unwrap();
            }),
        );
    }
    let (git, info) = sb.discover_with(&repo, spy.clone());
    let (cx, out, _) = sb.ctx_with(git, info);
    let e = snp_stash::enter(cx).unwrap_err();
    let (ty, text, _) = exc(&e);
    assert_eq!(
        (ty.as_str(), text.as_str(), e.exit_code()),
        (
            "FatalError",
            "snp: unstaged files kept changing while stashing; nothing was modified",
            1
        )
    );
    // 1 streamed + (verify + rewrite) × 3 + the last verify.
    assert_eq!(
        spy.calls()
            .iter()
            .filter(|c| c.starts_with("diff-index"))
            .count(),
        8
    );
    assert!(!spy.calls().iter().any(|c| c.contains("checkout")));
    assert_eq!(
        out.text(),
        "[WARNING] Unstaged intent-to-add files detected.\n"
    );
    // Nothing modified: the edit is in place, the intent-to-add entry re-added, no patch or journal left.
    assert!(
        String::from_utf8(sb.read(&repo, "foo"))
            .unwrap()
            .starts_with("edit ")
    );
    assert_eq!(sb.status_of(&repo, "n"), "A");
    assert!(sb.patch_files().is_empty(), "{:?}", sb.patch_files());
    assert!(!repo.join(".git/snp-stash.journal").exists());
}

#[test]
fn same_second_and_pid_never_overwrite_a_patch() {
    let sb = Sandbox::new();
    let mut patches = Vec::new();
    for name in ["a", "b", "c"] {
        let repo = sb.foo_staged(name);
        sb.write(&repo, "foo", name.as_bytes());
        let (mut cx, _, _) = sb.ctx(&repo);
        cx.clock = Arc::new(FixedClock(1_791_403_656));
        let g = snp_stash::enter(cx).unwrap();
        patches.push(patch_of(&g));
        g.restore().unwrap();
        assert_eq!(sb.read(&repo, "foo"), name.as_bytes());
    }
    let pid = std::process::id();
    let names: Vec<String> = patches
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            format!("patch1791403656-{pid}"),
            format!("patch1791403656-{pid}-1"),
            format!("patch1791403656-{pid}-2")
        ]
    );
    for (p, name) in patches.iter().zip(["a", "b", "c"]) {
        assert!(String::from_utf8_lossy(&std::fs::read(p).unwrap()).contains(&format!("+{name}")));
    }
}

#[test]
fn old_git_refuses_a_non_empty_stash_before_any_mutation() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    // Nothing unstaged: the run proceeds.
    let (mut cx, _, _) = sb.ctx(&repo);
    cx.old_git = true;
    let g = snp_stash::enter(cx).unwrap();
    assert!(g.diff_index_clean());
    g.restore().unwrap();
    // Unstaged changes: D-DATA-17.
    sb.write(&repo, "foo", b"herp\n");
    let spy = Spy::new();
    spy.on(
        &["--version"],
        None,
        Reply::Exit(0, b"git version 2.18.5\n".to_vec(), Vec::new()),
    );
    let (git, info) = sb.discover_with(&repo, spy.clone());
    let (mut cx, out, _) = sb.ctx_with(git, info);
    cx.old_git = true;
    let e = snp_stash::enter(cx).unwrap_err();
    let (ty, text, _) = exc(&e);
    let exe = snp_proc::resolve::normalize_cmd(&["git".into()], &sb.env, &repo).unwrap()[0].clone();
    assert_eq!((ty.as_str(), e.exit_code()), ("FatalError", 1));
    assert_eq!(
        text,
        format!(
            "git 2.18.5 cannot stash intent-to-add files safely; snp requires git 2.19.0 or newer to stash unstaged \
             changes (found {}).",
            exe.to_string_lossy()
        )
    );
    assert_eq!(out.text(), "");
    assert!(!spy.calls().iter().any(|c| c.contains("checkout")));
    assert!(sb.patch_files().is_empty());
    assert_eq!(sb.read(&repo, "foo"), b"herp\n");
}

#[test]
fn a_tty_stopped_restore_is_reported_and_the_k_plus_path_keeps_the_patch() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    sb.write(&repo, "foo", b"herp\n");
    let spy = Spy::new();
    spy.on(&["apply"], None, Reply::TtyStop);
    spy.on(
        &["-c", "core.autocrlf=false", "apply"],
        None,
        Reply::TtyStop,
    );
    let (git, info) = sb.discover_with(&repo, spy.clone());
    let (cx, out, err) = sb.ctx_with(git, info);
    let g = snp_stash::enter(cx).unwrap();
    let p = patch_of(&g);
    let e = g.restore().unwrap_err();
    assert_eq!(e.exit_code(), 3);
    assert!(exc(&e).1.contains("return code: -9"));
    assert_eq!(
        err.text(),
        "snp: git needed the terminal during the restore and was stopped\n"
    );
    assert!(out.text().contains("Rolling back fixes..."));
    assert!(p.is_file());
    assert_eq!(
        snp_stash::find_unrestored(&repo.join(".git"), &sb.patches(), &repo).len(),
        1
    );
    sb.git(
        &repo,
        &[OsStr::new("apply"), OsStr::new("--3way"), p.as_os_str()],
    );
    assert_eq!(sb.read(&repo, "foo"), b"herp\n");
}

#[test]
fn snp_stash_verify_warns_without_changing_the_outcome() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    sb.write(&repo, "foo", b"herp\n");
    // A correct restore: no warning.
    let (mut cx, _, err) = sb.ctx(&repo);
    cx.verify = true;
    let g = snp_stash::enter(cx).unwrap();
    let p = patch_of(&g);
    g.restore().unwrap();
    assert_eq!(err.text(), "");
    let (git, _) = sb.discover(&repo);
    assert!(snp_stash::verify_restored(&git, &p).unwrap());
    // An apply that claims success without applying: warn only.
    let spy = Spy::new();
    spy.on(
        &["apply", "--whitespace=nowarn"],
        None,
        Reply::Exit(0, Vec::new(), Vec::new()),
    );
    let (git, info) = sb.discover_with(&repo, spy.clone());
    let (mut cx, _, err) = sb.ctx_with(git, info);
    cx.verify = true;
    let g = snp_stash::enter(cx).unwrap();
    let p = patch_of(&g);
    g.restore().unwrap();
    assert_eq!(
        err.text(),
        format!(
            "snp: SNP_STASH_VERIFY: the worktree does not contain every change saved in {} after the restore\n",
            p.display()
        )
    );
}

#[test]
fn dropping_an_unrestored_guard_restores_and_a_panic_unwinds_through_it() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    sb.write(&repo, "foo", b"herp\n");
    let (cx, out, _) = sb.ctx(&repo);
    let g = snp_stash::enter(cx).unwrap();
    drop(g);
    assert_eq!(sb.read(&repo, "foo"), b"herp\n");
    assert!(out.text().ends_with(".\n") && out.text().contains("[INFO] Restored changes from "));
    let (cx, _, _) = sb.ctx(&repo);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = snp_stash::enter(cx).unwrap();
        assert_eq!(std::fs::read(repo.join("foo")).unwrap(), FOO.as_bytes());
        panic!("hook crashed");
    }));
    assert!(r.is_err());
    assert_eq!(sb.read(&repo, "foo"), b"herp\n");
    assert!(snp_stash::recovery_scan(&repo.join(".git"), &sb.patches(), &repo).is_empty());
}

#[test]
fn hazard_a_git_diff_opts_unified_zero_is_restorable() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    sb.write(&repo, "foo", FOO.replace('4', "four").as_bytes());
    let (git, info) = sb.discover(&repo);
    let git = git.with_env(
        git.env()
            .patched(&EnvPatch::default().set("GIT_DIFF_OPTS", "--unified=0")),
    );
    let (cx, out, _) = sb.ctx_with(git, info);
    let g = snp_stash::enter(cx).unwrap();
    assert_eq!(sb.read(&repo, "foo"), FOO.as_bytes());
    // A non-conflicting hook fix needs context lines to apply.
    sb.write(&repo, "foo", FOO.replace('8', "eight").as_bytes());
    g.restore().unwrap();
    assert!(!out.text().contains("Rolling back"));
    assert_eq!(
        sb.read(&repo, "foo"),
        FOO.replace('4', "four").replace('8', "eight").as_bytes()
    );
}

#[test]
fn hazard_b_intent_to_add_paired_as_a_rename() {
    let sb = Sandbox::new();
    let repo = sb.init("r");
    sb.write(&repo, "a.txt", b"same content\nline 2\nline 3\n");
    sb.git(&repo, &["add", "a.txt"]);
    sb.commit(&repo, "c");
    std::fs::remove_file(repo.join("a.txt")).unwrap();
    sb.write(&repo, "b.txt", b"same content\nline 2\nline 3\n");
    sb.git(&repo, &["add", "--intent-to-add", "b.txt"]);
    let (cx, out, _) = sb.ctx(&repo);
    let g = snp_stash::enter(cx).unwrap();
    assert!(repo.join("a.txt").is_file());
    g.restore().unwrap();
    assert!(
        out.text()
            .starts_with("[WARNING] Unstaged intent-to-add files detected.\n")
    );
    assert!(!out.text().contains("Rolling back"));
    // The unstaged deletion and the intent-to-add entry survive.
    assert!(!repo.join("a.txt").exists());
    assert_eq!(sb.status_of(&repo, "a.txt"), "D");
    assert_eq!(sb.status_of(&repo, "b.txt"), "A");
}

#[test]
fn ita_only_with_an_unwritable_store_writes_the_pointer_and_notes_it() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let sb = Sandbox::new();
        let repo = sb.init("r");
        sb.write(&repo, "n", b"x\n");
        sb.git(&repo, &["add", "--intent-to-add", "n"]);
        std::fs::create_dir_all(sb.patches()).unwrap();
        std::fs::set_permissions(sb.patches(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let (cx, out, err) = sb.ctx(&repo);
        let g = snp_stash::enter(cx).unwrap();
        let p = pointer(&repo);
        assert_eq!(
            (p["state"].as_str(), p["journal"].is_null()),
            (Some("started"), true)
        );
        g.restore().unwrap();
        std::fs::set_permissions(sb.patches(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            out.text(),
            "[WARNING] Unstaged intent-to-add files detected.\n"
        );
        assert!(err.text().starts_with(&format!(
            "snp: could not write the stash journal in {} ([Errno 13] Permission denied",
            sb.patches().display()
        )));
        assert_eq!(sb.status_of(&repo, "n"), "A");
        assert!(!repo.join(".git/snp-stash.journal").exists());
    }
}

#[test]
fn linked_worktrees_have_their_own_lock() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    sb.commit(&repo, "c");
    let wt = sb.path("wt");
    sb.git(
        &repo,
        &[
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("-q"),
            wt.as_os_str(),
        ],
    );
    sb.write(&repo, "foo", b"main\n");
    sb.write(&wt, "foo", b"linked\n");
    let (cx, _, _) = sb.ctx(&repo);
    let a = snp_stash::enter(cx).unwrap();
    let (mut cx, _, _) = sb.ctx(&wt);
    cx.lock_timeout = Duration::ZERO;
    let b = snp_stash::enter(cx).unwrap();
    assert_ne!(patch_of(&a), patch_of(&b));
    b.restore().unwrap();
    a.restore().unwrap();
    assert_eq!(
        (sb.read(&repo, "foo"), sb.read(&wt, "foo")),
        (b"main\n".to_vec(), b"linked\n".to_vec())
    );
}
