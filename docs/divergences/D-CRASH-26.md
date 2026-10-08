# D-CRASH-26

ls-files -z + adjacent dedup (byte-identical list); the real flag is kept on ≥ 2.31 for argv parity in error texts

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-18; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-26.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| git < 2.31: `run --all-files`, the pre-push root push and the meta hooks call `ls-files -z --deduplicate` → `CalledProcessError` (rc 129) exit 3 (a traceback inside the meta hook's output) (02 G.3) | `ls-files -z` + adjacent dedup (byte-identical list); the real flag is kept on ≥ 2.31 for argv parity in error texts |

## Rationale

To be written by WP-18: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-18: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-26.yaml` must keep reproducing the difference.
