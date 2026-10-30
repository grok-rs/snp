//! Every user-visible text of the stash guard (ARCHITECTURE §1.3 "Messages").
//!
//! The four log lines are pre-commit's (`staged_files_only.py:39,68-69,87-90,98`) and carry no tool name, so both brands
//! print them verbatim. The other texts are snp-only (COMPATIBILITY D-DATA-01, D-DATA-13, D-DATA-17, D-DIAG-04): they
//! name snp in both brands, like every snp-only diagnostic (COMPATIBILITY §5.8), and go to stderr — except the
//! `FatalError` texts, which the envelope prints on stdout like any exception.
//!
//! Owner: WP-23.

use std::path::Path;
use std::time::Duration;

use snp_core::path::RepoPath;
use snp_pycompat::shlex;

/// `staged_files_only.py:39`: `logger.warning('Unstaged intent-to-add files detected.')`.
pub(crate) const ITA_DETECTED: &str = "Unstaged intent-to-add files detected.";

/// `staged_files_only.py:68`: `logger.warning('Unstaged files detected.')`.
pub(crate) const UNSTAGED_DETECTED: &str = "Unstaged files detected.";

/// `staged_files_only.py:87-90`: the rollback warning.
pub(crate) const CONFLICT_ROLLBACK: &str =
    "Stashed changes conflicted with hook auto-fixes... Rolling back fixes...";

/// D-DIAG-04: printed once on stderr when the worktree lock is contended (ARCHITECTURE §4.2 step 1).
pub(crate) const LOCK_WAIT: &str = "snp: waiting for another snp run in this worktree\u{2026}";

/// D-DIAG-04: a restore command was stopped by SIGTTIN/SIGTTOU and killed (ARCHITECTURE §4.2 step 10, ADR-065).
pub(crate) const TTY_STOPPED: &str =
    "snp: git needed the terminal during the restore and was stopped";

/// D-DATA-13: the re-verification `diff-index` kept disagreeing with the saved patch (ARCHITECTURE §4.2 step 6).
pub(crate) const KEPT_CHANGING: &str =
    "snp: unstaged files kept changing while stashing; nothing was modified";

/// A path as pre-commit prints it in its log lines (`str` of the joined path).
pub(crate) fn shown(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `staged_files_only.py:69`: `logger.info(f'Stashing unstaged files to {patch_filename}.')`.
pub(crate) fn stashing(patch: &Path) -> String {
    format!("Stashing unstaged files to {}.", shown(patch))
}

/// `staged_files_only.py:98`: `logger.info(f'Restored changes from {patch_filename}.')`.
pub(crate) fn restored(patch: &Path) -> String {
    format!("Restored changes from {}.", shown(patch))
}

/// The argument as a POSIX shell word (`shlex.quote`), so a hint can be pasted.
fn sh(s: &str) -> String {
    shlex::quote(s).into_owned()
}

/// D-DATA-01 (K+, ADR-045/ADR-067): the stderr line after the envelope of a failed restore.
pub(crate) fn k_plus_hint(patch: &Path) -> String {
    let p = shown(patch);
    format!(
        "[ERROR] Your unstaged changes are saved in {p}; restore them with: git apply --3way {}",
        sh(&p)
    )
}

/// More than this many bytes of `git reset` paths, or more than [`RESET_MAX_PATHS`] paths, use `git read-tree`.
const RESET_MAX_BYTES: usize = 4096;
/// See [`RESET_MAX_BYTES`].
const RESET_MAX_PATHS: usize = 64;

/// D-DATA-13 (ARCHITECTURE §4.6): the next-run recovery notice for one orphaned journal.
///
/// `staged_fixed` (state `staging-fixed`) prepends the index reset: `git reset -q <tree> -- <paths…>`, or
/// `git read-tree <tree>` when the list is long.
pub(crate) fn recovery_notice(patch: &Path, tree: Option<&str>, staged_fixed: &[RepoPath]) -> String {
    let p = shown(patch);
    let mut s = format!(
        "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in {p}"
    );
    if let Some(t) = tree {
        s.push_str(" (index tree ");
        s.push_str(t);
        s.push(')');
    }
    s.push_str(". Restore with: ");
    if let (false, Some(t)) = (staged_fixed.is_empty(), tree) {
        let total: usize = staged_fixed
            .iter()
            .map(|r| r.as_bytes().len().saturating_add(1))
            .fold(0, usize::saturating_add);
        if staged_fixed.len() > RESET_MAX_PATHS || total > RESET_MAX_BYTES {
            s.push_str("git read-tree ");
            s.push_str(t);
        } else {
            s.push_str("git reset -q ");
            s.push_str(t);
            s.push_str(" --");
            for path in staged_fixed {
                s.push(' ');
                s.push_str(&sh(&path.lossy()));
            }
        }
        s.push_str(" && ");
    }
    s.push_str("git apply --3way ");
    s.push_str(&sh(&p));
    s
}

/// D-DATA-13: the `FatalError` text after waiting `timeout` for the worktree lock.
pub(crate) fn lock_timeout(timeout: Duration, lock: &Path) -> String {
    format!(
        "snp: another snp run in this worktree still holds {} after {} s; set SNP_STASH_LOCK_TIMEOUT (seconds, at most \
         3600) to wait longer",
        shown(lock),
        timeout.as_secs()
    )
}

/// D-DATA-17 (ADR-063): the refusal of a non-empty stash on git < 2.19.
pub(crate) fn old_git(version: &str, exe: &str) -> String {
    format!(
        "git {version} cannot stash intent-to-add files safely; snp requires git 2.19.0 or newer to stash unstaged \
         changes (found {exe})."
    )
}

/// ARCHITECTURE §4.2 step 2: the store journal could not be written and only intent-to-add files are cleared (no
/// patch to protect), so the run continues with the git-dir pointer only.
pub(crate) fn journal_skipped(dir: &Path, err: &str) -> String {
    format!(
        "snp: could not write the stash journal in {} ({err}); continuing without it",
        shown(dir)
    )
}

/// `SNP_STASH_VERIFY=1` (ARCHITECTURE §4.2 step 10e): warn-only.
pub(crate) fn verify_failed(patch: &Path) -> String {
    format!(
        "snp: SNP_STASH_VERIFY: the worktree does not contain every change saved in {} after the restore",
        shown(patch)
    )
}

/// The panic-path restore (`Drop`) failed: the exception line on stderr (the envelope is not reached on this path).
pub(crate) fn drop_restore_failed(ty: &str, text: &str) -> String {
    format!("snp: restoring the stash failed: {ty}: {text}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_commit_lines_are_verbatim() {
        assert_eq!(ITA_DETECTED, "Unstaged intent-to-add files detected.");
        assert_eq!(UNSTAGED_DETECTED, "Unstaged files detected.");
        assert_eq!(
            CONFLICT_ROLLBACK,
            "Stashed changes conflicted with hook auto-fixes... Rolling back fixes..."
        );
        let p = Path::new("/s/patches/patch1791403656-1781218");
        assert_eq!(
            stashing(p),
            "Stashing unstaged files to /s/patches/patch1791403656-1781218."
        );
        assert_eq!(
            restored(p),
            "Restored changes from /s/patches/patch1791403656-1781218."
        );
    }

    #[test]
    fn hints() {
        let p = Path::new("/s/patches/patch1-2");
        assert_eq!(
            k_plus_hint(p),
            "[ERROR] Your unstaged changes are saved in /s/patches/patch1-2; restore them with: git apply --3way \
             /s/patches/patch1-2"
        );
        let spaced = Path::new("/h o/patch1-2");
        assert_eq!(
            k_plus_hint(spaced),
            "[ERROR] Your unstaged changes are saved in /h o/patch1-2; restore them with: git apply --3way \
             '/h o/patch1-2'"
        );
        assert_eq!(
            recovery_notice(p, Some("abc"), &[]),
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
             /s/patches/patch1-2 (index tree abc). Restore with: git apply --3way /s/patches/patch1-2"
        );
        assert_eq!(
            recovery_notice(p, None, &[]),
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
             /s/patches/patch1-2. Restore with: git apply --3way /s/patches/patch1-2"
        );
        let paths = [
            RepoPath::new(b"a.txt").unwrap(),
            RepoPath::new(b"d/b c").unwrap(),
        ];
        assert_eq!(
            recovery_notice(p, Some("abc"), &paths),
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
             /s/patches/patch1-2 (index tree abc). Restore with: git reset -q abc -- a.txt 'd/b c' && git apply \
             --3way /s/patches/patch1-2"
        );
        let many: Vec<RepoPath> = (0..65)
            .map(|i| RepoPath::new(format!("f{i}").as_bytes()).unwrap())
            .collect();
        assert_eq!(
            recovery_notice(p, Some("abc"), &many),
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
             /s/patches/patch1-2 (index tree abc). Restore with: git read-tree abc && git apply --3way \
             /s/patches/patch1-2"
        );
    }

    #[test]
    fn snp_only_texts() {
        assert_eq!(
            LOCK_WAIT,
            "snp: waiting for another snp run in this worktree\u{2026}"
        );
        assert_eq!(
            TTY_STOPPED,
            "snp: git needed the terminal during the restore and was stopped"
        );
        assert_eq!(
            KEPT_CHANGING,
            "snp: unstaged files kept changing while stashing; nothing was modified"
        );
        assert_eq!(
            old_git("2.18.5", "/opt/git-2.18.5/bin/git"),
            "git 2.18.5 cannot stash intent-to-add files safely; snp requires git 2.19.0 or newer to stash unstaged \
             changes (found /opt/git-2.18.5/bin/git)."
        );
        assert_eq!(
            lock_timeout(Duration::from_secs(60), Path::new("/r/.git/snp-stash.lock")),
            "snp: another snp run in this worktree still holds /r/.git/snp-stash.lock after 60 s; set \
             SNP_STASH_LOCK_TIMEOUT (seconds, at most 3600) to wait longer"
        );
        assert_eq!(
            journal_skipped(Path::new("/s/patches"), "[Errno 13] Permission denied"),
            "snp: could not write the stash journal in /s/patches ([Errno 13] Permission denied); continuing without \
             it"
        );
        assert_eq!(
            verify_failed(Path::new("/s/p")),
            "snp: SNP_STASH_VERIFY: the worktree does not contain every change saved in /s/p after the restore"
        );
    }
}
