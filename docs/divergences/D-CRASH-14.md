# D-CRASH-14

stderr message, legacy rc 1, snp's hooks still run

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-14.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| broken legacy hook (missing shebang interpreter, directory) → exit 3 (X13-HI-07) | stderr message, legacy rc 1, snp's hooks still run |

## Rationale

To be written by WP-32: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-32: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-14.yaml` must keep reproducing the difference.
