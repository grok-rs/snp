# D-SEC-10

linear tier 1; tier 2 backtrack_limit 1e8 → An error has occurred: snp: regular expression {pattern!r} exceeded the backtracking limit on {path!r} (exit 1)

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-13; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-10.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| catastrophic regexes hang (tier-1-able patterns are exponential in CPython; backtracking patterns unbounded) | linear tier 1; tier 2 `backtrack_limit` 1e8 → `An error has occurred: snp: regular expression {pattern!r} exceeded the backtracking limit on {path!r}` (exit 1) |

## Rationale

To be written by WP-13: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-13: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-SEC-10.yaml` must keep reproducing the difference.
