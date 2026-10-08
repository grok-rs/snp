# D-SEC-05

installers and owned repos get pre-commit's no_git_env allow-list

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-18 (contributing: WP-24, WP-25); the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-05.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| installers and snp-owned repo operations inherit `GIT_DIR`/`GIT_INDEX_FILE` (index corruption in linked worktrees; autoupdate writes partial-clone config into the user's repo — hazard c) | installers and owned repos get pre-commit's `no_git_env` allow-list |

## Rationale

To be written by WP-18: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-18: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-SEC-05.yaml` must keep reproducing the difference.
