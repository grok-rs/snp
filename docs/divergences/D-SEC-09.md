# D-SEC-09

shebang line capped at 64 KiB (file-identify)

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-15; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-09.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| shebang reads are unbounded | shebang line capped at 64 KiB (file-identify) |

## Rationale

To be written by WP-15: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-15: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-SEC-09.yaml` must keep reproducing the difference.
