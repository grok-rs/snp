# D-MISC-07

An in-hook hazmat failure prints only the final line of Python's traceback.

* **Class:** MISC (COMPATIBILITY §5.9)
* **Owner:** WP-26 (`snp_hooks::hazmat::in_hook_main`, ADR-006); the first package of the register row owns this
  file and `tests/differential/divergences/D-MISC-07.yaml` (WBS "Divergence files", ADR-071).
* **Catalog:** X13-HZ-06 (13 X.4.9), 03 Addendum §H.5/§H.8.6; tests `x13_hz_06_in_hook_failures` and
  `hazmat_goldens` (`crates/snp-hooks/tests/hazmat/main.rs`).

## Register entry (COMPATIBILITY §5.9)

| pre-commit 4.6.2 | snp |
|---|---|
| in-hook hazmat exec failure: full Python traceback | only the final traceback line (`pre_commit.parse_shebang.ExecutableNotFoundError: Executable `x` not found`) |

## Rationale

An `entry: pre-commit hazmat …` is rewritten by `lang_base.hook_cmd` to `python -m pre_commit.commands.hazmat …`
(`lang_base.py:174-178`). That module has no error handler: when the command cannot be resolved (`not found`, `is a
directory`, `is not executable`), the command is empty (`IndexError`), the `cd` directory is missing
(`FileNotFoundError`) or the exec fails (`OSError: [Errno 8] Exec format error`), Python prints a full traceback into
the hook's output — 15 to 20 lines of frames naming the venv's absolute paths — and exits 1, once per xargs partition
(03 Addendum §H.5 A3–A7, B3–B13, C2–C8).

snp rewrites the same entries to `snp __hazmat …` (D-MISC-02), which reproduces the module's behaviour — the same
exit codes (1 for these failures, 2 for argparse errors with prog `hazmat.py`, the child's status otherwise, `n1`'s
OR, `-N & 0xFF` for signals), the same `SystemExit` texts and argparse texts — but prints only the last line of the
traceback, byte for byte: the line that names the problem (`pre_commit.parse_shebang.ExecutableNotFoundError:
Executable `nope-not-here` not found`). The frames cannot be reproduced (they are the user's interpreter paths and
line numbers of pre-commit's source), carry no information about the user's hook, and a fake `Traceback (most recent
call last):` header would be misleading. Pass/fail and exit codes are unchanged; the difference is confined to the
message block (Tier B, 03 Addendum §H.8.6).

## Examples

A local hook `entry: pre-commit hazmat n1 nope-not-here --`, `language: system`, on one file:

pre-commit 4.6.2 (`- exit code: 1`):

```
Traceback (most recent call last):
  File "<frozen runpy>", line 198, in _run_module_as_main
  File "<frozen runpy>", line 88, in _run_code
  File "<site>/pre_commit/commands/hazmat.py", line 95, in <module>
    raise SystemExit(main())
  …
  File "<site>/pre_commit/parse_shebang.py", line 48, in _error
    raise ExecutableNotFoundError(f'Executable `{orig}` {msg}')
pre_commit.parse_shebang.ExecutableNotFoundError: Executable `nope-not-here` not found
```

snp (`- exit code: 1`):

```
pre_commit.parse_shebang.ExecutableNotFoundError: Executable `nope-not-here` not found
```

The scenario in `tests/differential/divergences/D-MISC-07.yaml` runs exactly this hook.
