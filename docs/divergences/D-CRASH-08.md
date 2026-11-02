# D-CRASH-08

file excluded from this and later hooks

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-08.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| file deleted by an earlier hook before its tags were computed → `ValueError … does not exist.` exit 3 (X13-CLS-01/04) | file excluded from this and later hooks |

## Rationale

pre-commit's `Classifier` drops nonexistent names once (`os.path.lexists`, `run.py:73-74`) and then identifies a
file lazily, the first time a hook's `files`/`exclude` pass it (`functools.cache` on `_types_for_file`,
`run.py:76-79`). When an earlier hook deletes a file whose tags were not computed yet, the later hook's
`identify.tags_from_path` raises `ValueError('<f> does not exist.')` (`identify.py:40-44`; every `lstat` error maps
to it, X13-CLS-04) and the whole run dies with exit 3 after the earlier hook already ran — the remaining hooks never
run and the user sees a traceback-class error for a perfectly normal hook (formatters that delete or rename files).

snp's `FileTable` (`crates/snp-files`) treats a failing `lstat` (any error, as identify does) or a `NotFound` at the
content `open` as `TagOutcome::Vanished`: the file is dropped from that hook's list and, because the outcome is
memoised, from every later hook. A file whose tags were already computed before the deletion is still passed, exactly
as pre-commit's cache would pass it. The stat tier taken by the classifier's own `lexists` check is adopted only for
the first hook's candidates (pre-commit's first identification happens right then); every other file gets a fresh
`lstat` at its first use, so a deletion between hooks is always seen.

Safe: a deleted file cannot be checked by any hook (pre-commit would not check it either — it crashes); the run's
outcome still reflects every hook that ran, and nothing turns a failure into success (K5): the deleting hook itself
fails with `files were modified by this hook`. Two narrower cases follow from the tiers and stay within this ID: a
file whose stat tier was computed but whose content tier was not, deleted before a later hook needs its content, is
dropped too (pre-commit would pass it with tags computed earlier); and in staged mode with trusted git index modes
(ARCHITECTURE §2.3) the existence check costs no syscall, so a deleted staged file is passed on like pre-commit's
cached files (the hook then reports the missing file itself).

## Examples

Recorded with pre-commit 4.7.0 on CPython 3.12.3 (a venv built from the hash lock `xtask/oracle/requirements-pc470.txt`); `<W>` is the scratch root, `<store>` the store directory.

Hook 1 (`files: ^a\.txt$`, `always_run`, `pass_filenames: false`) runs `rm -f b.txt`; hook 2 is `echo`; both
`a.txt` and `b.txt` are committed; `run --all-files`
(`tests/differential/divergences/D-CRASH-08.yaml`).

pre-commit 4.7.0, exit 3:

```text
rm b.....................................................................Failed
- hook id: rm-b
- files were modified by this hook
An unexpected error has occurred: ValueError: b.txt does not exist.
Check the log at <store>/pre-commit.log
```

snp, exit 1 (`b.txt` is no longer passed to `echo`):

```text
rm b.....................................................................Failed
- hook id: rm-b
- files were modified by this hook
echo.....................................................................Passed
```

Unit coverage: `crates/snp-files/tests/catalog.rs::x13_cls_01_vanished_file_is_dropped_from_this_and_later_hooks`,
`x13_cls_04_lstat_errors_other_than_enoent_count_as_vanished`.
