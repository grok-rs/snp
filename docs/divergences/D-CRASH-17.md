# D-CRASH-17

marker-based store never needs repair; commands that do not need the store work; the error handler never double-faults

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-22 (contributing: WP-33); the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-17.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| store not creatable/corrupt (stale db rows, locked `db.db`, gc ENOENT, double faults in the error handler: X13-ST-*, X13-CMD-07, X13-EH-01/02) → exit 3 / tracebacks | marker-based store never needs repair; commands that do not need the store work; the error handler never double-faults |

## Rationale

To be written by WP-22: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-22: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-17.yaml` must keep reproducing the difference.
