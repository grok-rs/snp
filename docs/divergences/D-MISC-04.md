# D-MISC-04

one stderr line per distinct (pattern, warning): [WARNING] regex {pattern!r}: {msg} (Python FutureWarning); SNP_PYTHON_WARNINGS=0 silences

* **Class:** TIERB (COMPATIBILITY §5.9)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-MISC-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`items::Compiled` reports each pattern as its check compiles it, `load::validate_bytes` / `load::future_warnings` print them, `crates/snp-config/src/{items,load}.rs`; text `messages::future_warning`; catalog tests `x13_sm_03_future_warning_stderr_line`, `x13_sm_10_pythonwarnings_has_no_effect`); `SNP_PYTHON_WARNINGS=0` is applied by the caller's `DiagSink` (snp-config never reads the environment, WP-33).

## Register entry (COMPATIBILITY §5.9)

| pre-commit 4.7.0 | snp |
|---|---|
| regex `FutureWarning` lines: `…/cfgv.py:386: FutureWarning: Possible nested set at position 1` + `  re.compile(v)` on stderr | one stderr line per distinct (pattern, warning): `[WARNING] regex {pattern!r}: {msg} (Python FutureWarning)`; `SNP_PYTHON_WARNINGS=0` silences |

## Rationale

CPython 3.7+ emits a `FutureWarning` while compiling a `files`/`exclude` pattern that would change meaning under
future set syntax (`[[`, `&&`, `||`, `~~`, `--` inside a class). Python's warnings machinery prints it on stderr as
two lines naming the interpreter's site-packages path and the source line (`…/cfgv.py:386: FutureWarning:
Possible nested set at position 1` / `  re.compile(v)`), once per process and message text; `PYTHONWARNINGS`
controls it. Those bytes cannot be reproduced without Python (TIERB).

snp keeps the information and drops the interpreter noise: it prints one stderr line per distinct (pattern,
message), `[WARNING] regex {pattern!r}: {msg} (Python FutureWarning)`, with CPython 3.12's message and position
(snp-pyre). The lines come in the order pre-commit compiles the patterns (cfgv item order: a config's
`repos` and their hooks before the top-level `files` / `exclude`), and — like pre-commit, which compiles them
during validation — also when a later item makes validation fail. Two residual differences: Python prints a
message text once per process (a second pattern with the same message is silent), snp once per pattern; and
a pattern that fails to compile (`[a--b]`, a bad range CPython warns about before raising `re.error`) gets
only its validation error. stdout and the exit code are unchanged; the pattern keeps Python's literal
semantics; `PYTHONWARNINGS` has no effect on snp's line (X13-SM-10) and is passed through to hooks unchanged.

## Examples

```yaml
files: '[[]'
repos: []
```

pre-commit 4.7.0 `validate-config`, exit 0, stderr:

```
<site-packages>/cfgv.py:386: FutureWarning: Possible nested set at position 1
  re.compile(v)
```

snp, exit 0, stderr:

```
[WARNING] regex '[[]': Possible nested set at position 1 (Python FutureWarning)
```
