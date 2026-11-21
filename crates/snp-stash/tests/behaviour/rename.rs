//! ADR-152: the restore by rename (Linux). Each case runs on twin repositories built alike: one restored as `snp run`
//! restores (the `DiffOracle`'s proof handed to the guard), one with `SNP_STASH_RESTORE=apply` (always `git apply`).
//! The two must end alike: the output (patch path folded), every file's bytes and mode, `git status`, `git diff` and
//! the index; and the case decides whether the first renamed the preserved originals back (no restore `git apply`,
//! the original inodes kept) or fell back to `git apply`. No backup outlives a run.

use std::fmt::Write as _;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;

use snp_core::env::EnvPatch;
use snp_core::path::RepoPath;

use crate::common::{Sandbox, Spy, backups_in, prove_around};

/// What a run left, comparable across the twins.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    result: String,
    stdout: String,
    stderr: String,
    files: String,
    status: String,
    diff: String,
    index: String,
}

/// What only one twin tells.
#[derive(Debug)]
struct Seen {
    proven: bool,
    /// Restore `git apply` calls (`apply --whitespace=nowarn …`, with or without `-c core.autocrlf=false`).
    applies: usize,
    /// Per watched file: its inode after the run is the one it had before.
    kept: Vec<bool>,
}

/// The repository of a case: `setup` builds it (the stash's patch is what it leaves unstaged), `hook` is the only
/// hook, `fixes` the paths passed to `stage_fixed`, `watched` the files compared.
struct Case<'a> {
    setup: &'a dyn Fn(&Sandbox, &Path),
    hook: &'a dyn Fn(&Path),
    /// Runs right after the restore (before anything is compared).
    after: &'a dyn Fn(&Path),
    fixes: &'a [&'a str],
    watched: &'a [&'a str],
}

fn lines(salt: u32, n: usize) -> Vec<u8> {
    let mut s = String::new();
    for i in 0..n {
        _ = writeln!(s, "line {i:05} {salt}");
    }
    s.into_bytes()
}

/// A result as the envelope would show it.
fn shown<T: std::fmt::Debug>(r: &Result<T, snp_core::failure::Failure>) -> String {
    match r {
        Ok(v) => format!("Ok({v:?})"),
        Err(snp_core::failure::Failure::Exc { exc, .. }) => format!(
            "{}: {}\n{}",
            exc.ty,
            String::from_utf8_lossy(&exc.text),
            exc.hint.as_deref().unwrap_or_default()
        ),
        Err(f) => format!("{f:?}"),
    }
}

fn run(case: &Case<'_>, apply: bool) -> (Outcome, Seen) {
    let mut sb = Sandbox::new();
    if apply {
        sb.env = sb
            .env
            .patched(&EnvPatch::default().set("SNP_STASH_RESTORE", "apply"));
    }
    let repo = sb.init("r");
    (case.setup)(&sb, &repo);
    // inode and birth time: a new file may reuse a freed inode number, never the birth time
    let ino = |f: &str| {
        std::fs::symlink_metadata(repo.join(f))
            .ok()
            .map(|m| (m.ino(), m.created().ok()))
    };
    let before: Vec<_> = case.watched.iter().map(|f| ino(f)).collect();
    let spy = Spy::new();
    let (git, info) = sb.discover_with(&repo, Arc::<Spy>::clone(&spy));
    let (cx, out, err) = sb.ctx_with(git, info);
    let mut g = snp_stash::enter(cx).unwrap();
    let patch = match g.stashed() {
        snp_stash::Stashed::Patch(p) => p.to_string_lossy().into_owned(),
        other => panic!("the case must stash: {other:?}"),
    };
    let proven = prove_around(&sb, &repo, &mut g, || (case.hook)(&repo));
    let fixes: Vec<RepoPath> = case
        .fixes
        .iter()
        .map(|p| RepoPath::new(p.as_bytes()).unwrap())
        .collect();
    let staged = if fixes.is_empty() {
        Ok(true)
    } else {
        g.stage_fixed(&fixes)
    };
    let restored = g.restore();
    (case.after)(&repo);
    let result = format!("{} {}", shown(&staged), shown(&restored)).replace(&patch, "<patch>");
    let applies = spy
        .calls()
        .iter()
        .filter(|c| {
            c.starts_with("apply --whitespace=nowarn") || c.contains("autocrlf=false apply")
        })
        .count();
    let kept = case
        .watched
        .iter()
        .zip(&before)
        .map(|(f, b)| b.is_some() && ino(f) == *b)
        .collect();
    let mut files = String::new();
    for f in case.watched {
        let seen = std::fs::symlink_metadata(repo.join(f)).ok().map(|md| {
            (
                format!("{:o}", md.permissions().mode()),
                std::fs::read(repo.join(f)).unwrap(),
            )
        });
        _ = writeln!(files, "{f}: {seen:?}");
    }
    assert_eq!(
        backups_in(&repo.join(".git")),
        Vec::<String>::new(),
        "no backup outlives the run"
    );
    let text = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
    let o = Outcome {
        result,
        stdout: out.text().replace(&patch, "<patch>"),
        stderr: err.text().replace(&patch, "<patch>"),
        files,
        status: text(sb.git(
            &repo,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )),
        diff: text(sb.git(&repo, &["diff", "--no-ext-diff"])),
        index: sb.index(&repo),
    };
    (
        o,
        Seen {
            proven,
            applies,
            kept,
        },
    )
}

/// Runs both twins, asserts they end alike and returns what the proven one did.
fn twins(case: &Case<'_>) -> Seen {
    let (proven, seen) = run(case, false);
    let (applied, apply_seen) = run(case, true);
    assert_eq!(
        proven, applied,
        "the restore by rename must end as `git apply` ends"
    );
    assert!(apply_seen.applies >= 1, "{apply_seen:?}");
    seen
}

/// Whether the restore renamed every watched file back.
fn renamed(seen: &Seen) -> bool {
    seen.proven && seen.applies == 0 && seen.kept.iter().all(|k| *k)
}

fn b8(sb: &Sandbox, repo: &Path) {
    sb.write(repo, "big.txt", &lines(1, 4000));
    sb.write(repo, "small.txt", b"small\n");
    sb.write(repo, "d/run.sh", b"#!/bin/sh\necho 1\n");
    std::fs::set_permissions(
        repo.join("d/run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    sb.git(repo, &["add", "."]);
    sb.commit(repo, "init");
    sb.write(repo, "small.txt", b"small, changed\n");
    sb.git(repo, &["add", "small.txt"]);
    let mut big = lines(2, 4000);
    big.extend_from_slice(b"one more line\n");
    sb.write(repo, "big.txt", &big);
    std::fs::set_permissions(repo.join("big.txt"), std::fs::Permissions::from_mode(0o600)).unwrap();
    sb.write(repo, "d/run.sh", b"#!/bin/sh\necho 2\n");
}

const B8_FILES: &[&str] = &["big.txt", "small.txt", "d/run.sh"];

/// The B8 shape with an executable file in a subdirectory: the proven run renames both originals back (their
/// inodes kept), with the mode a fresh file gets (`git apply`'s), and ends exactly as `git apply` ends.
#[test]
fn a_proven_restore_renames_the_originals_back_and_ends_as_git_apply_ends() {
    let seen = twins(&Case {
        setup: &b8,
        hook: &|_| {},
        after: &|_| {},
        fixes: &[],
        watched: &["big.txt", "d/run.sh"],
    });
    assert!(renamed(&seen), "{seen:?}");
}

/// The K9 ¬G case (`hidden.rs::a_hook_edit_outside_the_users_hunks_merges_without_a_rollback`): a hook edit to a
/// file of the patch is a change the oracle sees, so no proof: `git apply` merges as before.
#[test]
fn a_hook_edit_to_a_file_of_the_patch_restores_with_git_apply() {
    let seen = twins(&Case {
        setup: &b8,
        hook: &|repo| {
            let mut big = std::fs::read(repo.join("big.txt")).unwrap();
            big.splice(0..0, b"hook line\n".iter().copied());
            std::fs::write(repo.join("big.txt"), big).unwrap();
        },
        after: &|_| {},
        fixes: &[],
        watched: B8_FILES,
    });
    assert!(!seen.proven && seen.applies >= 1, "{seen:?}");
}

/// A hook that only touches a file of the patch (same bytes, new stat): the snapshot changed, so no proof.
#[test]
fn a_touched_file_of_the_patch_restores_with_git_apply() {
    let seen = twins(&Case {
        setup: &b8,
        hook: &|repo| {
            let f = std::fs::File::options()
                .append(true)
                .open(repo.join("big.txt"))
                .unwrap();
            f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
                .unwrap();
        },
        after: &|_| {},
        fixes: &[],
        watched: B8_FILES,
    });
    assert!(!seen.proven && seen.applies == 1, "{seen:?}");
}

/// A `stage_fixed` staging before the restore (here of an unchanged staged file, so the oracle still proves the
/// worktree): the guard refuses the rename, `git apply` restores.
#[test]
fn a_stage_fixed_staging_restores_with_git_apply() {
    let seen = twins(&Case {
        setup: &b8,
        hook: &|_| {},
        after: &|_| {},
        fixes: &["small.txt"],
        watched: B8_FILES,
    });
    assert!(
        seen.proven && seen.applies == 1 && !seen.kept[0],
        "{seen:?}"
    );
}

/// `core.fileMode` on and the user made a file executable: the patch changes its mode, which the checkout's fresh
/// file does not carry, so `git apply` restores (and sets the new mode).
#[test]
fn a_mode_change_in_the_patch_restores_with_git_apply() {
    let seen = twins(&Case {
        setup: &|sb, repo| {
            b8(sb, repo);
            std::fs::set_permissions(repo.join("big.txt"), std::fs::Permissions::from_mode(0o750))
                .unwrap();
        },
        hook: &|_| {},
        after: &|_| {},
        fixes: &[],
        watched: B8_FILES,
    });
    assert!(
        seen.proven && seen.applies == 1 && !seen.kept[0],
        "{seen:?}"
    );
}

/// `core.fileMode` off: the index mode decides what `git apply` writes, whatever the original's executable bit; the
/// rename gives the original the checkout's fresh mode.
#[test]
fn without_core_filemode_the_index_mode_decides_and_the_rename_applies() {
    let seen = twins(&Case {
        setup: &|sb, repo| {
            sb.git(repo, &["config", "core.fileMode", "false"]);
            b8(sb, repo);
            std::fs::set_permissions(repo.join("big.txt"), std::fs::Permissions::from_mode(0o750))
                .unwrap();
            std::fs::set_permissions(
                repo.join("d/run.sh"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        },
        hook: &|_| {},
        after: &|_| {},
        fixes: &[],
        watched: &["big.txt", "d/run.sh"],
    });
    assert!(renamed(&seen), "{seen:?}");
}

/// An original with a second hard link is never backed up: renaming it back would split the two names.
#[test]
fn a_file_with_a_second_link_restores_with_git_apply() {
    let seen = twins(&Case {
        setup: &|sb, repo| {
            b8(sb, repo);
            std::fs::hard_link(repo.join("big.txt"), repo.join("../big-link")).unwrap();
        },
        hook: &|_| {},
        after: &|_| {},
        fixes: &[],
        watched: B8_FILES,
    });
    assert!(seen.applies == 1 && !seen.kept[0], "{seen:?}");
}

/// A directory made read-only by a hook (directories are compared by identity, so the oracle still proves the
/// files) makes the second rename fail after the first one succeeded: every backed-up path is checked out again and
/// `git apply` runs as on the always-apply twin, which fails the same way (K+ hint). Not as root (permissions do
/// not stop root).
#[test]
fn a_rename_that_fails_after_another_ends_as_git_apply_ends() {
    if rustix::process::geteuid().is_root() {
        return;
    }
    let seen = twins(&Case {
        setup: &b8,
        hook: &|repo| {
            std::fs::set_permissions(repo.join("d"), std::fs::Permissions::from_mode(0o555))
                .unwrap();
        },
        after: &|repo| {
            std::fs::set_permissions(repo.join("d"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        },
        fixes: &[],
        watched: B8_FILES,
    });
    assert!(seen.proven && seen.applies >= 1, "{seen:?}");
}
