# D-CRASH-23

passes (opens for writing only when a change is needed)

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-26; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-23.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| `end-of-file-fixer` (and the other fixers) on a read-only file that needs no change → crash in the hook | passes (opens for writing only when a change is needed) |

## Rationale

To be written by WP-26: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-26: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-23.yaml` must keep reproducing the difference.
