# D-PLAT-08

clear error, exit 1

* **Class:** PLAT (COMPATIBILITY §5.5)
* **Owner:** WP-18; the first package of the register row owns this file and
  `tests/differential/divergences/D-PLAT-08.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-18 (`snp_git::GitExe::version_gate`, `crates/snp-git/src/exe.rs`; ADR-051).

## Register entry (COMPATIBILITY §5.5)

| pre-commit 4.7.0 | snp |
|---|---|
| cygwin/MSYS2 git on Windows: a warning, then undefined behaviour | clear error, exit 1 |

## Rationale

A Cygwin or MSYS2 build of git speaks POSIX paths (`/cygdrive/c/…`, `/c/…`) and resolves them through its emulation
DLL; a native Windows program cannot use what it prints (`rev-parse` roots, `--git-path index`, `ls-files` paths) and
git cannot use the native paths it is given. pre-commit's guard (`git.check_for_cygwin_mismatch`, `git.py:210-230`)
only compares `get_root()` with `/`, which on native Windows Python is never true (`ntpath.abspath` always yields a
drive path, 14 #13): it is dead code, and the run continues with mismatched paths — wrong file lists, a stash that
cannot be restored, crashes deep in the run (undefined behaviour with the user's unstaged work at stake).

snp detects such a git before any repository call: on Windows, the directory of the resolved `git.exe` holds the
emulation DLL (`cygwin1.dll`, `msys-2.0.dll`); Git for Windows' `cmd\`, `bin\` and `mingw64\bin\` launchers never
do. `version_gate` then stops with a `FatalError` (exit 1) naming the git it found and the fix. Elsewhere nothing
changes (the check is compiled out of the decision on Unix).

## Examples

Windows, `PATH` starting with `C:\msys64\usr\bin` (MSYS2's `git.exe` next to `msys-2.0.dll`), `pre-commit run`
through the compat alias.

pre-commit 4.7.0 (native Windows Python): no warning (dead check), then undefined behaviour — typically
`An unexpected error has occurred: …` once a POSIX path from git meets a Windows path API, or a silent mis-selection
of files.

snp, exit **1**, stdout:

```
An error has occurred: FatalError: pre-commit does not support a Cygwin or MSYS2 git on Windows (found C:\msys64\usr\bin\git.exe); install Git for Windows (https://gitforwindows.org/) and put it first on PATH.
Check the log at <store>\pre-commit.log
```

(snp brand: `snp does not support …`.)

Regression tests: the detection (`exe::tests::cygwin_detection`, every OS: the DLL next to the executable decides)
and the brand texts (`messages::tests::cygwin_both_brands`). The scenario runs on the Windows CI runner with MSYS2.
