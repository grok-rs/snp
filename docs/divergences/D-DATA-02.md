# D-DATA-02

descendants of the cancelled hooks are killed (Linux subreaper, Windows job), then the stash is restored (restore messages printed), then death by the same signal…

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23 (with WP-15's signal handling); the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented: WP-15 (`snp_proc::signals`, `registry::quiesce`, Critical spawns), WP-23 (the restore
  under a `RestoreShield`, proven process-level by `snp-stash-driver`); `snp run` wiring is WP-32's.

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.7.0 | snp |
|---|---|
| SIGTERM/SIGHUP kill pre-commit without restoring the stash (unstaged work only in the patch) | descendants of the cancelled hooks are killed (Linux subreaper, Windows job), then the stash is restored (restore messages printed), then death by the same signal (exit status 143/129 as before). Residual risk, documented: on macOS an orphaned descendant of an Inherit-group hook (e.g. a formatter started by a shell entry) cannot be found and may still write after the restore; the patch file is kept, so the work stays recoverable |

## Rationale

pre-commit installs no handler for SIGTERM or SIGHUP: the default action kills the Python process at once, inside the
`staged_files_only` context, so the `finally` that re-applies the patch never runs. A `git commit` cancelled by an IDE,
a CI job timeout, a closed terminal or `kill <pid>` leaves the worktree at the *staged* content; the unstaged work sits
only in `<store>/patch<ts>-<pid>` and nothing tells the user.

snp handles both signals (ARCHITECTURE §4.3, §10.4, ADR-026/065): the run token is cancelled, sent signals are
forwarded to the hooks, the grace expires, leftover descendants of the cancelled hooks are killed (`quiesce`), the
stash is restored with every git command in its own process group (never signalled), the restore messages are
printed, and then snp re-raises the signal with the default disposition, so the parent still sees death by SIGTERM
(143) or SIGHUP (129) — the same status as pre-commit. Only the worktree differs: it is restored.

Why it is safe: the exit status and the absence of a banner are unchanged; the extra stdout lines are pre-commit's own
restore messages; the restore can only put the user's work back. On macOS an orphaned descendant of a hook may still
write after the restore (no subreaper there); the patch stays on disk and the journal names it, so the work remains
recoverable.

## Examples

```text
$ kill -TERM <pid of `pre-commit run`, during a slow hook>
# pre-commit 4.7.0: the process dies (status 143), stdout ends after `slow.....`, foo has the staged content,
# the unstaged change is only in /s/patch1791403833-1793278

$ kill -TERM <pid of `snp run`, during a slow hook>
slow...................................................................[INFO] Restored changes from /s/patches/patch1791403833-1793278.
# status 143 (death by SIGTERM), foo has the user's unstaged change again
```

Process-level proof: `crates/snp-stash/tests/driver.rs` (`sigterm_sent_during_hooks_…`, `sighup_sent_during_hooks_…`,
`sighup_from_a_terminal_hangup_…` for the kernel-origin hangup, `sigterm_at_each_transition_…`). The differential
scenario `tests/differential/divergences/D-DATA-02.yaml` sends SIGTERM to the tool while a hook sleeps.
