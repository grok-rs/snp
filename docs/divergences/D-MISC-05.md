# D-MISC-05

An invalid pygrep entry fails the hook once, with the last line of Python's traceback.

* **Class:** MISC (COMPATIBILITY §5.9)
* **Owner:** WP-26 (`snp_hooks::pygrep::prepare`); the first package of the register row owns this file and
  `tests/differential/divergences/D-MISC-05.yaml` (WBS "Divergence files", ADR-071).
* **Catalog:** 01 Addendum §A.4.5, X13-EXE-07 (13 X.4.5); tests `x13_exe_07_non_re_error_entries` and the
  `invalid-*` rows of `pygrep_goldens` (`crates/snp-hooks/tests/pygrep/main.rs`).

## Register entry (COMPATIBILITY §5.9)

| pre-commit 4.6.2 | snp |
|---|---|
| invalid pygrep entry: one full traceback per xargs partition | one line in Python's form (`re.error: … at position N`), exit 1 |

## Rationale

pre-commit runs `python -m pre_commit.languages.pygrep *args entry *files` once per xargs partition, and the entry is
never validated as a regex (`entry` is not a `check_regex` key). An entry that `re.compile` rejects therefore prints a
complete Python traceback — about ten lines naming the interpreter's own files — **once per partition**: 19 identical
tracebacks for 40 000 files (01 Addendum §A.4.5). Each ends in the same line: `re.PatternError: missing ),
unterminated subpattern at position 0` on CPython 3.13 (`re.error: …` up to 3.12), `OverflowError: the repetition
number is too large`, `ValueError: ASCII and LOCALE flags are incompatible`, `RecursionError: maximum recursion depth
exceeded`. The hook fails with `- exit code: 1`.

The native compiles the pattern once per hook per run (`pygrep::prepare`) and reports an invalid one once, as that
final line plus a newline, exit 1, without touching any file. The pass/fail result and the exit code are pre-commit's;
the error text is the part of pre-commit's output that identifies the problem; what is dropped are the
machine-specific frames and the repetitions. The tracebacks cannot be byte-identical in any case (they contain the
user's venv paths), so this is a Tier B message difference (01 Addendum §A.4.5 "snp rule"). The class name follows the
emulated CPython 3.13 (`re.PatternError`); the message text and its `at position N` / `(line L, column C)` suffix are
CPython's exactly (snp-pyre).

## Examples

`.pre-commit-config.yaml` with a local hook `id: grep`, `language: pygrep`, `entry: '('`, run on a repository whose
file list makes two xargs partitions:

pre-commit 4.6.2 (`- exit code: 1`):

```
Traceback (most recent call last):
  File "<frozen runpy>", line 198, in _run_module_as_main
  …
re.PatternError: missing ), unterminated subpattern at position 0
Traceback (most recent call last):
  File "<frozen runpy>", line 198, in _run_module_as_main
  …
re.PatternError: missing ), unterminated subpattern at position 0
```

snp (`- exit code: 1`):

```
re.PatternError: missing ), unterminated subpattern at position 0
```

The scenario in `tests/differential/divergences/D-MISC-05.yaml` runs this hook over enough files for several
partitions.

## Related hook-time regex messages (pygrep, no-commit-to-branch)

Two further hook-time regex texts of the natives follow from the same rule (the problem is reported in snp's own
one-line form because Python's text names interpreter files). Until the integration lead extends the register row
(D-MISC-04 covers only cfgv's config-time warnings), they are documented here:

* **`FutureWarning`s** (`[[`, `[a&&b]`, …). Python writes
  `<site-packages>/pre_commit/languages/pygrep.py:123: FutureWarning: Possible nested set at position 1` plus the
  source line `  pattern = re.compile(args.pattern.encode(), flags)` to stderr, which the runner merges into the
  hook output, once per process (once per xargs partition); no-commit-to-branch prints
  `…/no_commit_to_branch.py:23: FutureWarning: …` plus `  re.match(p, branch_name) for p in patterns` for each
  pattern it compiles. snp writes the D-MISC-04 form at the same place in the output (before any match line):
  `[WARNING] regex '[[x]': Possible nested set at position 1 (Python FutureWarning)`. no-commit-to-branch honours
  `SNP_PYTHON_WARNINGS=0`; `pygrep::run_partition` has no environment in its frozen signature (ARCH §1.4.19), so
  pygrep always prints them. Exit codes and match lines are unchanged.
* **Backtracking limit** (D-SEC-10). A pattern that exceeds snp-pyre's limit (CPython would hang) ends the
  partition with `snp: regular expression {pattern!r} exceeded the backtracking limit on {path!r}`, exit 1, after
  the match lines printed so far. no-commit-to-branch fails **closed** the same way (`… on {branch!r}`, exit 1): a
  hanging CPython never let the commit through either.
