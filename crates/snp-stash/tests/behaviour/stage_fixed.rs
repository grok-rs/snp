//! `stage_fixed` (ARCHITECTURE §4.5, §8.3; 05 §18.4 A4/A5): apply-check ordering, the durable `staging-fixed` journal
//! state before the add, the conflict path, the CRLF quirk, binary patches and `GIT_DIFF_OPTS`.

use std::sync::{Arc, Mutex};

use snp_core::env::EnvPatch;
use snp_core::path::RepoPath;
use snp_stash::Stashed;

use crate::common::{FOO, Sandbox, Spy};

fn rp(s: &str) -> RepoPath {
    RepoPath::new(s.as_bytes()).unwrap()
}

#[test]
fn check_then_durable_journal_then_add_then_restore() {
    let sb = Sandbox::new();
    let repo = sb.init("r");
    sb.write(&repo, "foo", FOO.as_bytes());
    sb.write(&repo, "fixme", b"bad  \n");
    sb.git(&repo, &["add", "foo", "fixme"]);
    sb.write(&repo, "foo", format!("{FOO}9\n").as_bytes());
    let journal_at_add = Arc::new(Mutex::new(None));
    let spy = Spy::new();
    {
        let seen = Arc::clone(&journal_at_add);
        let repo = repo.clone();
        spy.before(
            &["--literal-pathspecs", "add"],
            None,
            Box::new(move |_| {
                let p: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(repo.join(".git/snp-stash.journal")).unwrap(),
                )
                .unwrap();
                *seen.lock().unwrap() = Some((p["state"].clone(), p["staged_fixed"].clone()));
            }),
        );
    }
    let (git, info) = sb.discover_with(&repo, Arc::<Spy>::clone(&spy));
    let (cx, out, _) = sb.ctx_with(git, info);
    let mut g = snp_stash::enter(cx).unwrap();
    let Stashed::Patch(patch) = g.stashed().clone() else {
        panic!()
    };
    // The hook fixes `fixme` (no conflict with the user's unstaged `foo` change).
    sb.write(&repo, "fixme", b"bad\n");
    assert!(g.stage_fixed(&[rp("fixme")]).unwrap());
    let (state, staged) = journal_at_add.lock().unwrap().take().unwrap();
    assert_eq!(
        (state, staged),
        ("staging-fixed".into(), serde_json::json!(["fixme"]))
    );
    let calls = spy.calls();
    let check = calls
        .iter()
        .position(|c| c.starts_with("apply --check --whitespace=nowarn"))
        .unwrap();
    let add = calls
        .iter()
        .position(|c| c == "--literal-pathspecs add -- fixme")
        .unwrap();
    assert!(check < add);
    assert_eq!(
        calls[check],
        format!("apply --check --whitespace=nowarn {}", patch.display())
    );
    g.restore().unwrap();
    // The index has the fix; the user's unstaged change is back on top; fixme is clean.
    assert_eq!(sb.status_of(&repo, "fixme"), "A");
    assert_eq!(sb.git(&repo, &["show", ":fixme"]), b"bad\n");
    assert_eq!(sb.read(&repo, "foo"), format!("{FOO}9\n").as_bytes());
    assert_eq!(sb.status_of(&repo, "foo"), "AM");
    assert!(out.text().ends_with(&format!(
        "[INFO] Restored changes from {}.\n",
        patch.display()
    )));
    assert!(!repo.join(".git/snp-stash.journal").exists());
}

#[test]
fn conflict_stages_nothing_and_rolls_back() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    sb.write(&repo, "foo", FOO.replace('1', "a").as_bytes());
    let index_before = sb.index(&repo);
    let (cx, out, _) = sb.ctx(&repo);
    let mut g = snp_stash::enter(cx).unwrap();
    sb.write(&repo, "foo", FOO.replace('1', "b").as_bytes());
    assert!(!g.stage_fixed(&[rp("foo")]).unwrap());
    assert_eq!(sb.index(&repo), index_before, "nothing staged");
    g.restore().unwrap();
    assert!(out.text().contains("Rolling back fixes..."));
    assert_eq!(sb.read(&repo, "foo"), FOO.replace('1', "a").as_bytes());
    assert_eq!(sb.index(&repo), index_before);
}

#[test]
fn crlf_quirk_has_no_patch_and_stages_directly() {
    let sb = Sandbox::new();
    let repo = sb.init("r");
    sb.git(&repo, &["config", "--local", "core.autocrlf", "true"]);
    sb.write(&repo, "foo", b"1\r\n2\r\n");
    sb.write(&repo, "fixme", b"x  \n");
    sb.git(&repo, &["add", "foo", "fixme"]);
    sb.write(&repo, "foo", b"1\n2\n");
    let (cx, _, _) = sb.ctx(&repo);
    let mut g = snp_stash::enter(cx).unwrap();
    assert_eq!(g.stashed(), &Stashed::Nothing);
    assert!(!g.diff_index_clean());
    sb.write(&repo, "fixme", b"x\n");
    assert!(g.stage_fixed(&[rp("fixme")]).unwrap());
    g.restore().unwrap();
    assert_eq!(sb.git(&repo, &["show", ":fixme"]), b"x\n");
}

#[test]
fn binary_patch_and_unified_zero_are_restorable_with_staged_fixes() {
    let sb = Sandbox::new();
    let repo = sb.init("r");
    let mut bin: Vec<u8> = (0..=255_u8).cycle().take(3000).collect();
    sb.write(&repo, "img.bin", &bin);
    sb.write(&repo, "fixme", b"y  \n");
    sb.write(&repo, "foo", FOO.as_bytes());
    sb.git(&repo, &["add", "."]);
    bin[100] = 0;
    bin[2000] = 7;
    sb.write(&repo, "img.bin", &bin);
    sb.write(&repo, "foo", FOO.replace('2', "two").as_bytes());
    let (git, info) = sb.discover(&repo);
    let git = git.with_env(
        git.env()
            .patched(&EnvPatch::default().set("GIT_DIFF_OPTS", "--unified=0")),
    );
    let (cx, out, _) = sb.ctx_with(git, info);
    let mut g = snp_stash::enter(cx).unwrap();
    let Stashed::Patch(p) = g.stashed().clone() else {
        panic!()
    };
    assert!(String::from_utf8_lossy(&std::fs::read(&p).unwrap()).contains("GIT binary patch"));
    sb.write(&repo, "fixme", b"y\n");
    sb.write(&repo, "foo", FOO.replace('7', "seven").as_bytes());
    assert!(g.stage_fixed(&[rp("fixme"), rp("foo")]).unwrap());
    g.restore().unwrap();
    assert!(!out.text().contains("Rolling back"));
    assert_eq!(sb.read(&repo, "img.bin"), bin);
    assert_eq!(
        sb.read(&repo, "foo"),
        FOO.replace('2', "two").replace('7', "seven").as_bytes()
    );
    assert_eq!(
        sb.git(&repo, &["show", ":foo"]),
        FOO.replace('7', "seven").as_bytes()
    );
}

#[test]
fn kill_at_staging_fixed_is_recoverable_by_the_hint() {
    // The in-process half of the kill-9 matrix row: the journal written before the add names the reset.
    let sb = Sandbox::new();
    let repo = sb.init("r");
    sb.write(&repo, "foo", FOO.as_bytes());
    sb.write(&repo, "fixme", b"z  \n");
    sb.git(&repo, &["add", "."]);
    sb.write(&repo, "foo", format!("{FOO}9\n").as_bytes());
    let index_before = sb.index(&repo);
    let tree = sb.git_text(&repo, &["write-tree"]);
    let snapshot = Arc::new(Mutex::new(None));
    let spy = Spy::new();
    {
        let snapshot = Arc::clone(&snapshot);
        let git_dir = repo.join(".git");
        let patches = sb.patches();
        let root = repo.clone();
        spy.before(
            &["--literal-pathspecs", "add"],
            None,
            Box::new(move |_| {
                *snapshot.lock().unwrap() =
                    Some(snp_stash::find_unrestored(&git_dir, &patches, &root));
            }),
        );
    }
    let (git, info) = sb.discover_with(&repo, Arc::<Spy>::clone(&spy));
    let (cx, _, _) = sb.ctx_with(git, info);
    let mut g = snp_stash::enter(cx).unwrap();
    sb.write(&repo, "fixme", b"z\n");
    assert!(g.stage_fixed(&[rp("fixme")]).unwrap());
    let found = snapshot.lock().unwrap().take().unwrap();
    assert_eq!(found.len(), 1);
    let u = &found[0];
    assert_eq!(
        (&*u.state, u.tree.as_deref()),
        ("staging-fixed", Some(tree.as_str()))
    );
    assert_eq!(
        u.hint(),
        format!(
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in {0} \
             (index tree {tree}). Restore with: git --literal-pathspecs checkout {tree} -- foo && git apply --3way {0} \
             && git --literal-pathspecs reset -q {tree} -- foo fixme",
            u.patch.display()
        )
    );
    // The kill right after the add: no restore runs (the leaked guard keeps its lock, which the hint never needs).
    let _leaked = std::mem::ManuallyDrop::new(g);
    run_hint(&sb, &repo, &u.hint());
    // Exactly the pre-run index and unstaged work; the hook fix stays in the worktree, unstaged.
    assert_eq!(sb.index(&repo), index_before);
    assert_eq!(sb.git(&repo, &["show", ":fixme"]), b"z  \n");
    assert_eq!(sb.read(&repo, "fixme"), b"z\n");
    assert_eq!(sb.read(&repo, "foo"), format!("{FOO}9\n").as_bytes());
}

/// Runs the commands after `Restore with: ` exactly as printed.
fn run_hint(sb: &Sandbox, repo: &std::path::Path, hint: &str) {
    let cmds = hint.split("Restore with: ").nth(1).unwrap();
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmds)
        .current_dir(repo)
        .env_clear()
        .envs(sb.env.iter())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{cmds}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn kill_at_staging_fixed_of_a_file_in_the_patch_is_recoverable_by_the_hint() {
    // The fixer rewrote a file the user has partially staged: the stage-fixed path is also in the patch.
    let sb = Sandbox::new();
    let repo = sb.init("r");
    let staged = "1  \n2\n3\n4\n5\n6\n7\n8\n";
    sb.write(&repo, "x", staged.as_bytes());
    sb.git(&repo, &["add", "x"]);
    let user = staged.replace("8\n", "USER\n");
    sb.write(&repo, "x", user.as_bytes());
    let index_before = sb.index(&repo);
    let diff_before = sb.unstaged_diff(&repo);
    let (cx, _, _) = sb.ctx(&repo);
    let mut g = snp_stash::enter(cx).unwrap();
    sb.write(&repo, "x", staged.replace("1  ", "1").as_bytes());
    assert!(g.stage_fixed(&[rp("x")]).unwrap());
    let found = snp_stash::find_unrestored(&repo.join(".git"), &sb.patches(), &repo);
    assert_eq!(found.len(), 1);
    let _leaked = std::mem::ManuallyDrop::new(g);
    run_hint(&sb, &repo, &found[0].hint());
    assert_eq!(sb.read(&repo, "x"), user.as_bytes());
    assert_eq!(sb.index(&repo), index_before);
    assert_eq!(sb.unstaged_diff(&repo), diff_before);
}
