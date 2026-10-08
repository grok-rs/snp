# D-MISC-06

as stated (corpus frequency 0)

* **Class:** TIERB (COMPATIBILITY §5.9)
* **Owner:** WP-13; the first package of the register row owns this file and
  `tests/differential/divergences/D-MISC-06.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.9)

| pre-commit 4.6.2 | snp |
|---|---|
| regex corner cases (01 Addendum §A.6.13 D1–D6): IGNORECASE backreferences across `K`/`k`, `ſ`/`s`, `ς`/`σ`; code points unassigned in UCD 15.0 treated as UCD 16; `\N{…}` without the `py-re-names` feature; `(?L)` under a single-byte non-UTF-8 locale; patterns too large for the engines (`Unsupported`, reported as the validation error plus a stderr hint) | as stated (corpus frequency 0) |

## Rationale

To be written by WP-13: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-13: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-MISC-06.yaml` must keep reproducing the difference.
