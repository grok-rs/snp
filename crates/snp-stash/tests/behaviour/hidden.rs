//! D-DATA-19: changes `git diff` cannot see — hidden by a clean filter, or in an `--assume-unchanged` file — survive
//! the stash. pre-commit's whole-tree `git checkout -- .` rewrote every stat-dirty file from the index, so they were
//! lost; snp checks out only the patch's files (and, on the conflict rollback, every file `git diff` shows changed).

use std::sync::Arc;

use snp_stash::Stashed;

use crate::common::{FOO, Sandbox, Spy};

/// A clean filter that drops the `SECRET` lines, so a `SECRET` line is a change git cannot see.
#[cfg(unix)]
fn with_strip_filter(sb: &Sandbox, repo: &std::path::Path) {
    sb.git(repo, &["config", "filter.strip.clean", "grep -v SECRET"]);
    sb.write(repo, ".gitattributes", b"secret.cfg filter=strip\n");
    sb.write(repo, "secret.cfg", b"key=1\n");
    sb.write(repo, "other", b"other\n");
    sb.git(repo, &["add", ".gitattributes", "secret.cfg", "other"]);
    sb.commit(repo, "base");
}

const HIDDEN: &[u8] = b"key=1\nSECRET=hunter2\n";

#[cfg(unix)]
#[test]
fn a_change_a_clean_filter_hides_survives_the_stash() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    with_strip_filter(&sb, &repo);
    sb.write(&repo, "secret.cfg", HIDDEN);
    sb.write(&repo, "foo", FOO.replace('4', "four").as_bytes());
    let (cx, out, _) = sb.ctx(&repo);
    let g = snp_stash::enter(cx).unwrap();
    let Stashed::Patch(p) = g.stashed().clone() else {
        panic!("no patch")
    };
    // The patch has only what git can see; the hidden line is never in it, and the checkout leaves it alone.
    let patch = std::fs::read(&p).unwrap();
    assert!(!String::from_utf8_lossy(&patch).contains("secret.cfg"));
    assert_eq!(sb.read(&repo, "foo"), FOO.as_bytes());
    assert_eq!(sb.read(&repo, "secret.cfg"), HIDDEN);
    g.restore().unwrap();
    assert!(!out.text().contains("Rolling back"));
    assert_eq!(sb.read(&repo, "foo"), FOO.replace('4', "four").as_bytes());
    assert_eq!(sb.read(&repo, "secret.cfg"), HIDDEN);
}

#[cfg(unix)]
#[test]
fn the_conflict_rollback_keeps_a_hidden_change_and_discards_every_visible_fix() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    with_strip_filter(&sb, &repo);
    sb.write(&repo, "secret.cfg", HIDDEN);
    sb.write(&repo, "foo", FOO.replace('1', "a").as_bytes());
    let index = sb.index(&repo);
    let (cx, out, _) = sb.ctx(&repo);
    let g = snp_stash::enter(cx).unwrap();
    // The "hook": a fix that conflicts with the stashed change, and a fix in a file the patch does not touch.
    sb.write(&repo, "foo", FOO.replace('1', "b").as_bytes());
    sb.write(&repo, "other", b"fixed\n");
    g.restore().unwrap();
    assert!(
        out.text().contains("Rolling back fixes..."),
        "{}",
        out.text()
    );
    // As in pre-commit: every fix git can see is discarded and the user's change is back …
    assert_eq!(sb.read(&repo, "foo"), FOO.replace('1', "a").as_bytes());
    assert_eq!(sb.read(&repo, "other"), b"other\n");
    assert_eq!(sb.index(&repo), index);
    // … and, unlike pre-commit, the change git cannot see is still there.
    assert_eq!(sb.read(&repo, "secret.cfg"), HIDDEN);
}

#[test]
fn an_assume_unchanged_edit_survives_the_stash() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    sb.write(&repo, "local.cfg", b"default\n");
    sb.git(&repo, &["add", "local.cfg"]);
    sb.commit(&repo, "base");
    sb.git(&repo, &["update-index", "--assume-unchanged", "local.cfg"]);
    sb.write(&repo, "local.cfg", b"machine-local\n");
    sb.write(&repo, "foo", FOO.replace('4', "four").as_bytes());
    let (cx, _, _) = sb.ctx(&repo);
    let g = snp_stash::enter(cx).unwrap();
    assert_eq!(sb.read(&repo, "local.cfg"), b"machine-local\n");
    g.restore().unwrap();
    assert_eq!(sb.read(&repo, "local.cfg"), b"machine-local\n");
    assert_eq!(sb.read(&repo, "foo"), FOO.replace('4', "four").as_bytes());
}

/// Every name is a literal pathspec: `*` and `[ab]` never match other files (here an `--assume-unchanged` edit a glob
/// would overwrite), and names with spaces, quotes and newlines are checked out and restored exactly.
#[cfg(unix)]
#[test]
fn names_are_literal_pathspecs() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let names: &[&str] = &["*", "[ab]", "sp ace", "quo\"te", "new\nline", "a"];
    for n in names {
        sb.write(&repo, n, b"base\n");
    }
    sb.git(&repo, &["add", "--", "."]);
    sb.commit(&repo, "base");
    sb.git(&repo, &["update-index", "--assume-unchanged", "a"]);
    sb.write(&repo, "a", b"local edit\n");
    for n in &names[..5] {
        sb.write(&repo, n, b"user\n");
    }
    let (cx, _, _) = sb.ctx(&repo);
    let g = snp_stash::enter(cx).unwrap();
    for n in &names[..5] {
        assert_eq!(sb.read(&repo, n), b"base\n", "{n:?}");
    }
    assert_eq!(sb.read(&repo, "a"), b"local edit\n");
    g.restore().unwrap();
    for n in &names[..5] {
        assert_eq!(sb.read(&repo, n), b"user\n", "{n:?}");
    }
    assert_eq!(sb.read(&repo, "a"), b"local edit\n");
}

/// The list goes to git on stdin: a patch with many files (long names) runs one checkout with a fixed argv.
#[test]
fn many_files_take_one_checkout_with_a_fixed_argv() {
    let sb = Sandbox::new();
    let repo = sb.foo_staged("r");
    let long: String = std::iter::repeat_n('n', 120).collect();
    let names: Vec<String> = (0..400).map(|i| format!("{long}-{i}")).collect();
    for n in &names {
        sb.write(&repo, n, b"base\n");
    }
    sb.git(&repo, &["add", "--", "."]);
    sb.commit(&repo, "base");
    for n in &names {
        sb.write(&repo, n, b"user\n");
    }
    let spy = Spy::new();
    let (git, info) = sb.discover_with(&repo, Arc::<Spy>::clone(&spy));
    let (cx, _, _) = sb.ctx_with(git, info);
    let g = snp_stash::enter(cx).unwrap();
    assert!(names.iter().all(|n| sb.read(&repo, n) == b"base\n"));
    let checkouts: Vec<String> = spy
        .calls()
        .into_iter()
        .filter(|c| c.contains(" checkout"))
        .collect();
    assert_eq!(
        checkouts,
        ["-c submodule.recurse=0 checkout --pathspec-from-file=- --pathspec-file-nul"]
    );
    g.restore().unwrap();
    assert!(names.iter().all(|n| sb.read(&repo, n) == b"user\n"));
}
