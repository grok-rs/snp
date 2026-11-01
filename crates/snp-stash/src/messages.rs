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

/// A path list longer than this many bytes, or than [`LIST_MAX_PATHS`] paths, is not spelled out in a hint.
const LIST_MAX_BYTES: usize = 4096;
/// See [`LIST_MAX_BYTES`].
pub(crate) const LIST_MAX_PATHS: usize = 64;

/// Whether `paths` is short enough to be spelled out in a hint.
fn short_list(paths: &[&[u8]]) -> bool {
    let total = paths
        .iter()
        .map(|p| p.len().saturating_add(1))
        .fold(0, usize::saturating_add);
    paths.len() <= LIST_MAX_PATHS && total <= LIST_MAX_BYTES
}

/// ` <path>…` as shell words.
fn push_paths(s: &mut String, paths: &[&[u8]]) {
    for p in paths {
        s.push(' ');
        s.push_str(&sh(&String::from_utf8_lossy(p)));
    }
}

/// What a recovery notice is rendered from (journal data, plus the paths the patch touches).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Recovery<'a> {
    /// The saved patch.
    pub(crate) patch: &'a Path,
    /// The `write-tree` of the index at stash time.
    pub(crate) tree: Option<&'a str>,
    /// The paths the patch touches, each with whether the patch creates it (`None` when the patch could not be read
    /// or parsed).
    pub(crate) patch_paths: Option<&'a [(Vec<u8>, bool)]>,
    /// The paths `stage_fixed` staged (or was about to stage).
    pub(crate) staged_fixed: &'a [Vec<u8>],
    /// The intent-to-add paths the run cleared from the index.
    pub(crate) ita: &'a [Vec<u8>],
    /// The journal that lists `ita` (named when the list is too long to spell out).
    pub(crate) journal: Option<&'a Path>,
}

/// D-DATA-13 (ARCHITECTURE §4.6): the next-run recovery notice for one orphaned journal.
///
/// With the index tree known, the commands first put the patch's paths back to their stash-time contents (the
/// worktree there holds only staged content and hook edits: the user's work is in the patch), apply the patch with
/// `--3way`, and reset the index of those paths and of the `stage_fixed` paths to the stash-time tree:
/// `git --literal-pathspecs checkout <tree> -- <patch paths…> && git apply --3way <patch> && git --literal-pathspecs
/// reset -q <tree> -- <patch paths…> <staged paths…>`, or `git read-tree <tree> && git checkout-index -f -a && git
/// apply --3way <patch> && git read-tree <tree>` when a list is long (or the patch cannot be parsed). Intent-to-add
/// paths are re-added last: `git --literal-pathspecs add --intent-to-add -- <paths…>`.
pub(crate) fn recovery_notice(r: &Recovery<'_>) -> String {
    let p = shown(r.patch);
    let mut s = format!(
        "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in {p}"
    );
    if let Some(t) = r.tree {
        s.push_str(" (index tree ");
        s.push_str(t);
        s.push(')');
    }
    s.push_str(". Restore with: ");
    let apply = format!("git apply --3way {}", sh(&p));
    match r.tree {
        Some(t) => {
            let mut reset: Vec<&[u8]> = r
                .patch_paths
                .unwrap_or_default()
                .iter()
                .map(|(p, _)| p.as_slice())
                .collect();
            for f in r.staged_fixed {
                if !reset.contains(&f.as_slice()) {
                    reset.push(f);
                }
            }
            match r.patch_paths {
                Some(touched) if !touched.is_empty() && short_list(&reset) => {
                    // A path the patch creates is not in the tree (nothing to check out).
                    let existing: Vec<&[u8]> = touched
                        .iter()
                        .filter(|(_, new)| !new)
                        .map(|(p, _)| p.as_slice())
                        .collect();
                    if !existing.is_empty() {
                        s.push_str("git --literal-pathspecs checkout ");
                        s.push_str(t);
                        s.push_str(" --");
                        push_paths(&mut s, &existing);
                        s.push_str(" && ");
                    }
                    s.push_str(&apply);
                    s.push_str(" && git --literal-pathspecs reset -q ");
                    s.push_str(t);
                    s.push_str(" --");
                    push_paths(&mut s, &reset);
                }
                _ => {
                    s.push_str("git read-tree ");
                    s.push_str(t);
                    s.push_str(" && git checkout-index -f -a && ");
                    s.push_str(&apply);
                    s.push_str(" && git read-tree ");
                    s.push_str(t);
                }
            }
        }
        None => s.push_str(&apply),
    }
    push_ita(&mut s, r.ita, r.journal);
    s
}

/// ` && git --literal-pathspecs add --intent-to-add -- <paths…>` (or a pointer to the journal for a long list).
fn push_ita(s: &mut String, ita: &[Vec<u8>], journal: Option<&Path>) {
    if ita.is_empty() {
        return;
    }
    let list: Vec<&[u8]> = ita.iter().map(Vec::as_slice).collect();
    if short_list(&list) || journal.is_none() {
        s.push_str(" && git --literal-pathspecs add --intent-to-add --");
        push_paths(s, &list);
    } else if let Some(j) = journal {
        s.push_str(", then re-add the intent-to-add files listed under \"ita\" in ");
        s.push_str(&shown(j));
    }
}

/// D-DATA-13 (ARCHITECTURE §4.6, ADR-083): printed once after the recovery notices of a pass — how a notice stops.
pub(crate) const DISMISS_HINT: &str = "snp: this notice repeats until those changes are back in the worktree; to \
                                       dismiss it and keep the patch, run: snp doctor --fix --stash";

/// D-DATA-13 (ARCHITECTURE §4.6, ADR-083): the changes of an orphaned run are back in the worktree (`git apply
/// --check --reverse` of its patch succeeds); its journal is retired. Printed once.
pub(crate) fn retired_notice(patch: &Path) -> String {
    format!(
        "snp: the unstaged changes saved in {} are back in the worktree; the recovery notice of that interrupted run \
         is retired",
        shown(patch)
    )
}

/// D-DATA-13 (ARCHITECTURE §4.6): a run was interrupted after it cleared intent-to-add entries and before it saved a
/// patch (state `started`): no unstaged content is at risk, only the intent-to-add marks. Printed once.
pub(crate) fn ita_notice(ita: &[Vec<u8>], journal: Option<&Path>) -> String {
    let mut s = "snp: an earlier run was interrupted before it re-added its intent-to-add files. Restore with: "
        .to_owned();
    let mut tail = String::new();
    push_ita(&mut tail, ita, journal);
    s.push_str(
        tail.strip_prefix(" && ")
            .or_else(|| tail.strip_prefix(", then "))
            .unwrap_or(&tail),
    );
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

    fn rec<'a>(
        patch: &'a Path,
        tree: Option<&'a str>,
        patch_paths: Option<&'a [(Vec<u8>, bool)]>,
        staged_fixed: &'a [Vec<u8>],
        ita: &'a [Vec<u8>],
    ) -> Recovery<'a> {
        Recovery {
            patch,
            tree,
            patch_paths,
            staged_fixed,
            ita,
            journal: Some(Path::new("/s/patches/run-1-ab.snp-journal")),
        }
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
        let foo = [(b"foo".to_vec(), false)];
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), Some(&foo), &[], &[])),
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
             /s/patches/patch1-2 (index tree abc). Restore with: git --literal-pathspecs checkout abc -- foo && git \
             apply --3way /s/patches/patch1-2 && git --literal-pathspecs reset -q abc -- foo"
        );
        assert_eq!(
            recovery_notice(&rec(p, None, Some(&foo), &[], &[])),
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
             /s/patches/patch1-2. Restore with: git apply --3way /s/patches/patch1-2"
        );
        let staged = [b"a*.txt".to_vec(), b"d/b c".to_vec(), b"foo".to_vec()];
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), Some(&foo), &staged, &[b"n".to_vec()])),
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
             /s/patches/patch1-2 (index tree abc). Restore with: git --literal-pathspecs checkout abc -- foo && git \
             apply --3way /s/patches/patch1-2 && git --literal-pathspecs reset -q abc -- foo 'a*.txt' 'd/b c' && git \
             --literal-pathspecs add --intent-to-add -- n"
        );
        let many: Vec<Vec<u8>> = (0..65).map(|i| format!("f{i}").into_bytes()).collect();
        let long = "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
                    /s/patches/patch1-2 (index tree abc). Restore with: git read-tree abc && git checkout-index -f -a \
                    && git apply --3way /s/patches/patch1-2 && git read-tree abc";
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), Some(&foo), &many, &[])),
            long
        );
        let many_touched: Vec<(Vec<u8>, bool)> = many.iter().map(|p| (p.clone(), false)).collect();
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), Some(&many_touched), &[], &[])),
            long
        );
        // A path the patch creates is reset but not checked out.
        let created = [(b"new".to_vec(), true), (b"foo".to_vec(), false)];
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), Some(&created), &[], &[])),
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
             /s/patches/patch1-2 (index tree abc). Restore with: git --literal-pathspecs checkout abc -- foo && git \
             apply --3way /s/patches/patch1-2 && git --literal-pathspecs reset -q abc -- new foo"
        );
        // An unparseable patch: the whole-index form.
        assert_eq!(recovery_notice(&rec(p, Some("abc"), None, &[], &[])), long);
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), None, &[], &many)),
            format!(
                "{long}, then re-add the intent-to-add files listed under \"ita\" in \
                 /s/patches/run-1-ab.snp-journal"
            )
        );
        assert_eq!(
            ita_notice(&[b"n".to_vec(), b"a b".to_vec()], None),
            "snp: an earlier run was interrupted before it re-added its intent-to-add files. Restore with: git \
             --literal-pathspecs add --intent-to-add -- n 'a b'"
        );
        assert_eq!(
            ita_notice(&many, Some(Path::new("/r/.git/snp-stash.journal"))),
            "snp: an earlier run was interrupted before it re-added its intent-to-add files. Restore with: re-add the \
             intent-to-add files listed under \"ita\" in /r/.git/snp-stash.journal"
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
