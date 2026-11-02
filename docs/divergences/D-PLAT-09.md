# D-PLAT-09

Windows: partitions of a `.cmd`/`.bat` hook are re-split when the real (quoted) command line would exceed cmd.exe's
8191-character limit.

* **Class:** PLAT (COMPATIBILITY §5.5)
* **Owner:** WP-15 (`snp_proc::xargs::partition`); the first package of the register row owns this file and
  `tests/differential/divergences/D-PLAT-09.yaml` (WBS "Divergence files", ADR-071).
* **Tests:** `d_plat_09_batch_partitions_are_resplit`, `batch_argument_lengths_follow_rusts_quoting`
  (`crates/snp-proc/src/xargs.rs`).

## Register entry (COMPATIBILITY §5.5)

| pre-commit 4.7.0 | snp |
|---|---|
| Windows: `.cmd` partitions can exceed 8191 characters after quoting (crash) | partitions re-split |

## Rationale

For a batch-file hook `xargs.xargs` budgets `8192 - len(cmd.exe) - len(' /c ') - 1024` characters (`xargs.py:155-164`)
and partitions by the *unquoted* length of each argument plus one separator. The command line cmd.exe finally
receives is longer: arguments that need quoting gain two quotes, embedded quotes are doubled, and with D-PLAT-03 every
`%` becomes the 8-character `%%cd:~,%`. File names with many such characters push a partition past cmd.exe's hard
limit of 8191 characters, and the hook fails with `The command line is too long.` (or `[WinError 206]` from
`CreateProcess` above 32 767), which pre-commit reports as a failed hook or a crash.

snp computes partitions exactly as pre-commit does (`partition` is an exact port, so partition boundaries and the
order of output never change for ordinary names), then measures each partition's real Rust batch command line
(`cmd.exe /e:ON /v:OFF /d /c ""<script>" <quoted args…>"`, UTF-16 units) and halves any partition that would exceed
8191 characters until it fits. Files keep their order; only the number of hook invocations grows, and only for
partitions that could not have run at all.

## Examples

Windows, a local hook `entry: check.cmd` (`@exit /b 0`), `language: system`, `require_serial: true`, and 1 000
tracked files named `a b0000` … `a b0999` (7 characters, one space each).

pre-commit 4.7.0: the unquoted budget (~7 100 characters with `C:\Windows\system32\cmd.exe`) admits ~887 names
per partition at 8 characters each; `list2cmdline` quotes every name (10 characters each), so cmd.exe receives
~8 900 characters and the hook fails with `The command line is too long.` (`- exit code: 1`).

snp: the same partitions are computed, then each one whose quoted line exceeds 8 191 characters is halved; every
invocation fits and the hook passes (exit 0).
