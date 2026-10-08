# D-CRASH-24

silent, exit 120, stash restored

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-33 (contributing: WP-00); the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-24.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| stdout closed (EPIPE) → chained Python tracebacks on stderr, exit 120 (X13-EH-03) | silent, exit 120, stash restored |

## Rationale

To be written by WP-33: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-33: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-24.yaml` must keep reproducing the difference.
