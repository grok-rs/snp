# D-CRASH-09

opened non-blocking, classified, never blocks

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-09.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| FIFO / device passed via `--files` → hangs forever (X13-CLS-03) | opened non-blocking, classified, never blocks |

## Rationale

identify's `lstat` classifies only directories, symlinks and sockets specially; a FIFO or a device falls through as a
`file` whose content decides `text`/`binary`, and `file_is_text` (and, for an executable without name tags,
`parse_shebang_from_file`) opens it with a blocking `open()` (`identify.py:135-139, 205-219`). A FIFO without a writer
blocks that `open` forever: `pre-commit run --files <fifo>` hangs with no output (git cannot track FIFOs, so only an
explicit file list reaches one). A hang in a git hook freezes the user's commit.

snp opens every file of the content tier with `O_RDONLY | O_NONBLOCK | O_NOCTTY` and treats `EAGAIN` ("no data yet")
as end of input. The classification is then exactly identify's whenever identify completes: an empty FIFO (no writer,
or a writer that wrote nothing) is `text` — what identify reports once a writer opens and closes it — `/dev/null` is
`text` and `/dev/zero` is `binary`. Directories, symlinks and sockets are never opened (as in identify). file-identify
0.5 itself never opens non-regular files (X13-CLS-05); snp classifies FIFOs and devices as regular files for the
content step so that the result matches identify's instead of file-identify's "binary" default.

Safe: only a hang becomes a result, and the result is the one identify gives whenever it does not hang; at most 1 KiB
(plus a long shebang line) is read, so a device that never ends (`/dev/zero`) cannot exhaust memory.

## Examples

Recorded with pre-commit 4.7.0 on CPython 3.12.3 (a venv built from the hash lock `xtask/oracle/requirements-pc470.txt`); `<W>` is the scratch root, `<store>` the store directory.

A FIFO `ff` in the worktree, one `echo` hook, `run --files ff` (`tests/differential/divergences/D-CRASH-09.yaml`).

pre-commit 4.7.0 hangs; killed by `timeout 5` it prints nothing (exit 124).

snp, exit 0 (the FIFO is tagged `file, non-executable, text`):

```text
echo.....................................................................Passed
```

Unit coverage: `crates/snp-files/tests/catalog.rs::x13_cls_03_fifo_never_blocks`,
`x13_cls_05_file_identify_does_not_block_on_fifos`; the identify vectors (`crates/snp-files/tests/vectors/
identify_tree.json`, FIFOs fed by a writer so identify completes) pin the tags.
