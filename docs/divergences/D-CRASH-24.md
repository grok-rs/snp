# D-CRASH-24

silent, exit 120, stash restored

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-33 (contributing: WP-00); the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-24.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-33 (`snp::envelope`, the latched `Printer`; tests `errors::x13_eh_03_stdout_closed`,
  `errors::x13_sm_08_no_epipe_noise`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| stdout closed (EPIPE) → chained Python tracebacks on stderr, exit 120 (X13-EH-03) | silent, exit 120, stash restored |

## Rationale

When the reader of pre-commit's stdout goes away (`pre-commit run | head -c 20`, a GUI that closes git's stderr
pipe, which carries hook output), the next `stream.flush()` raises `BrokenPipeError`. The error handler then tries to
print the error on the same closed stdout, which raises again, and CPython's shutdown flush fails a third time:
stderr receives two or three chained tracebacks plus `Exception ignored on flushing sys.stdout:` /
`BrokenPipeError: [Errno 32] Broken pipe`, and the interpreter exits 120 (13 X.2 EH-03, X13-SM-08). The remaining
hooks do not run and the stash is restored by the `staged_files_only` context manager.

snp keeps the semantics and drops the noise: the `Printer` latches `closed` on the first `EPIPE`, cancels the run
token (`StdoutClosed`), starts no further hook, still runs the stash restore, and the envelope exits 120 without
writing anything to stderr and without a crash log (ARCHITECTURE §10.3; S4 "fail closed": the exit status is never
0). An `EPIPE` on stderr is ignored the same way.

Any other stdout write error is treated the same way: ENOSPC (`> /dev/full`), EIO or EAGAIN latch the Printer as
EPIPE does (`snp::envelope::FailClosedStdout`), so the command exits 120 silently instead of 0 with its output lost.
pre-commit 4.7.0 also exits 120 there (CPython's shutdown flush fails: `Exception ignored in: <_io.TextIOWrapper
name='<stdout>' ...>` / `OSError: [Errno 28] No space left on device`). Test: `errors::x13_eh_03_stdout_full`.

A stdout that is already closed when the process starts (`snp sample-config >&-`) is not a write error for snp: the
Rust runtime reopens fds 0-2 on `/dev/null` before `main` (so no file snp opens can become fd 1 and receive output),
and the command behaves as under `> /dev/null` (exit 0). pre-commit 4.7.0 exits 1 with an import-time traceback
(`sys.stdout` is `None`), an interpreter crash rather than a designed status. Test:
`errors::x13_eh_03_stdout_closed_at_start_is_dev_null`.

Why it is safe: the exit status (120) and every effect on the repository are pre-commit's; what disappears is a
Python traceback that describes the interpreter's own failure to report a closed pipe and that no tool parses.

## Examples

```text
$ { sleep 0.5; pre-commit sample-config; } | true          # pre-commit 4.7.0
Exception ignored on flushing sys.stdout:
BrokenPipeError: [Errno 32] Broken pipe
$ # exit 120
$ { sleep 0.5; pre-commit run --all-files; } | true        # stderr (abridged)
Traceback (most recent call last):
  File ".../pre_commit/error_handler.py", line 73, in error_handler
  ...
  File ".../pre_commit/output.py", line 11, in write
    stream.flush()
BrokenPipeError: [Errno 32] Broken pipe

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  ...
BrokenPipeError: [Errno 32] Broken pipe
Exception ignored on flushing sys.stdout:
BrokenPipeError: [Errno 32] Broken pipe
$ # exit 120
```

```text
$ { sleep 0.5; snp sample-config; } | true                 # nothing on stderr, exit 120
$ { sleep 0.5; snp run --all-files; } | true               # nothing on stderr, exit 120, unstaged changes restored
```

The scenario `tests/differential/divergences/D-CRASH-24.yaml` closes stdout before the tool starts.
