# D-DATA-10

the line keeps its \r (DEV-PY-1)

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-10.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| `autoupdate --freeze` rewrites a CRLF `rev:` line with LF (mixed endings) | the line keeps its `\r` (DEV-PY-1) |

## Rationale

To be written by WP-21: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-21: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DATA-10.yaml` must keep reproducing the difference.
