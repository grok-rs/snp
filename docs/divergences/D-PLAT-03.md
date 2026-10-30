# D-PLAT-03

Windows: batch-file (`.cmd`/`.bat`) hooks receive their arguments literally (BatBadBut-safe quoting); an argument
containing a line break fails the hook with a clear message.

* **Class:** PLAT, security relevant (COMPATIBILITY §5.5)
* **Owner:** WP-15 (`snp_proc::spawn`, which spawns through `std::process::Command`); the first package of the
  register row owns this file and `tests/differential/divergences/D-PLAT-03.yaml` (WBS "Divergence files", ADR-071).
* **Tests:** `batch_line_break_refusal` (`crates/snp-proc/src/spawn.rs`), the message golden in
  `crates/snp-proc/src/messages.rs`.

## Register entry (COMPATIBILITY §5.5)

| pre-commit 4.6.2 | snp |
|---|---|
| Windows: batch-file (`.cmd`/`.bat`) arguments with `&`, `^`, `%VAR%` are interpreted by cmd.exe | Rust's BatBadBut-safe quoting; arguments containing CR/LF fail the hook with ``Executable `{cmd0}` cannot receive an argument containing a line break: {repr}`` (rc 1) — class SEC |

## Rationale

A `.cmd`/`.bat` hook (npm shims such as `prettier.CMD` are batch files) is started by `CreateProcess`, which hands the
command line to `cmd.exe /c`. CPython builds that line with `subprocess.list2cmdline`, which quotes only arguments
containing a space or tab: a file name like `a&calc.txt`, `x|y.md` or `%PATH%.txt` reaches cmd.exe unquoted and is
*executed* or expanded (CVE-2024-24576 class, "BatBadBut"; 14 A.1 #6, P-25/P-26). File names come from the
repository, so this is command injection driven by repository content.

snp spawns through Rust's `std::process::Command`, which since 1.77.2 quotes every batch argument for cmd.exe
(`"a&calc.txt"`, `%` rewritten to `%%cd:~,%`), so the hook receives exactly the file names git reported. The one input
cmd.exe cannot receive safely is an argument containing CR or LF; Rust refuses it with an opaque
`batch file arguments are invalid`. snp checks for it before spawning and fails that partition like any spawn error
(rc 1, output = the message + `\n`):

``Executable `C:\…\run.cmd` cannot receive an argument containing a line break: 'a\nb'``

The quoting changes the command line only for arguments that pre-commit would let cmd.exe interpret; ordinary
arguments are passed unchanged. The longer quoted lines are kept under cmd.exe's limit by D-PLAT-09.

## Examples

Windows, a local hook `entry: run.cmd` (`@echo %*`), `language: system`, run on a tracked file `a&echo INJECTED.txt`:

pre-commit 4.6.2: cmd.exe runs `run.cmd a` and then `echo INJECTED.txt`; the output contains `INJECTED.txt` on a line
of its own.

snp: the batch file receives the single argument `"a&echo INJECTED.txt"` and echoes it verbatim.

With `args: ["line1\nline2"]` pre-commit's line is cut at the line feed by cmd.exe; snp fails the hook with
``Executable `run.cmd` cannot receive an argument containing a line break: 'line1\nline2'`` (`- exit code: 1`).
