# D-CRASH-17

marker-based store never needs repair; commands that do not need the store work; the error handler never double-faults

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-22 (contributing: WP-33); the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-17.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| store not creatable/corrupt (stale db rows, locked `db.db`, gc ENOENT, double faults in the error handler: X13-ST-*, X13-CMD-07, X13-EH-01/02) → exit 3 / tracebacks | marker-based store never needs repair; commands that do not need the store work; the error handler never double-faults |

## Rationale

pre-commit builds `Store()` for every command before dispatch (`main.py:377-384`) and keeps its clones in SQLite rows
(`store.py`). Three families of failures follow (13 X.4.2):

* **Store not creatable** (read-only parent, a path below a regular file): every command fails, including
  `validate-config`, `sample-config` and `clean`; the error handler calls `Store()` again to find the log path and
  fails again, so stderr gets two tracebacks joined by `During handling of the above exception…` and the exit code is
  1 (X13-ST-01/02, X13-EH-01).
* **Corrupt or locked database** (`db.db` garbage, empty, a directory, read-only, locked > 5 s): exit 3 from the
  first SQL statement (X13-ST-05…09).
* **Stale rows**: a row whose `repoXXXXXXXX` directory was deleted is returned without checking
  (`store.py:142-153`): `run` fails with `InvalidManifestError … is not a file` and `gc` crashes with
  `FileNotFoundError`, sticky, because the transaction rolls back (X13-ST-10, X13-CMD-07).

snp's store has no database (D-STORE-01). It is created lazily, only by commands that need it (`Store::new` does no
I/O; `ensure_created` creates the layout); a repository entry exists only when its `.snp-repo.json` commit marker —
written last, durably, inside a scratch directory that is then renamed into place — parses, derives the entry's key
and its checkout is present; anything else is "not materialised" and is replaced under the repository lock. Config
tracking is lock-free and best effort. gc retires entries by rename and tolerates `ENOENT`. The crash-log path is a
pure path computation (`Store::crash_log`), so the error handler never touches the file system to find it; when the
log cannot be written WP-33's envelope prints `Failed to write to log at …` and the body (pre-commit's branch for an
unwritable log), never a second traceback.

Safe: every one of these cases is a pre-commit crash or a sticky corruption; snp's behaviour is pre-commit's behaviour
for a healthy store. When a command needs a store that cannot be created, snp still fails with pre-commit's first
line (`PermissionError: [Errno 13] Permission denied: '<store>'`, exit 3) instead of succeeding silently.

## Examples

Recorded with pre-commit 4.7.0 on CPython 3.12.3 (a venv built from the hash lock `xtask/oracle/requirements-pc470.txt`); `<W>` is the scratch root, `<store>` the store directory.

`validate-config` with the store path below a regular file (`PRE_COMMIT_HOME`/`SNP_HOME` = `<W>/a.txt/store`;
`tests/differential/divergences/D-CRASH-17.yaml`). pre-commit 4.7.0, exit 1, stdout:

```text
An unexpected error has occurred: NotADirectoryError: [Errno 20] Not a directory: '<W>/a.txt/store'
```

and 30 lines of stderr: two tracebacks joined by `During handling of the above exception, another exception
occurred:`. With a read-only parent the same shape reads `PermissionError: [Errno 13] Permission denied: '<W>/ro/pch'`.
snp: `validate-config` does not need the store, exit 0, no output.

A cached remote repository whose clone directory was deleted. pre-commit 4.7.0, `run --all-files`, exit 1:

```text
An error has occurred: InvalidManifestError:
=====> <store>/repovvg7zgiz/.pre-commit-hooks.yaml is not a file
Check the log at <store>/pre-commit.log
```

then `gc`, exit 3 (and on every later `gc`):

```text
An unexpected error has occurred: FileNotFoundError: [Errno 2] No such file or directory: '<store>/repovvg7zgiz'
Check the log at <store>/pre-commit.log
```

snp re-materialises the entry (`[INFO] Initializing environment for …` is printed only when the `keys-v1` marker is
new, ADR-032) and runs the hook; `gc` prints `0 repo(s) removed.`.

Unit coverage: `crates/snp-store/tests/catalog.rs` (X13-ST-01…12 except ST-04, owned by WP-23) and
`support/behaviour.rs::kill_9_during_fill_leaves_no_partial_entry`.
