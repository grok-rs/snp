# D-EXT-08

`interactive: true` hooks read and write the terminal directly: `{name}....(interactive)` before the hook, no captured
output in its block, a singleton bucket; without a usable terminal one stderr note
`snp: interactive: …; running '<id>' without a terminal` (ADR-085)

* **Class:** EXT (COMPATIBILITY §5.7)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-EXT-08.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by ADR-085 (`snp_proc::tty`, `spawn::interactive`, the singleton buckets of
  `crates/snp-run/src/sched.rs`; PTY tests in `crates/snp/tests/it/interactive.rs`).

## Register entry (COMPATIBILITY §5.7)

| Divergence |
|---|
| `interactive: true` hooks read and write the terminal directly: `{name}....(interactive)` before the hook, no captured output in its block, a singleton bucket; without a usable terminal one stderr note `snp: interactive: …; running '<id>' without a terminal` (ADR-085) |

## Rationale

pre-commit has no way to run a hook that prompts: every hook gets an empty stdin and its output is captured and shown
after it ends. lefthook's `interactive: true` gives the job the terminal; `snp migrate lefthook` keeps the key, and a
commitizen-style `prepare-commit-msg` hook (`test -n "$PRE_COMMIT_COMMIT_MSG_SOURCE" || cz --hook`) then prompts during
`git commit`.

For pre-commit `interactive` is an unknown key: it prints `[WARNING] Unexpected key(s) present on local => <id>:
interactive` and runs the hook as usual (D-EXT-01). snp, with a usable terminal (snp's stdin, or `/dev/tty` when stdin
is `/dev/null` as in a git hook; snp's process group must be the terminal's foreground group):

* prints `{name}{dots}(interactive)` (aligned with the status lines) and flushes it, then runs the hook with stdin on
  the terminal and stdout/stderr inherited, in snp's process group;
* prints the usual status line when the hook ends; the failure block has no output section (nothing was captured);
* runs it alone (a singleton bucket) and its xargs partitions one after another.

Without a usable terminal (CI, cron, a background job) it prints one stderr line and runs the hook exactly as without
the key — stdin an empty pipe, output captured. `SNP_NO_TTY=1` does the same without the line. The divergence is
therefore observable only in a config that uses the key; `SNP_COMPAT_STRICT=1` restores pre-commit's behaviour.

Why it is safe: only configs that opt in change; the opt-in key is lefthook's, with lefthook's semantics (terminal
stdin, no capture, the child in the parent's process group); a missing terminal never hangs the hook.

## Examples

```yaml
repos:
-   repo: local
    hooks:
    -   id: ask
        name: ask
        entry: echo asked
        language: system
        interactive: true
        verbose: true
```

`run --all-files` without a terminal (the differential harness runs the tools in their own process group):
pre-commit 4.6.2 prints `[WARNING] Unexpected key(s) present on local => ask: interactive` and the hook's block; snp
prints `snp: interactive: …; running 'ask' without a terminal` on stderr and the same block, without the warning. On a
terminal (`crates/snp/tests/it/interactive.rs`):

```text
ask...............................................................(interactive)
asked
ask......................................................................Passed
```

The scenario `tests/differential/divergences/D-EXT-08.yaml` checks the no-terminal case against the oracle.
