# D-SEC-06

one retry with the other --object-format after mismatched algorithms

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-18; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-06.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| clones fail when the user's git defaults to SHA-256 objects (hazard d) | one retry with the other `--object-format` after `mismatched algorithms` |

## Rationale

To be written by WP-18: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-18: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-SEC-06.yaml` must keep reproducing the difference.
