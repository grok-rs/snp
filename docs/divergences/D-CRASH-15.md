# D-CRASH-15

output printed, [WARNING] line, run continues

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-15.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-32 (`snp_run::sched`, the output block).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| unwritable `log_file` → exit 3 and the hook output is lost (X13-EXE-05) | output printed, `[WARNING]` line, run continues |

## Rationale

`output.write_line_b(out.strip(), logfile_name=hook.log_file)` opens the log file (`open(logfile, 'ab')`) **before** it
writes anything, so a `log_file` whose directory does not exist, which is a directory, or which is not writable raises
`OSError` in the middle of a failing hook's block: the run aborts with exit 3 and the hook's output — the one thing the
user needs to fix the failure — is never printed (X13-EXE-05).

snp prints the output block first, exactly as pre-commit would have, then appends to the log file; a failure there
becomes one `[WARNING] Could not write to log_file <path>: <OSError text>` line inside the block and the run continues
with its normal exit code (decision F of 13 X.4).

Why it is safe: the hook's result and the run's exit code are unchanged; only a logging side channel degrades, and it
does so visibly.

## Examples

```text
$ pre-commit run -a        # log_file: nodir/x.log
logged...................................................................Failed
- hook id: l
- exit code: 1

An unexpected error has occurred: FileNotFoundError: [Errno 2] No such file or directory: 'nodir/x.log'
Check the log at ~/.cache/pre-commit/pre-commit.log
$ snp run -a
logged...................................................................Failed
- hook id: l
- exit code: 1

some-output
[WARNING] Could not write to log_file nodir/x.log: [Errno 2] No such file or directory: 'nodir/x.log'

```

Proof: `crates/snp-run/tests/pipeline/run.rs::x13_exe_05_unwritable_log_file_warns_and_keeps_the_output`; the scenario
`tests/differential/divergences/D-CRASH-15.yaml`.
