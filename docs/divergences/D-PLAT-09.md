# D-PLAT-09

partitions re-split

* **Class:** PLAT (COMPATIBILITY §5.5)
* **Owner:** WP-15; the first package of the register row owns this file and
  `tests/differential/divergences/D-PLAT-09.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.5)

| pre-commit 4.6.2 | snp |
|---|---|
| Windows: `.cmd` partitions can exceed 8191 characters after quoting (crash) | partitions re-split |

## Rationale

To be written by WP-15: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-15: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-PLAT-09.yaml` must keep reproducing the difference.
