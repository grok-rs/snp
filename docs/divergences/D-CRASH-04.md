# D-CRASH-04

validation error '<repr>' is not a valid python regex, exit 1

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-21 (contributing: WP-13); the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| regex repeat ≥ 4294967295, nesting ≥ 485, `(?a)(?u)` in `files`/`exclude` → `OverflowError`/`RecursionError`/`ValueError` exit 3 (X13-CFG-05…07, 13) | validation error `'<repr>' is not a valid python regex`, exit 1 |

## Rationale

To be written by WP-21: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-21: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-04.yaml` must keep reproducing the difference.
