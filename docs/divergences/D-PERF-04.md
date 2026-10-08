# D-PERF-04

skipped when the tagged image exists (SNP_DOCKER_ALWAYS_BUILD=1 restores it)

* **Class:** PERF (COMPATIBILITY §5.6)
* **Owner:** WP-25; the first package of the register row owns this file and
  `tests/differential/divergences/D-PERF-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.6)

| pre-commit 4.6.2 | snp |
|---|---|
| docker hooks run `docker build` every run | skipped when the tagged image exists (`SNP_DOCKER_ALWAYS_BUILD=1` restores it) |

## Rationale

To be written by WP-25: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-25: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-PERF-04.yaml` must keep reproducing the difference.
