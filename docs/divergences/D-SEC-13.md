# D-SEC-13

as root (euid 0), a store owned by a non-root uid is refused with An error has occurred: FatalError: snp refuses to run as root from a store owned by uid <n>:…

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-13.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| a store writable by another user is used as is, including when running as root (tools and environments that user can replace get executed) | as root (euid 0), a store owned by a non-root uid is refused with `An error has occurred: FatalError: snp refuses to run as root from a store owned by uid <n>: <path>; set SNP_ALLOW_FOREIGN_STORE=1 to trust it` (exit 1) unless `SNP_ALLOW_FOREIGN_STORE=1`; other foreign owners only warn (D-DIAG-05) |

## Rationale

To be written by WP-22: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-22: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-SEC-13.yaml` must keep reproducing the difference.
