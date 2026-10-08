# D-PLAT-10

available_parallelism (differs only above 64 logical CPUs; no unsafe)

* **Class:** PLAT (COMPATIBILITY §5.5)
* **Owner:** WP-15; the first package of the register row owns this file and
  `tests/differential/divergences/D-PLAT-10.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.5)

| pre-commit 4.6.2 | snp |
|---|---|
| Windows CPU count = `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)` | `available_parallelism` (differs only above 64 logical CPUs; no `unsafe`) |

## Rationale

To be written by WP-15: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-15: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-PLAT-10.yaml` must keep reproducing the difference.
