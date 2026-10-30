# D-CRASH-12

bytes end to end (argv, identify); lossy UTF-8 view for regex matching

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-12.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| non-UTF-8 file names from git or `--files` → `UnicodeDecodeError`/`UnicodeEncodeError` exit 3 (X13-GIT-05…08) | bytes end to end (argv, identify); lossy UTF-8 view for regex matching |

## Rationale

pre-commit decodes every git output strictly (`util.cmd_output`, `util.py:116-120`) — a single file name that is not
UTF-8 in `git ls-files -z`, the staged list, the merge-conflict diff or the intent-to-add list kills the run with
`UnicodeDecodeError` (exit 3, X13-GIT-05). A non-UTF-8 name given to `--files` survives argparse (surrogateescape)
but crashes when the hook's argv is encoded (`UnicodeEncodeError … surrogates not allowed`, X13-GIT-06), after the
hook's `name…` was already printed. On Linux and macOS such names are legal and occur in real repositories (Latin-1
archives, generated files).

snp keeps paths as the raw bytes git printed (`FileTable`, one byte arena, ADR-052): identify works on the bytes
(`file_identify::tags_from_filename(&[u8])`; parts that are not UTF-8 match no table key, exactly like Python's
surrogate-escaped names), hooks receive the raw bytes in argv (Unix `OsStr::from_bytes`), and `files`/`exclude`
match a lossy UTF-8 view (`FileTable::match_view`, U+FFFD for invalid sequences). Messages print the bytes verbatim;
git's own non-UTF-8 stderr is kept as bytes in `CalledProcessError` texts (X13-GIT-08). For every repository
pre-commit can handle (all names UTF-8) the results are byte-identical.

Safe: it only replaces a crash by the behaviour pre-commit has for valid names. The lossy view can only make a pattern
match or not match a byte that Python would have represented as a lone surrogate; patterns written for real names
(ASCII or UTF-8) behave identically, and nothing is ever decoded for argv or the file system.

## Examples

Recorded with pre-commit 4.6.2 on CPython 3.12.3 (a venv built from the hash lock `xtask/oracle/requirements-pc462.txt`); `<W>` is the scratch root, `<store>` the store directory.

A committed file `bad\xff.txt` and one `echo` hook (`tests/differential/divergences/D-CRASH-12.yaml`).

`run --all-files`, pre-commit 4.6.2, exit 3:

```text
An unexpected error has occurred: UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 27: invalid start byte
Check the log at <store>/pre-commit.log
```

`run --files $'bad\xff.txt'`, pre-commit 4.6.2, exit 3:

```text
echo.....................................................................An unexpected error has occurred: UnicodeEncodeError: 'utf-8' codec can't encode character '\udcff' in position 3: surrogates not allowed
Check the log at <store>/pre-commit.log
```

snp, both invocations, exit 0 (`echo` receives the raw bytes):

```text
echo.....................................................................Passed
```

Unit coverage: `crates/snp-files/tests/catalog.rs::x13_git_05_non_utf8_paths_from_git_stay_bytes`,
`x13_git_06_non_utf8_files_argument_is_passed_as_bytes`, `x13_git_08_non_utf8_command_stderr_is_kept_as_bytes`.
