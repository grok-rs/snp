# D-CRASH-21

treated as "not rootless"

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-25; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-21.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| `docker system info` fails during the rootless probe → `CalledProcessError` exit 3 | treated as "not rootless" |

## Rationale

To be written by WP-25: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-25: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-21.yaml` must keep reproducing the difference.
