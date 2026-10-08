# D-DATA-02

descendants of the cancelled hooks are killed (Linux subreaper, Windows job), then the stash is restored (restore messages printed), then death by the same…

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23 (contributing: WP-15); the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| SIGTERM/SIGHUP kill pre-commit without restoring the stash (unstaged work only in the patch) | descendants of the cancelled hooks are killed (Linux subreaper, Windows job), then the stash is restored (restore messages printed), then death by the same signal (exit status 143/129 as before). Residual risk, documented: on macOS an orphaned descendant of an Inherit-group hook (e.g. a formatter started by a shell entry) cannot be found and may still write after the restore; the patch file is kept, so the work stays recoverable |

## Rationale

To be written by WP-23: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-23: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DATA-02.yaml` must keep reproducing the difference.
