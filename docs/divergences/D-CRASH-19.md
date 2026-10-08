# D-CRASH-19

uv-managed download into the store (SNP_PYTHON_DOWNLOADS=never restores the failure)

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-24; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-19.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| explicitly requested Python version not installed → install crash exit 3 | uv-managed download into the store (`SNP_PYTHON_DOWNLOADS=never` restores the failure) |

## Rationale

To be written by WP-24: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-24: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-19.yaml` must keep reproducing the difference.
