# D-DIAG-03

run hint, only when stderr is a TTY, SNP_NO_HINTS unset/empty and core.hooksPath unset: hint: {hook_path} runs {owner}, not snp; run {prog} install to switch (1…

* **Class:** DIAG (COMPATIBILITY §5.8)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-DIAG-03.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.8)

| Diagnostic (stderr only) |
|---|
| `run` hint, only when stderr is a TTY, `SNP_NO_HINTS` unset/empty and `core.hooksPath` unset: ``hint: {hook_path} runs {owner}, not snp; run `{prog} install` to switch`` (11 §A.14) |

## Rationale

To be written by WP-32: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-32: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DIAG-03.yaml` must keep reproducing the difference.
