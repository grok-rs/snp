# D-DIAG-01

git < 2.31: [WARNING] git <V> is older than 2.31.0, the oldest git version snp is tested with. Set SNP_ALLOW_OLD_GIT=1 to silence this warning. (once per…

* **Class:** DIAG (COMPATIBILITY §5.8)
* **Owner:** WP-18; the first package of the register row owns this file and
  `tests/differential/divergences/D-DIAG-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.8)

| Diagnostic (stderr only) |
|---|
| git < 2.31: `[WARNING] git <V> is older than 2.31.0, the oldest git version snp is tested with. Set SNP_ALLOW_OLD_GIT=1 to silence this warning.` (once per process; 02 G.4) |

## Rationale

To be written by WP-18: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-18: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DIAG-01.yaml` must keep reproducing the difference.
