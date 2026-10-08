# D-CRASH-16

A pty that cannot be allocated in colour mode falls back to pipes.

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-15 (`snp_proc::spawn::merged`); the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-16.yaml` (WBS "Divergence files", ADR-071).
* **Catalog:** X13-EXE-06 (13 X.4.5); test `x13_exe_06_pty_failure_falls_back_to_pipes` (`crates/snp-proc/src/spawn.rs`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| pty allocation failure in colour mode → exit 3 (X13-EXE-06) | falls back to pipes |

## Rationale

With `--color always` (or `auto` on a terminal) pre-commit runs every hook partition under a fresh pseudo-terminal:
`cmd_output_p` (`util.py:162-200`) enters `Pty()`, which calls `os.openpty()` before the hook is spawned. When no pty
can be allocated — a container or sandbox without `/dev/ptmx` or `devpts`, an exhausted `kernel.pty.max`, a seccomp
profile that denies `TIOCGPTN` — `openpty` raises `OSError` outside the `try` that guards `Popen`, the exception
escapes `run_hook`, and pre-commit crashes: `An unexpected error has occurred: …` + `Check the log at …`, exit 3,
after the hook's start line was already printed. The same happens when reading the master fails with any error other
than `EIO` (`util.py:188-194`).

The pty exists only so that hooks see a terminal and colour their output; it never changes what a hook does to the
work tree. snp's `merged(spec, pty: true, …)` therefore treats a failed `openpt`/`grantpt`/`unlockpt`/`ptsname`/slave
`open`/`tcsetattr` as "no pty available" and runs that partition exactly like the non-colour path (one `pipe` for
stdout and stderr, stdin an empty pipe): the hook runs, its exit status decides the result, and the stash is restored
as on every other path. Nothing that pre-commit reports as a success becomes a failure, and the only change is that a
crash becomes a normal hook run whose output is not coloured by the hook itself (snp's status lines keep their colour).
This satisfies K4 (a) and K5: a crash is replaced by the behaviour pre-commit has whenever a pty *is* available, minus
the hook's own colours.

## Examples

Repository with one local hook `entry: echo coloured`, `language: system`, `pass_filenames: false`, `verbose: true`,
run in a sandbox whose `/dev/ptmx` is unusable, `pre-commit run --all-files --color always`:

pre-commit 4.6.2 (exit 3; the exception class and text depend on how the pty fails — `FileNotFoundError: [Errno 2] No
such file or directory` without `/dev/ptmx`, `OSError: out of pty devices` when exhausted):

```
echo hook................................................................An unexpected error has occurred: FileNotFoundError: [Errno 2] No such file or directory
Check the log at <store>/pre-commit.log
```

snp (exit 0):

```
echo hook................................................................\x1b[42mPassed\x1b[m
\x1b[2m- hook id: echo-hook\x1b[m
\x1b[2m- duration: [D]s\x1b[m

coloured

```

The scenario in `tests/differential/divergences/D-CRASH-16.yaml` runs the hook with `--color always` inside a sandbox
that hides `/dev/ptmx` (`requires.sandbox: no-ptmx`, provided by the harness with bubblewrap); outside such a sandbox
the scenario is skipped.
