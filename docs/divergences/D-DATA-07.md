# D-DATA-07

value repaired (all lines equal and an existing directory ⇒ use one line)

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-07.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| an exported `CDPATH` corrupts `--hook-dir`; legacy hooks (git-lfs pre-push) are silently skipped | value repaired (all lines equal and an existing directory ⇒ use one line) |

## Rationale

To be written by WP-32: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-32: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DATA-07.yaml` must keep reproducing the difference.
