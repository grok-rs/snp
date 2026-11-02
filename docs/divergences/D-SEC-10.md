# D-SEC-10

Catastrophic regular expressions cannot hang snp: tier 1 is linear, tier 2 stops after 10⁸ backtracking steps with
an error.

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-13 (`snp_pyre::PyRegex`, `MatchError::Backtrack`); the run-time message is printed by the file
  filter (WP-22/WP-32) and by pygrep (WP-26). The first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-10.yaml` (WBS "Divergence files", ADR-071).

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.7.0 | snp |
|---|---|
| catastrophic regexes hang (tier-1-able patterns are exponential in CPython; backtracking patterns unbounded) | linear tier 1; tier 2 `backtrack_limit` 1e8 → `An error has occurred: snp: regular expression {pattern!r} exceeded the backtracking limit on {path!r}` (exit 1) |

## Rationale

pre-commit matches `files`, `exclude`, `types`-independent filters and pygrep entries with CPython's `re`
(`pre_commit/commands/run.py:60-70`, `pre_commit/languages/pygrep.py`), a backtracking engine without any step or
time limit. A pattern such as `^(.+/)*\.pre-commit-config\.yaml$` is exponential in the depth of a non-matching path
and `(?:(?=a)a+)+(?<!a)b` in the length of a run of `a`; a hostile `.pre-commit-config.yaml` in a pull request (or
a hostile file name meeting an innocent-looking pattern) makes `pre-commit run` spin for minutes or forever
(01 Addendum §A.6.13 D7, D8).

snp lowers every pattern it can to the `regex` crate (tier 1, 01 Addendum §A.6.9), which guarantees linear-time
matching: D8 patterns give CPython's answer without the exponential blow-up — no visible difference except time.
Patterns that need backtracking (look-around, back-references, conditionals, atomic groups, possessive repeats)
run on `fancy-regex` (tier 2) behind a mandatory linear prefilter (§A.6.11) and with `backtrack_limit` = 10⁸
(≈ 2.5 s on one core). Exceeding it is `MatchError::Backtrack`; for `files`/`exclude` it aborts the run with the
message above and exit 1 — never "not matched", which could silently skip a security hook (01 Addendum §A.10 Q6,
DECISIONS Appendix B). For pygrep the hook fails with the same message. Real configurations stay several orders of
magnitude below the limit (corpus frequency 0): a regex that takes 10⁸ steps on one file name is a bug or an attack.

## Examples

A local hook `language: fail`, `entry: the files pattern matched`, `files: '(?:(?=a)a+)+(?<!a)b'`; the repository
holds the empty file `aaaaaaaaaaaaaaaaaaaaaaaaaab` (26 × `a` + `b`) and the config; `run --all-files`.

pre-commit 4.7.0 (CPython 3.12.3; bytes captured with the same `fail`/filter code of pre-commit 4.1.0 on 3.12.3):
`re.search` explores 2²⁵ partitions of the `a` run for ≈ 5.4 s, finds no match and prints, exit 0:

```text
matched..............................................(no files to check)Skipped
```

Each further `a` doubles the time (30 × `a`: ≈ 90 s; 40 × `a`: about a day).

snp: the relaxed prefilter `(?:a+)+b` matches, the backtracking engine hits the 10⁸-step limit after ≈ 2.5 s
(`MatchError::Backtrack`; from 26 × `a` on, independent of the length) and the run aborts, exit 1:

```text
An error has occurred: snp: regular expression '(?:(?=a)a+)+(?<!a)b' exceeded the backtracking limit on 'aaaaaaaaaaaaaaaaaaaaaaaaaab'
```

The tier-1 half (D8, same answer, different time): `files: '^(.+/)*\.pre-commit-config\.yaml$'` with a file at
`d/d/…/d/x` (5 000 levels deep) never finishes in CPython; snp answers "not matched" in milliseconds
(`crates/snp-pyre/tests/backtrack.rs`). The 10⁸-step stop is tested by
`crates/snp-pyre/tests/perf.rs::perf_catastrophic_tier2_pattern_hits_the_backtrack_limit`.
