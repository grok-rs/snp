# D-PERF-03

native implementations (fast path, ADR-031); output byte-identical; Tier A repos are never cloned (FD-8)

* **Class:** PERF (COMPATIBILITY §5.6)
* **Owner:** WP-31 (contributing: WP-26, WP-27); the first package of the register row owns this file and
  `tests/differential/divergences/D-PERF-03.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.6)

| pre-commit 4.6.2 | snp |
|---|---|
| pre-commit-hooks run from a cloned venv | native implementations (fast path, ADR-031); output byte-identical; Tier A repos are never cloned (FD-8) |

## Rationale

To be written by WP-31: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-31: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-PERF-03.yaml` must keep reproducing the difference.
