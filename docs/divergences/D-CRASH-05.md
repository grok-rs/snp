# D-CRASH-05

the hook fails (Executable not found, ValueError: No closing quotation, [Errno 7] Argument list too long: '<exe>', …), the run continues

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-05.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-32 (`snp_lang` returns `RunPlan::Error` for the entry errors; `snp_run::exec` turns `ArgumentTooLong` and unreadable scripts into a failed hook).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| empty `entry`, shlex errors, `ArgumentTooLong`, unreadable entry script → exit 3 (X13-EXE-01…04) | the hook fails (`Executable `` not found`, `ValueError: No closing quotation`, `[Errno 7] Argument list too long: '<exe>'`, …), the run continues |

## Rationale

pre-commit lets four configuration mistakes escape `run_hook` as Python exceptions: an empty `entry` (`IndexError`
from `cmd[0]`), an `entry` that `shlex.split` rejects (`ValueError`), a command that is too long for one partition
(`ArgumentTooLongError`) and an entry script it cannot read for its shebang (`PermissionError`). Each aborts the whole
run with exit 3 after a dangling `name....` segment, and the crash log points at pre-commit's own traceback, not at
the hook that is misconfigured.

snp reports them as what they are: a broken hook. The hook gets the `Failed` status, exit code 1 and a one-line
output naming the problem in the form pre-commit already uses for its own C-hook errors (``Executable `x` not
found``, `[Errno 7] Argument list too long: '/usr/bin/echo'` — the text pre-commit prints when `exec` itself fails
with `E2BIG`, X13-SM-06). The other hooks still run and the stash is restored as usual (13 R2, "C-hook").

Why it is safe: the run still fails (exit 1 instead of 3), so a commit is still blocked; nothing is skipped silently;
only the error's shape changes from a crash to a hook failure, which is strictly more informative.

## Examples

```text
$ cat .pre-commit-config.yaml
repos:
-   repo: local
    hooks:
    -   id: empty
        name: empty entry
        entry: ''
        language: system
    -   id: after
        name: after
        entry: 'true'
        language: system
$ pre-commit run -a; echo $?
empty entry..............................................................An unexpected error has occurred: IndexError: tuple index out of range
Check the log at ~/.cache/pre-commit/pre-commit.log
3
$ snp run -a; echo $?
empty entry..............................................................Failed
- hook id: empty
- exit code: 1

Executable `` not found

after....................................................................Passed
1
```

Proof: `crates/snp-run/tests/pipeline/run.rs` (`x13_exe_01_…`, `x13_exe_02_…`, `x13_exe_03_…`, `x13_exe_04_…`,
`x13_sm_07_…`); the scenario `tests/differential/divergences/D-CRASH-05.yaml`.
