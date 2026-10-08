# D-MISC-02

argv[1] == "__hazmat" is reserved (in-hook hazmat entry, both brands) — class BRAND

* **Class:** BRAND (COMPATIBILITY §5.4)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-MISC-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.4)

| pre-commit 4.6.2 | snp |
|---|---|
| `pre-commit __hazmat …` → `invalid choice: '__hazmat'`, exit 2 | `argv[1] == "__hazmat"` is reserved (in-hook hazmat entry, both brands) — class BRAND |

## Rationale

To be written by WP-33: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-33: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-MISC-02.yaml` must keep reproducing the difference.
