# D-DATA-18

a SIGINT sent with kill(2) to the tool is forwarded to the running hooks before the stash is restored

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-15 (`snp_proc::signals`, the origin-aware forwarding of ARCHITECTURE §10.4); WP-32 wires it into
  `snp run`. The first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-18.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented (ADR-065); registered after integration (ADR-083).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.7.0 | snp |
|---|---|
| a SIGINT sent to pre-commit alone (`kill -INT <pid>`: an IDE or CI cancel) becomes `KeyboardInterrupt` in its own process only: the running hook never sees it and keeps running, orphaned, after the stash was restored (X13-EH-04) | the sent SIGINT is forwarded to every running hook (ARCHITECTURE §10.4, ADR-065), leftover descendants are killed (Linux), then the stash is restored and the run exits 130 as before; a terminal ^C (kernel origin) is not re-sent |

## Rationale

Python turns SIGINT into `KeyboardInterrupt` in the main thread. pre-commit's `cmd_output_b` (`util.py`) waits in
`proc.communicate()`; the exception unwinds through it without signalling or waiting for the child, through
`staged_files_only`'s `finally` (the restore) and `error_handler` (`Interrupted (^C): KeyboardInterrupt:`, exit 130).
With a terminal ^C that is harmless: the kernel delivered the SIGINT to the whole foreground process group, hooks
included. A SIGINT *sent* to pre-commit's pid alone (an IDE's commit cancel, a CI step's cancel, `timeout -s INT`,
`kill -INT`) reaches only Python: the hook — a formatter, a code generator — keeps running after pre-commit restored
the user's unstaged changes, and whatever it writes then lands on top of them (or after the commit the user retries).

snp classifies every signal by origin (`SignalsInfo<WithOrigin>`, ARCHITECTURE §10.4, ADR-065): a kernel-origin
SIGINT is not re-sent (the hooks already have it, parity), a process-sent one is forwarded to every Hook/Tool child.
After the grace (`SNP_KILL_GRACE`) the remaining children are killed and, on Linux, every leftover descendant is reaped
(`registry::quiesce`) before the restore; Critical git commands of the stash are never signalled.

Why it is safe: the exit status (130), the `Interrupted (^C): KeyboardInterrupt: ` line and the restore are pre-commit's;
the only difference is that the hook is told to stop — exactly what a terminal ^C does to it under both tools — so no
hook writes into the worktree after the restore. A hook that traps SIGINT sees one more signal than under pre-commit.

## Examples

A `trap` hook that appends the name of each signal it receives to `.hook-started`; `kill -INT <tool pid>` once it runs
(`tests/differential/divergences/D-DATA-18.yaml`, also `tests/differential/scenarios/run/x13-eh-04-interrupt.yaml`):

```text
# pre-commit 4.7.0: exit 130, .hook-started was never written (the hook got no signal and kept running)
$ echo signals:; cat .hook-started
signals:

# snp: exit 130, the hook got the forwarded SIGINT and stopped before the restore
$ echo signals:; cat .hook-started
signals:
INT
```

Process-level proof: `crates/snp-proc/tests/suite/signals.rs::{signal_matrix_sent_sigint, signal_matrix_kernel_sigint}`
(a sent SIGINT reaches Inherit hooks, a kernel one is not re-sent) and
`crates/snp/tests/it/stash.rs::sigint_during_a_hook_restores_the_stash`.
