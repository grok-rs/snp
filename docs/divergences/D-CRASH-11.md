# D-CRASH-11

same algorithm on bytes (ASCII whitespace), hooks run

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-18; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-11.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| non-UTF-8 pre-push stdin → `UnicodeDecodeError` exit 3, push blocked (X13-HI-04) | same algorithm on bytes (ASCII whitespace), hooks run |

## Rationale

To be written by WP-18: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-18: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-11.yaml` must keep reproducing the difference.
