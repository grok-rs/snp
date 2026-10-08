# D-PLAT-03

Rust's BatBadBut-safe quoting; arguments containing CR/LF fail the hook with Executable {cmd0} cannot receive an argument containing a line break: {repr} (rc…

* **Class:** SEC (COMPATIBILITY §5.5)
* **Owner:** WP-15; the first package of the register row owns this file and
  `tests/differential/divergences/D-PLAT-03.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.5)

| pre-commit 4.6.2 | snp |
|---|---|
| Windows: batch-file (`.cmd`/`.bat`) arguments with `&`, `^`, `%VAR%` are interpreted by cmd.exe | Rust's BatBadBut-safe quoting; arguments containing CR/LF fail the hook with ``Executable `{cmd0}` cannot receive an argument containing a line break: {repr}`` (rc 1) — class SEC |

## Rationale

To be written by WP-15: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-15: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-PLAT-03.yaml` must keep reproducing the difference.
