# D-SEC-11

depth 512, 1 M constructed nodes, 16 MiB file limits → YAML error

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-14; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-11.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| YAML alias bombs / deep nesting / huge files exhaust memory or the stack | depth 512, 1 M constructed nodes, 16 MiB file limits → YAML error |

## Rationale

To be written by WP-14: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-14: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-SEC-11.yaml` must keep reproducing the difference.
