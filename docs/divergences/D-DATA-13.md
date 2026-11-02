# D-DATA-13

per-worktree lock (nested runs exempt via token only while the lock is held), durable journal + git-dir pointer, patch re-verified before the checkout, next-run recovery notice…

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-13.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-23 (`snp_stash::{enter, recovery_scan, find_unrestored, Unrestored::hint}`).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.7.0 | snp |
|---|---|
| two concurrent runs in one worktree can stash over each other; a killed run leaves no recovery hint | per-worktree lock (nested runs exempt via token only while the lock is held), durable journal + git-dir pointer, patch re-verified before the checkout, next-run recovery notice on stderr (`git apply --whitespace=nowarn`, never overwriting later edits) |

## Rationale

pre-commit's stash has no mutual exclusion and no record. Two runs in one worktree (an IDE's commit next to a
terminal one; git ≥ 2.55 parallel hooks) can both `diff-index` the same unstaged changes: the second sees the first's
clean checkout, and whichever restores last decides what the user gets back. A run killed between the checkout and the
restore (SIGKILL, OOM, power loss, a crashed IDE) leaves the work only in `patch<ts>-<pid>`, the file is written
without `fsync`, and nothing ever mentions it again. An editor autosave between `diff-index` and the checkout is
discarded silently.

snp adds, with no change to any byte pre-commit prints on a working path (ARCHITECTURE §4, ADR-016/026/067):

* a per-worktree lock `<git_dir>/snp-stash.lock` (linked worktrees have their own); a second run waits up to
  `SNP_STASH_LOCK_TIMEOUT` (60 s) with one stderr notice (D-DIAG-04), then fails with a `FatalError`; a nested run (a
  hook running snp) is exempt only when its `_SNP_STASH_LOCK` (D-EXT-04) equals the token in the lock file **and** the
  lock is really held — a stale token exempts nobody;
* the patch is created `create_new` 0600 (two runs with the same second and pid get `-1`, `-2` …), `fsync`ed, and
  re-verified by a second `diff-index` digest right before the checkout (an autosave is caught: the patch is
  rewritten, at most 3 times, else `FatalError: snp: unstaged files kept changing while stashing; nothing was
  modified`);
* a journal `<store>/patches/run-<pid>-<rand>.snp-journal` and a pointer `<git_dir>/snp-stash.journal`, written before
  every mutation (fsynced for the data-protecting states), deleted when the run is done;
* the next run in the worktree (any store: the pointer is in the git dir) prints, on stderr, for every orphaned
  journal: `snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in <patch>
  (index tree <tree>). Restore with: git apply --whitespace=nowarn <patch>` — pre-commit's own restore command:
  worktree only, and all-or-nothing, so it never overwrites anything that changed since the interrupted run (a hook's
  fix, or work done after the kill): it applies around such edits or refuses and changes nothing (ADR-084). With the
  run's own index (not the temporary `GIT_INDEX_FILE` of `git commit -- <paths>`) it continues `&& git
  --literal-pathspecs reset -q <tree> -- <stage_fixed paths…>` (`&& git read-tree <tree>` for a long list), then
  re-adds the intent-to-add paths the run cleared (`&& git --literal-pathspecs add --intent-to-add -- <paths…>`; a
  list of more than 64 paths or 4096 bytes is kept in `<patch>.ita`, NUL-separated, and read with
  `--pathspec-file-nul --pathspec-from-file=<patch>.ita`). The notice then says what to do when the apply refuses:
  `; if git apply reports that the patch does not apply, those files changed after the stash (hook fixes or later
  edits): keep a copy with git diff --binary <tree> > <patch>.now, put them back with git --literal-pathspecs restore
  --source=<tree> --worktree -- <patch paths…>, then run the command again` (`git restore --source=<tree> --worktree
  -- :/` for a long or unparseable list). Paths are literal pathspecs; the commands are run from the top of the
  worktree. A run killed after clearing intent-to-add entries but with no patch at risk (before the patch, or after
  the restore) gets one notice `snp: an earlier run was interrupted before it re-added its intent-to-add files.
  Restore with: git --literal-pathspecs add --intent-to-add -- <paths…>` (or the kept-list form), printed once.
  Nothing is ever applied automatically.
* the notices are printed once per run, at its start (before any preflight check can end it), followed by `snp: this
  notice repeats until those changes are back in the worktree; to dismiss it and keep the patch, run: snp doctor
  --fix --stash`. Once the patch's changes are back (`git -c apply.ignoreWhitespace=no apply --check --reverse
  --whitespace=nowarn <patch>` succeeds and every mode the patch sets is the worktree file's: the user ran the printed
  command), the next run retires the journal and says so once: `snp: the unstaged changes saved in <patch>
  are back in the worktree; the recovery notice of that interrupted run is retired`. `snp doctor --fix --stash`
  dismisses every orphan of the worktree explicitly. Neither deletes the patch (ADR-083).

Why it is safe: stdout and exit codes of every run that works in pre-commit are unchanged; the only visible
differences are waiting (instead of corrupting a concurrent run) and stderr notices about work pre-commit would
silently strand.

## Examples

```text
# a second `snp run` while the first is inside its hooks
$ snp run
snp: waiting for another snp run in this worktree…
… (after SNP_STASH_LOCK_TIMEOUT)
An error has occurred: FatalError: snp: another snp run in this worktree still holds /r/.git/snp-stash.lock after 60 s; set SNP_STASH_LOCK_TIMEOUT (seconds, at most 3600) to wait longer

# the run after a `kill -9` during the hooks (stderr)
snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in /s/patches/patch1791403656-1781218 (index tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904). Restore with: git apply --whitespace=nowarn /s/patches/patch1791403656-1781218; if git apply reports that the patch does not apply, those files changed after the stash (hook fixes or later edits): keep a copy with git diff --binary 4b825dc642cb6eb9a060e54bf8d69288fbee4904 > /s/patches/patch1791403656-1781218.now, put them back with git --literal-pathspecs restore --source=4b825dc642cb6eb9a060e54bf8d69288fbee4904 --worktree -- foo, then run the command again
snp: this notice repeats until those changes are back in the worktree; to dismiss it and keep the patch, run: snp doctor --fix --stash

# the run after the user ran that command (stderr, once)
snp: the unstaged changes saved in /s/patches/patch1791403656-1781218 are back in the worktree; the recovery notice of that interrupted run is retired
```

Proof: `crates/snp-stash/tests/behaviour/{guard,recovery}.rs` (lock, nested token, stale token, autosave, patch
names, pointer with another store), `tests/driver.rs::kill_9_at_every_transition_…` (the next run prints exactly
`Unrestored::hint`, and the hinted commands restore the work); the scenario
`tests/differential/divergences/D-DATA-13.yaml`; the dismissal paths:
`crates/snp-stash/tests/behaviour/recovery.rs::{an_applied_orphan_is_retired_once,
dismissal_keeps_the_patch_and_silences_the_notice}` and `crates/snp/tests/it/stash.rs::{
an_orphaned_stash_is_reported_once_on_every_path, following_the_recovery_notice_retires_it,
doctor_fix_stash_dismisses_the_notice_and_keeps_the_patch}`.
