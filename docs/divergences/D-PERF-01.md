# D-PERF-01

uv (--seed) with pip-config translation and a pip fallback; seeded packages may differ in version, and uv's resolver can differ on pre-releases, yanked versions…

* **Class:** PERF (COMPATIBILITY §5.6)
* **Owner:** WP-24; the first package of the register row owns this file and
  `tests/differential/divergences/D-PERF-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.6)

| pre-commit 4.6.2 | snp |
|---|---|
| Python envs built with virtualenv + pip | uv (`--seed`) with pip-config translation and a pip fallback; seeded packages may differ in version, and uv's resolver can differ on pre-releases, yanked versions and extra-index strategy (FD-7); `SNP_PYTHON_INSTALLER=pip` restores pip's resolver per env (ADR-073) |

## Rationale

To be written by WP-24: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-24: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-PERF-01.yaml` must keep reproducing the difference.
