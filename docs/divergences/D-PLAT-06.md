# D-PLAT-06

absolute path kept

* **Class:** PLAT (COMPATIBILITY §5.5)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-PLAT-06.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-32 (`snp_run::common::rebase_path`).

## Register entry (COMPATIBILITY §5.5)

| pre-commit 4.6.2 | snp |
|---|---|
| Windows: config/`--files` on another drive → `ValueError` (relpath) exit 3 | absolute path kept |

## Rationale

`_adjust_args_and_chdir` makes `--config`, `--files` and `--commit-msg-filename` absolute against the invocation
directory and then relative to the repository root with `os.path.relpath`. On Windows, `relpath` between two drives
raises `ValueError: path is on mount 'D:', start on mount 'C:'`, so `pre-commit run --files D:\\x.txt` from a
repository on `C:` crashes with exit 3.

snp keeps the absolute path in that case. Every consumer accepts an absolute path (git, the classifier, hooks), so the
file is simply handled from where it is; on Unix and on one drive nothing changes.

Why it is safe: the path still names the same file; no output that pre-commit produces on a working invocation changes.

## Examples

```text
C:\repo> pre-commit run --files D:\data\x.txt
An unexpected error has occurred: ValueError: path is on mount 'D:', start on mount 'C:'
C:\repo> snp run --files D:\data\x.txt
… hooks run with D:/data/x.txt
```

Proof: the unit tests of `snp_run::common` (`rebase_keeps_order_and_dots`); the scenario
`tests/differential/divergences/D-PLAT-06.yaml` (Windows runner with a second drive).
