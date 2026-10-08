# D-CRASH-25

UTF-8 always (see D-DATA-09)

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-25.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| `migrate-config` on a list-root / non-UTF-8 config under a non-UTF-8 locale (Windows cp1252) → crash or mis-decode | UTF-8 always (see D-DATA-09) |

## Rationale

To be written by WP-21: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-21: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-25.yaml` must keep reproducing the difference.
