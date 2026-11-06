# D-MISC-09

A file pygrep cannot open fails the hook with the last line of Python's traceback.

* **Class:** TIERB (COMPATIBILITY §5.9)
* **Owner:** WP-26 (`snp_hooks::pygrep`); the first package of the register row owns this file and
  `tests/differential/divergences/D-MISC-09.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented; registered by ADR-090 (the behaviour dates from WP-26).

## Register entry (COMPATIBILITY §5.9)

| pre-commit 4.7.0 | snp |
|---|---|
| pygrep cannot open a file (in practice: a hook's `args` read as file names): a full Python traceback, coloured by CPython ≥ 3.13 in a colour run | only the final traceback line, uncoloured (`FileNotFoundError: [Errno 2] No such file or directory: 'x'`), same exit code |

## Rationale

pre-commit runs `python -m pre_commit.languages.pygrep *args entry *files`. pygrep's parser takes the first positional
as the pattern and the rest as file names, so any `args` value that is not one of its options shifts everything by
one: the argument becomes the pattern and the hook's `entry` becomes a file name. The first `open()` of a name that
does not exist ends the process with an uncaught exception, which is a complete Python traceback in the hook's output:
about a dozen lines naming the interpreter's frames and the user's venv.

The native reports the same failure as that traceback's final line plus a newline, after the same matches, with the
same exit code (`- exit code: 1`), which is what identifies the problem. The rest cannot be byte-identical in any
case: the frames contain the venv's paths and the interpreter's line numbers, and CPython 3.13 adds `~~~~^^^^` markers
and, because a `--color always` run gives the hook a pty, colours the whole traceback. It is the same class of
difference as D-MISC-05 (an invalid entry) and D-MISC-07 (an in-hook hazmat failure).

The generated differential suite hits this constantly, because it gives hooks random `args` to check exactly this
parsing. It compares the final line (COMPATIBILITY §2, `tests/differential/README.md`): that comparison found the
`open('')` parity bug, where an empty name was reported as `IsADirectoryError`.

## Examples

`.pre-commit-config.yaml` with a local hook `id: grep`, `language: pygrep`, `entry: TODO`, `args: [x]`:

pre-commit 4.7.0 (`- exit code: 1`):

```
Traceback (most recent call last):
  File "<frozen runpy>", line 198, in _run_module_as_main
  …
  File "…/site-packages/pre_commit/languages/pygrep.py", line 24, in _process_filename_by_line
    with open(filename, 'rb') as f:
         ^^^^^^^^^^^^^^^^^^^^
FileNotFoundError: [Errno 2] No such file or directory: 'TODO'
```

snp (`- exit code: 1`):

```
FileNotFoundError: [Errno 2] No such file or directory: 'TODO'
```

The scenario in `tests/differential/divergences/D-MISC-09.yaml` runs exactly this hook.
