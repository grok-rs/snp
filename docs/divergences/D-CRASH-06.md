# D-CRASH-06

per-repo [{repo}] <error> lines, the others are updated and written, exit 1

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-06.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| `autoupdate`: one unreachable repo → `CalledProcessError` exit 3, nothing written (X13-GIT-02) | per-repo `[{repo}] <error>` lines, the others are updated and written, exit 1 |

## Rationale

To be written by WP-33: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-33: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-06.yaml` must keep reproducing the difference.
