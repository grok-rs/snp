# D-MISC-07

only the final traceback line (pre_commit.parse_shebang.ExecutableNotFoundError: Executable x not found)

* **Class:** TIERB (COMPATIBILITY §5.9)
* **Owner:** WP-26; the first package of the register row owns this file and
  `tests/differential/divergences/D-MISC-07.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.9)

| pre-commit 4.6.2 | snp |
|---|---|
| in-hook hazmat exec failure: full Python traceback | only the final traceback line (`pre_commit.parse_shebang.ExecutableNotFoundError: Executable `x` not found`) |

## Rationale

To be written by WP-26: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-26: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-MISC-07.yaml` must keep reproducing the difference.
