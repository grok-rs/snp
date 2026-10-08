# D-DATA-11

GIT_DIFF_OPTS removed for the stash diff-index

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23 (contributing: WP-18); the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-11.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| `GIT_DIFF_OPTS=--unified=0` makes the stash patch unrestorable (hazard a) | `GIT_DIFF_OPTS` removed for the stash `diff-index` |

## Rationale

To be written by WP-23: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-23: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DATA-11.yaml` must keep reproducing the difference.
