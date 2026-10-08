# D-DIAG-04

stash notices: the next-run recovery notice (§4.6), snp: waiting for another snp run in this worktree…, snp: restoring stashed changes, please wait, snp: git ne…

* **Class:** DIAG (COMPATIBILITY §5.8)
* **Owner:** WP-23; the first package of the register row owns this file and
  `tests/differential/divergences/D-DIAG-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.8)

| Diagnostic (stderr only) |
|---|
| stash notices: the next-run recovery notice (§4.6), `snp: waiting for another snp run in this worktree…`, `snp: restoring stashed changes, please wait`, `snp: git needed the terminal during the restore and was stopped` |

## Rationale

To be written by WP-23: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-23: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DIAG-04.yaml` must keep reproducing the difference.
