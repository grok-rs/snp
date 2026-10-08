# D-CRASH-01

validation error leaf invalid literal for int() with base 10: 'abc', exit 1

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| `minimum_pre_commit_version: "abc"` (also `""`, `"4."`, `"4.6.3rc1"`) → `ValueError` exit 3 (X13-CFG-01) | validation error leaf `invalid literal for int() with base 10: 'abc'`, exit 1 |

## Rationale

To be written by WP-21: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-21: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-01.yaml` must keep reproducing the difference.
