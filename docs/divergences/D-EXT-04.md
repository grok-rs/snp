# D-EXT-04

_SNP_STASH_LOCK=<token> is exported to hooks while a stash is held (nested runs skip the worktree lock) — class DATA

* **Class:** DATA (COMPATIBILITY §5.7)
* **Owner:** WP-23; the first package of the register row owns this file and
  `tests/differential/divergences/D-EXT-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.7)

| Divergence |
|---|
| `_SNP_STASH_LOCK=<token>` is exported to hooks while a stash is held (nested runs skip the worktree lock) — class DATA |

## Rationale

To be written by WP-23: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-23: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-EXT-04.yaml` must keep reproducing the difference.
