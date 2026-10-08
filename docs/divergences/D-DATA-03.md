# D-DATA-03

refused unless -f; a .legacy that is itself a manager script is removed

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-03.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| `install` silently overwrites an existing foreign `.legacy` hook | refused unless `-f`; a `.legacy` that is itself a manager script is removed |

## Rationale

To be written by WP-33: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-33: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DATA-03.yaml` must keep reproducing the difference.
