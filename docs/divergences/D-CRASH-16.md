# D-CRASH-16

falls back to pipes

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-15; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-16.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| pty allocation failure in colour mode → exit 3 (X13-EXE-06) | falls back to pipes |

## Rationale

To be written by WP-15: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-15: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-16.yaml` must keep reproducing the difference.
