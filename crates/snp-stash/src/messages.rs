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

/// D-DATA-19: `git apply --numstat` listed no file of a non-empty stash patch (git would have to contradict itself),
/// so snp cannot tell which files to check out; nothing was modified.
pub(crate) fn no_patch_paths(patch: &Path) -> String {
    format!(
        "`git apply --numstat` listed no files for the stash patch {}; nothing was modified",
        shown(patch)
    )
}

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

/// `git apply --whitespace=nowarn <patch>`: pre-commit's own restore command (`staged_files_only.py:_git_apply`). It
/// changes the worktree only, and it is all-or-nothing: when a file changed since the stash in a way the patch does not
/// fit, it refuses and changes nothing, so it never overwrites anything (ADR-084).
fn apply_cmd(patch: &str) -> String {
    format!("git apply --whitespace=nowarn {}", sh(patch))
}

/// The second step of a hint, for when the apply refuses (ADR-084): the files changed after the stash (hook fixes, or
/// later edits). Their current state is saved first (`git diff --binary <tree>`: worktree against the stash-time
/// tree), then only the worktree copies of the patch's files go back to the stash-time tree (`git restore
/// --worktree` leaves the index alone), and `again` says what to run then. A list too long to spell out is named by
/// its kept file `paths_file` (D-DATA-19: a whole-worktree restore would overwrite changes git cannot see, in files the
/// patch does not touch); only without one (an unparsable patch, an unwritable store) is the whole worktree restored.
fn push_fallback(
    s: &mut String,
    patch: &str,
    tree: &str,
    existing: Option<&[&[u8]]>,
    paths_file: Option<&Path>,
    again: &str,
) {
    s.push_str("; if git apply reports that the patch does not apply, those files changed after the stash (hook fixes \
                or later edits): keep a copy with git diff --binary ");
    s.push_str(tree);
    s.push_str(" > ");
    s.push_str(&sh(&format!("{patch}.now")));
    s.push_str(", put them back with ");
    if let Some(paths) = existing {
        s.push_str("git --literal-pathspecs restore --source=");
        s.push_str(tree);
        s.push_str(" --worktree --");
        push_paths(s, paths);
    } else if let Some(f) = paths_file {
        s.push_str("git --literal-pathspecs restore --source=");
        s.push_str(tree);
        s.push_str(" --worktree --pathspec-file-nul --pathspec-from-file=");
        s.push_str(&sh(&shown(f)));
    } else {
        s.push_str("git restore --source=");
        s.push_str(tree);
        s.push_str(" --worktree -- :/");
    }
    s.push_str(", then ");
    s.push_str(again);
}

/// The patch's paths that exist in the stash-time tree (`None` for an unparsed or long list: the kept list of
/// `recovery::kept_paths_file`, else the whole worktree).
fn existing_paths(patch_paths: Option<&[(Vec<u8>, bool)]>) -> Option<Vec<&[u8]>> {
    let touched = patch_paths?;
    let existing: Vec<&[u8]> = touched
        .iter()
        .filter(|(_, new)| !new)
        .map(|(p, _)| p.as_slice())
        .collect();
    short_list(&existing).then_some(existing)
}

/// D-DATA-01 (K+, ADR-045/ADR-067/ADR-084): the stderr line after the envelope of a failed restore.
///
/// `[ERROR] Your unstaged changes are saved in <patch>; restore them with: git apply --whitespace=nowarn <patch>`,
/// plus, with the stash-time tree known, the fallback for a worktree the rollback could not reset (see
/// [`push_fallback`]). Never `--3way`: it implies `--index`, so it stages the user's unstaged changes, and it refuses
/// every path whose worktree differs from the index — the very state a failed rollback leaves.
pub(crate) fn k_plus_hint(
    patch: &Path,
    tree: Option<&str>,
    patch_paths: Option<&[(Vec<u8>, bool)]>,
    paths_file: Option<&Path>,
) -> String {
    let p = shown(patch);
    let mut s = format!(
        "[ERROR] Your unstaged changes are saved in {p}; restore them with: {}",
        apply_cmd(&p)
    );
    if let Some(t) = tree {
        let existing = existing_paths(patch_paths);
        if existing.as_ref().is_none_or(|e| !e.is_empty()) {
            push_fallback(
                &mut s,
                &p,
                t,
                existing.as_deref(),
                paths_file,
                "apply it again",
            );
        }
    }
    s
}

/// A path list longer than this many bytes, or than [`LIST_MAX_PATHS`] paths, is not spelled out in a hint.
const LIST_MAX_BYTES: usize = 4096;
/// See [`LIST_MAX_BYTES`].
pub(crate) const LIST_MAX_PATHS: usize = 64;

/// Whether `paths` is short enough to be spelled out in a hint.
pub(crate) fn short_list(paths: &[&[u8]]) -> bool {
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
    /// A kept NUL-separated file listing the patch's paths (used when they are too many to spell out, D-DATA-19).
    pub(crate) paths_file: Option<&'a Path>,
    /// The paths `stage_fixed` staged (or was about to stage).
    pub(crate) staged_fixed: &'a [Vec<u8>],
    /// Whether the run used the worktree's own index (not a temporary `GIT_INDEX_FILE`, as `git commit -- <paths>`
    /// sets): only then is an index entry reset.
    pub(crate) own_index: bool,
    /// The intent-to-add paths the run cleared from the index.
    pub(crate) ita: &'a [Vec<u8>],
    /// A kept NUL-separated file listing `ita` (used when the list is too long to spell out).
    pub(crate) ita_file: Option<&'a Path>,
}

/// D-DATA-13 (ARCHITECTURE §4.6, ADR-084): the next-run recovery notice for one orphaned journal.
///
/// `Restore with: git apply --whitespace=nowarn <patch>` (worktree only, all-or-nothing: nothing changed since the
/// interrupted run is ever overwritten), then — own index only — `&& git --literal-pathspecs reset -q <tree> --
/// <staged paths…>` for the paths `stage_fixed` had staged (`git read-tree <tree>` for a long list), then the
/// intent-to-add re-add, then the fallback of [`push_fallback`] for a refused apply.
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
    s.push_str(&apply_cmd(&p));
    if let Some(t) = r.tree
        && r.own_index
        && !r.staged_fixed.is_empty()
    {
        let staged: Vec<&[u8]> = r.staged_fixed.iter().map(Vec::as_slice).collect();
        if short_list(&staged) {
            s.push_str(" && git --literal-pathspecs reset -q ");
            s.push_str(t);
            s.push_str(" --");
            push_paths(&mut s, &staged);
        } else {
            s.push_str(" && git read-tree ");
            s.push_str(t);
        }
    }
    push_ita(&mut s, r.ita, r.ita_file);
    if let Some(t) = r.tree {
        let existing = existing_paths(r.patch_paths);
        if existing.as_ref().is_none_or(|e| !e.is_empty()) {
            push_fallback(
                &mut s,
                &p,
                t,
                existing.as_deref(),
                r.paths_file,
                "run the command again",
            );
        }
    }
    s
}

/// ` && git --literal-pathspecs add --intent-to-add -- <paths…>`, or, for a long list kept in `file`, ` && git
/// --literal-pathspecs add --intent-to-add --pathspec-file-nul --pathspec-from-file=<file>`.
fn push_ita(s: &mut String, ita: &[Vec<u8>], file: Option<&Path>) {
    if ita.is_empty() {
        return;
    }
    let list: Vec<&[u8]> = ita.iter().map(Vec::as_slice).collect();
    s.push_str(" && git --literal-pathspecs add --intent-to-add");
    match file {
        Some(f) if !short_list(&list) => {
            s.push_str(" --pathspec-file-nul --pathspec-from-file=");
            s.push_str(&sh(&shown(f)));
        }
        _ => {
            s.push_str(" --");
            push_paths(s, &list);
        }
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
/// patch (state `started`), or its unstaged changes are back: only the intent-to-add marks are left. Printed once; a
/// long list is named by its kept list file (`file`), which outlives the journal (ADR-084).
pub(crate) fn ita_notice(ita: &[Vec<u8>], file: Option<&Path>) -> String {
    let mut s = "snp: an earlier run was interrupted before it re-added its intent-to-add files. Restore with: "
        .to_owned();
    let mut tail = String::new();
    push_ita(&mut tail, ita, file);
    s.push_str(tail.trim_prefix(" && "));
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
            paths_file: None,
            staged_fixed,
            own_index: true,
            ita,
            ita_file: Some(Path::new("/s/patches/patch1-2.ita")),
        }
    }

    const HEAD: &str = "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved \
                        in /s/patches/patch1-2 (index tree abc). Restore with: git apply --whitespace=nowarn \
                        /s/patches/patch1-2";
    const FALLBACK: &str = "; if git apply reports that the patch does not apply, those files changed after the stash \
                            (hook fixes or later edits): keep a copy with git diff --binary abc > \
                            /s/patches/patch1-2.now, put them back with ";

    #[test]
    fn k_plus_hints() {
        let p = Path::new("/s/patches/patch1-2");
        assert_eq!(
            k_plus_hint(p, None, None, None),
            "[ERROR] Your unstaged changes are saved in /s/patches/patch1-2; restore them with: git apply \
             --whitespace=nowarn /s/patches/patch1-2"
        );
        let spaced = Path::new("/h o/patch1-2");
        assert_eq!(
            k_plus_hint(spaced, None, None, None),
            "[ERROR] Your unstaged changes are saved in /h o/patch1-2; restore them with: git apply \
             --whitespace=nowarn '/h o/patch1-2'"
        );
        let foo = [(b"foo".to_vec(), false), (b"new".to_vec(), true)];
        assert_eq!(
            k_plus_hint(p, Some("abc"), Some(&foo), None),
            format!(
                "[ERROR] Your unstaged changes are saved in /s/patches/patch1-2; restore them with: git apply \
                 --whitespace=nowarn /s/patches/patch1-2{FALLBACK}git --literal-pathspecs restore --source=abc \
                 --worktree -- foo, then apply it again"
            )
        );
        // Never `--3way` (it implies `--index`: it would stage the unstaged changes).
        assert!(!k_plus_hint(p, Some("abc"), None, None).contains("3way"));
        // D-DATA-19: a list too long to spell out is named by its kept file, never the whole worktree.
        let long: Vec<(Vec<u8>, bool)> = (0..=LIST_MAX_PATHS)
            .map(|i| (format!("f{i}").into_bytes(), false))
            .collect();
        let kept = Path::new("/s/patches/patch1-2.paths");
        assert_eq!(
            k_plus_hint(p, Some("abc"), Some(&long), Some(kept)),
            format!(
                "[ERROR] Your unstaged changes are saved in /s/patches/patch1-2; restore them with: git apply \
                 --whitespace=nowarn /s/patches/patch1-2{FALLBACK}git --literal-pathspecs restore --source=abc \
                 --worktree --pathspec-file-nul --pathspec-from-file=/s/patches/patch1-2.paths, then apply it again"
            )
        );
        assert!(k_plus_hint(p, Some("abc"), Some(&long), None).contains(" --worktree -- :/, then"));
    }

    #[test]
    fn recovery_hints() {
        let p = Path::new("/s/patches/patch1-2");
        let foo = [(b"foo".to_vec(), false)];
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), Some(&foo), &[], &[])),
            format!(
                "{HEAD}{FALLBACK}git --literal-pathspecs restore --source=abc --worktree -- foo, then run the \
                 command again"
            )
        );
        assert_eq!(
            recovery_notice(&rec(p, None, Some(&foo), &[], &[])),
            "snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in \
             /s/patches/patch1-2. Restore with: git apply --whitespace=nowarn /s/patches/patch1-2"
        );
        let staged = [b"a*.txt".to_vec(), b"d/b c".to_vec()];
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), Some(&foo), &staged, &[b"n".to_vec()])),
            format!(
                "{HEAD} && git --literal-pathspecs reset -q abc -- 'a*.txt' 'd/b c' && git --literal-pathspecs add \
                 --intent-to-add -- n{FALLBACK}git --literal-pathspecs restore --source=abc --worktree -- foo, then \
                 run the command again"
            )
        );
        // A temporary index (`git commit -- <paths>`): the real index is never reset.
        let temp = Recovery {
            own_index: false,
            ..rec(p, Some("abc"), Some(&foo), &staged, &[])
        };
        assert!(!recovery_notice(&temp).contains("reset"));
        let many: Vec<Vec<u8>> = (0..65).map(|i| format!("f{i}").into_bytes()).collect();
        let whole = format!(
            "{FALLBACK}git restore --source=abc --worktree -- :/, then run the command again"
        );
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), None, &many, &[])),
            format!("{HEAD} && git read-tree abc{whole}")
        );
        let many_touched: Vec<(Vec<u8>, bool)> = many.iter().map(|p| (p.clone(), false)).collect();
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), Some(&many_touched), &[], &[])),
            format!("{HEAD}{whole}")
        );
        // D-DATA-19: with the kept list, a long list is restored by name, never as the whole worktree.
        let kept = Recovery {
            paths_file: Some(Path::new("/s/patches/patch1-2.paths")),
            ..rec(p, Some("abc"), Some(&many_touched), &[], &[])
        };
        assert_eq!(
            recovery_notice(&kept),
            format!(
                "{HEAD}{FALLBACK}git --literal-pathspecs restore --source=abc --worktree --pathspec-file-nul \
                 --pathspec-from-file=/s/patches/patch1-2.paths, then run the command again"
            )
        );
        // A path the patch creates is not restored (it is not in the tree); a patch that only creates files has no
        // fallback.
        let created = [(b"new".to_vec(), true), (b"foo".to_vec(), false)];
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), Some(&created), &[], &[])),
            format!(
                "{HEAD}{FALLBACK}git --literal-pathspecs restore --source=abc --worktree -- foo, then run the \
                 command again"
            )
        );
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), Some(&created[..1]), &[], &[])),
            HEAD
        );
        // A long intent-to-add list is read from its kept list file.
        assert_eq!(
            recovery_notice(&rec(p, Some("abc"), None, &[], &many)),
            format!(
                "{HEAD} && git --literal-pathspecs add --intent-to-add --pathspec-file-nul \
                 --pathspec-from-file=/s/patches/patch1-2.ita{whole}"
            )
        );
        assert_eq!(
            ita_notice(&[b"n".to_vec(), b"a b".to_vec()], None),
            "snp: an earlier run was interrupted before it re-added its intent-to-add files. Restore with: git \
             --literal-pathspecs add --intent-to-add -- n 'a b'"
        );
        assert_eq!(
            ita_notice(&many, Some(Path::new("/r/.git/snp-stash.journal.ita"))),
            "snp: an earlier run was interrupted before it re-added its intent-to-add files. Restore with: git \
             --literal-pathspecs add --intent-to-add --pathspec-file-nul \
             --pathspec-from-file=/r/.git/snp-stash.journal.ita"
        );
        // Without a kept file the long list is spelled out.
        assert!(ita_notice(&many, None).ends_with(" f63 f64"));
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
