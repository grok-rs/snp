# D-DIAG-04

stash notices: the next-run recovery notice (§4.6), snp: waiting for another snp run in this worktree…, snp: restoring stashed changes, please wait, snp: git…

* **Class:** DIAG (COMPATIBILITY §5.8)
* **Owner:** WP-23 (the restore notice is printed by WP-15's signal handler); the first package of the register row
  owns this file and `tests/differential/divergences/D-DIAG-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented (`crates/snp-stash/src/messages.rs`; `snp_proc::messages::RESTORE_NOTICE`).

## Register entry (COMPATIBILITY §5.8)

| Diagnostic (stderr only) |
|---|
| stash notices: the next-run recovery notice (§4.6), `snp: waiting for another snp run in this worktree…`, `snp: restoring stashed changes, please wait`, `snp: git needed the terminal during the restore and was stopped` |

## Rationale

Each notice reports a situation pre-commit cannot be in (it has no lock, journal, signal handler or isolated restore)
or handles silently to the user's loss:

* **recovery notice** — an earlier run never restored; where the work is and the command that restores it
  (D-DATA-13, ARCHITECTURE §4.6). Printed by the next stash in the worktree, once per orphaned journal.
* **`snp: waiting for another snp run in this worktree…`** — printed once when the worktree lock is contended
  (D-DATA-13), so a waiting commit does not look hung.
* **`snp: restoring stashed changes, please wait`** — printed once when a further signal arrives while the restore
  runs under the `RestoreShield` (D-DATA-02): the restore is never abandoned, the user learns why snp does not exit.
* **`snp: git needed the terminal during the restore and was stopped`** — a restore command (e.g. a git-lfs credential
  prompt) read the terminal from its own process group and was stopped by SIGTTIN/SIGTTOU; snp kills it and continues
  with the rollback / K+ path (ADR-065), so the run cannot hang forever on an invisible prompt.

All of them name snp in both brands (snp-only diagnostics, COMPATIBILITY §5.8), go to stderr only, and never change
stdout or the exit code.

## Examples

```text
snp: waiting for another snp run in this worktree…
snp: restoring stashed changes, please wait
snp: git needed the terminal during the restore and was stopped
snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in /s/patches/patch1791403656-1781218 (index tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904). Restore with: git --literal-pathspecs checkout 4b825dc642cb6eb9a060e54bf8d69288fbee4904 -- foo && git apply --3way /s/patches/patch1791403656-1781218 && git --literal-pathspecs reset -q 4b825dc642cb6eb9a060e54bf8d69288fbee4904 -- foo
```

Proof: `crates/snp-stash/src/messages.rs` (goldens), `tests/behaviour/guard.rs::{concurrent_second_run_waits_once_…,
a_tty_stopped_restore_is_reported_…}`, `tests/driver.rs::a_second_ctrl_c_during_git_apply_never_interrupts_it`
(exactly one restore notice). The scenario `tests/differential/divergences/D-DIAG-04.yaml` provokes the recovery
notice (the other three need concurrency or signals and are covered by the process tests).
