# D-DATA-04

gc removes *restored* patches older than 30 days (never unrestored ones) — class STORE

* **Class:** STORE (COMPATIBILITY §5.4)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.4)

| pre-commit 4.6.2 | snp |
|---|---|
| restored stash patches are kept forever | gc removes *restored* patches older than 30 days (never unrestored ones) — class STORE |

## Rationale

To be written by WP-22: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-22: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DATA-04.yaml` must keep reproducing the difference.
