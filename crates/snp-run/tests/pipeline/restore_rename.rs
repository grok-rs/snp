//! ADR-152: after a stash whose checkout the `DiffOracle` proved (ADR-119) and whose files no hook changed, the
//! restore renames the preserved originals back instead of running `git apply` (Linux). The run's output, exit code
//! and resulting repository (the worktree's and the index's view through git, and the mode and bytes of every file
//! the cases touch) equal the always-apply run's (`SNP_STASH_RESTORE=apply`) and the always-diff run's
//! (`SNP_DIFF_MODE=git`, which has no proof), `SNP_STASH_VERIFY=1` finds every restore complete, and no backup is
//! left in the git dir.

use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::sync::Arc;
use std::time::Duration;

use crate::harness::{H, Out, StepClock, args, local, sys};

/// Numbered lines; `salt` changes every line.
fn text(salt: u32, lines: usize) -> Vec<u8> {
    let mut s = String::new();
    for i in 0..lines {
        _ = writeln!(s, "line {i:05} {salt:06} lorem ipsum");
    }
    s.into_bytes()
}

/// A hook that changes nothing.
const NOOP: &str = "sh -c 'exit 0' --";

const FILES: [&str; 4] = ["big.txt", "small.txt", "other.txt", "d/run.sh"];

/// The B8 shape (a large unstaged rewrite of `big.txt`, a staged `small.txt` for the hook) plus an unstaged edit of
/// an executable `d/run.sh`; `after` runs once the changes are in place.
fn make(hook: &str, after: &dyn Fn(&H)) -> H {
    let mut h = H::new();
    // the warm window of ADR-078(3) must hold however loaded the machine is
    h.clock = Some(Arc::new(StepClock::new(Duration::from_millis(1))));
    h.config(&local(&sys("p", "hook", hook, "files: '^small\\.txt$'")));
    h.write("big.txt", &text(1, 400));
    h.write("small.txt", b"small\n");
    h.write("other.txt", b"other\n");
    h.exec("d/run.sh", b"#!/bin/sh\necho 1\n");
    h.git(&["add", "."]);
    h.commit("init");
    h.write("small.txt", b"small, changed\n");
    h.stage("small.txt");
    let mut rewrite = text(2, 400);
    rewrite.extend_from_slice(b"one more line\n");
    h.write("big.txt", &rewrite);
    std::fs::set_permissions(h.path("big.txt"), std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(h.path("d/run.sh"), b"#!/bin/sh\necho 2\n").unwrap();
    after(&h);
    h
}

/// What a run leaves (see the module docs).
fn outcome(h: &H, o: &Out) -> String {
    let base = h.base.to_string_lossy().into_owned();
    let diff = h.git_any(&["diff", "--no-ext-diff", "--no-textconv"]);
    let cached = h.git_any(&["diff", "--cached", "--no-ext-diff", "--no-textconv"]);
    let status = h.git_any(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
    let mut files = String::new();
    for f in FILES {
        let seen = std::fs::symlink_metadata(h.path(f)).ok().map(|md| {
            let mut hasher = DefaultHasher::new();
            std::fs::read(h.path(f))
                .unwrap_or_default()
                .hash(&mut hasher);
            (md.permissions().mode(), md.len(), hasher.finish())
        });
        _ = writeln!(files, "{f}: {seen:?}");
    }
    format!(
        "code {}\nstdout\n{}\nstderr\n{}\ndiff {}\ncached {}\nstatus {}\n{files}",
        o.code,
        o.stdout.replace(&base, "<base>"),
        o.stderr.replace(&base, "<base>"),
        String::from_utf8_lossy(&diff.stdout),
        String::from_utf8_lossy(&cached.stdout),
        String::from_utf8_lossy(&status.stdout),
    )
}

/// One run: its outcome, and whether `big.txt` and `d/run.sh` are still the files they were (inode and birth time:
/// renamed back; `git apply` and the checkout always write a new file, which may reuse a freed inode number but not
/// the birth time).
fn run(hook: &str, after: &dyn Fn(&H), env: &[(&str, &str)]) -> (String, bool) {
    let mut h = make(hook, after);
    for (k, v) in env {
        h.env_set(k, *v);
    }
    let ino = |h: &H, f: &str| {
        std::fs::symlink_metadata(h.path(f))
            .ok()
            .map(|m| (m.ino(), m.created().ok()))
    };
    let before = (ino(&h, "big.txt"), ino(&h, "d/run.sh"));
    let o = h.run(args());
    assert!(
        o.stdout.contains("[INFO] Stashing unstaged files to "),
        "the case must stash: {o:?}"
    );
    let left: Vec<String> = std::fs::read_dir(h.path(".git"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("snp-stash-bk-"))
        .collect();
    assert_eq!(left, Vec::<String>::new(), "no backup outlives the run");
    let kept = before.0.is_some() && (ino(&h, "big.txt"), ino(&h, "d/run.sh")) == before;
    (outcome(&h, &o), kept)
}

/// Runs the case three ways (default, always `git apply`, always `git diff`) and with `SNP_STASH_VERIFY=1`; all four
/// end alike. Returns whether the default run renamed the originals back.
fn check(hook: &str, after: &dyn Fn(&H)) -> bool {
    let (got, kept) = run(hook, after, &[]);
    let (apply, apply_kept) = run(hook, after, &[("SNP_STASH_RESTORE", "apply")]);
    let (diff, diff_kept) = run(hook, after, &[("SNP_DIFF_MODE", "git")]);
    let (verified, _) = run(hook, after, &[("SNP_STASH_VERIFY", "1")]);
    assert_eq!(
        got, apply,
        "the default run differs from the always-apply run"
    );
    assert_eq!(
        got, diff,
        "the default run differs from the always-diff run"
    );
    assert_eq!(
        got, verified,
        "SNP_STASH_VERIFY found an incomplete restore"
    );
    assert!(!apply_kept && !diff_kept, "only a proven run renames");
    kept
}

fn nothing(_: &H) {}

/// Whether the restore by rename can run here: Linux and a Stat-mode filesystem (ADR-119's proof).
fn rename_possible() -> bool {
    let h = H::new();
    cfg!(target_os = "linux") && snp_os::fsinfo::stat_oracle_safe(&h.root).unwrap_or(false)
}

/// The B8 shape with a hook that changes nothing: the originals are renamed back.
#[test]
fn the_b8_shape_restores_by_rename() {
    assert_eq!(check(NOOP, &nothing), rename_possible());
}

/// A hook that touches a file of the patch (a stat change the oracle sees): `git apply` restores.
#[test]
fn a_hook_touching_a_file_of_the_patch_restores_with_git_apply() {
    assert!(!check("sh -c 'touch big.txt' --", &nothing));
}

/// A hook that rewrites a file of the patch away from the user's hunks: `git apply` merges both edits.
#[test]
fn a_hook_edit_outside_the_users_hunks_is_merged_by_git_apply() {
    assert!(!check(
        "sh -c 'printf \"#!/bin/sh\\n# hook\\necho 1\\n\" > d/run.sh' --",
        &nothing
    ));
}

/// An unstaged deletion is in the patch: nothing to back up for it, so `git apply` restores.
#[test]
fn an_unstaged_deletion_restores_with_git_apply() {
    assert!(!check(NOOP, &|h| std::fs::remove_file(h.path("other.txt"))
        .unwrap()));
}

/// A smudge filter that does not round-trip: ADR-119's proof fails, so no rename.
#[test]
fn a_converting_filter_restores_with_git_apply() {
    assert!(!check(NOOP, &|h| {
        h.write(".git/info/attributes", b"big.txt filter=upper\n");
        h.git(&["config", "filter.upper.smudge", "sed s/lorem/LOREM/"]);
        h.git(&["config", "filter.upper.clean", "cat"]);
    }));
}
