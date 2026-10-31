# D-CRASH-23

A read-only file that needs no change passes `end-of-file-fixer`.

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-26 (`snp_hooks::pch::p0` end-of-file-fixer, all epochs, and its `repo: builtin` variant); the first
  package of the register row owns this file and `tests/differential/divergences/D-CRASH-23.yaml` (WBS "Divergence
  files", ADR-071).
* **Catalog:** 09 §4.A end-of-file-fixer "Edge cases", 09 §10 Q3; test `readonly_file_without_change_passes`
  (`crates/snp-hooks/tests/p0/main.rs`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| `end-of-file-fixer` (and the other fixers) on a read-only file that needs no change → crash in the hook | passes (opens for writing only when a change is needed) |

## Rationale

`end_of_file_fixer.main` (`end_of_file_fixer.py:52-63`, identical v3.0.0…v6.0.0) opens **every** file with
`open(filename, 'rb+')` before looking at it. A file the user cannot write — a vendored file checked out read-only, a
file on a read-only bind mount, a Windows file locked by an editor — makes that `open` raise
`PermissionError: [Errno 13] Permission denied: '<file>'` even when the file already ends with exactly one newline.
The exception is uncaught: Python prints a traceback, exits 1, and the files after it in the same xargs partition are
never checked. The failure is purely environmental: the file's content is fine and the hook would not change it.

The native reads the tail of the file through a read-only handle, decides whether a change is needed, and opens the
file for writing only to append the newline or truncate. A read-only file that needs no change therefore passes
silently and the rest of the partition is checked; a read-only file that *does* need a change still fails exactly as
in Python (`PermissionError: [Errno 13] Permission denied: '<file>'`, the partition stops, exit 1), because the fix
cannot be written. 09 §10 Q3 decided to keep parse-related crashes and improve purely environmental ones; this is the
only environmental crash among the P0 fixers that happens without a needed change (trailing-whitespace,
mixed-line-ending and fix-byte-order-marker open for writing only when they rewrite, as in Python). K4 (a): a
spurious crash becomes the result the hook would have had with write permission.

## Examples

A repository with `ro.txt` = `ok\n` (mode 0444) and `b.txt` = `x` (no newline), hook `end-of-file-fixer` (any tag),
partition order `ro.txt b.txt`:

pre-commit 4.6.2 (hook output, `- exit code: 1`, `b.txt` unchanged):

```
Traceback (most recent call last):
  …
PermissionError: [Errno 13] Permission denied: 'ro.txt'
```

snp (`- exit code: 1`, `- files were modified by this hook`, `b.txt` = `x\n`):

```
Fixing b.txt
```

The scenario in `tests/differential/divergences/D-CRASH-23.yaml` runs exactly this case (POSIX: the harness chmods
the file; it is skipped when the tests run as root, where the open succeeds).
