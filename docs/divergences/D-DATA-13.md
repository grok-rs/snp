# D-DATA-13

per-worktree lock (nested runs exempt via token only while the lock is held), durable journal + git-dir pointer, patch re-verified before the checkout, next-run recovery notice…

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-13.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-23 (`snp_stash::{enter, recovery_scan, find_unrestored, Unrestored::hint}`).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| two concurrent runs in one worktree can stash over each other; a killed run leaves no recovery hint | per-worktree lock (nested runs exempt via token only while the lock is held), durable journal + git-dir pointer, patch re-verified before the checkout, next-run recovery notice on stderr (`git apply --3way`) |

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
  (index tree <tree>). Restore with: git apply --3way <patch>` — preceded by `git reset -q <tree> -- <paths…> && `
  when `stage_fixed` had already staged fixes. Nothing is ever applied automatically.

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
snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in /s/patches/patch1791403656-1781218 (index tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904). Restore with: git apply --3way /s/patches/patch1791403656-1781218
```

Proof: `crates/snp-stash/tests/behaviour/{guard,recovery}.rs` (lock, nested token, stale token, autosave, patch
names, pointer with another store), `tests/driver.rs::kill_9_at_every_transition_…` (the next run prints exactly
`Unrestored::hint`, and the hinted commands restore the work); the scenario
`tests/differential/divergences/D-DATA-13.yaml`.
