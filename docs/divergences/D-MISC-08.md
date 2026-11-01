# D-MISC-08

error source chain / Rust backtrace (crash-log body is Tier B)

* **Class:** TIERB (COMPATIBILITY §5.9)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-MISC-08.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-33 (`snp::crash_log::body`; tests `errors::t10_err_02_line_two_and_log_skeleton`,
  `errors::x13_eh_05_rust_panic`, `crash_log::tests::skeleton`).

## Register entry (COMPATIBILITY §5.9)

| pre-commit 4.6.2 | snp |
|---|---|
| Python tracebacks in the crash log body | error source chain / Rust backtrace (crash-log body is Tier B) |

## Rationale

pre-commit's crash log (`error_handler.py:_log_and_exit`) ends with `traceback.format_exc()` — a Python traceback
through pre-commit's own modules. snp has no Python stack; reproducing one would be fiction. ADR-013 keeps
pre-commit's skeleton byte for byte up to and including the `### error information` block with line 1, so log
scrapers that read the version block or the error line keep working, and replaces the traceback block with what
explains an snp failure: the error's source chain (`Failure` → cause → …, one line per level), for a panic the
`panicked at {file}:{line}:{col}` location, the Rust backtrace and the bug-report hint naming
`https://github.com/grok-rs/snp/issues`; `(no further details)` when the failure carries none. The log is written
`O_NOFOLLOW` and truncated, named by brand (`pre-commit.log` / `snp.log`, D-STORE-01); an unwritable log prints
`Failed to write to log at …` and the body on stdout instead of faulting twice.

Why it is safe: the terminal output (line 1 and `Check the log at …`) and the exit code are pre-commit's; the log body
below the version block is Tier B by design (ADR-013), read by people filing bugs, never parsed by tools.

## Examples

`pre-commit install` with `.git/hooks/pre-commit` being a directory (both: exit 3, same two stdout lines):

~~~text
### error information                                   # pre-commit 4.6.2 log, abridged

```
An unexpected error has occurred: IsADirectoryError: [Errno 21] Is a directory: '.git/hooks/pre-commit'
```

```
Traceback (most recent call last):
  File ".../pre_commit/error_handler.py", line 73, in error_handler
    yield
  ...
  File ".../pre_commit/commands/install_uninstall.py", line 59, in is_our_script
    with open(filename, 'rb') as f:
IsADirectoryError: [Errno 21] Is a directory: '.git/hooks/pre-commit'
```
~~~

~~~text
### version information                                 # snp log (compat brand)

```
pre-commit version: 4.6.2
git --version: git version 2.43.0
sys.version:
    snp 2.0.0 (rustc 1.99.0) [x86_64-linux]
sys.executable: /usr/local/bin/snp
os.name: posix
sys.platform: linux
```

### error information

```
An unexpected error has occurred: IsADirectoryError: [Errno 21] Is a directory: '.git/hooks/pre-commit'
```

```
(no further details)
```
~~~

A panic (`SNP_FAULT=PrinterWrite` in a `testing` build) ends the log with `panicked at crates/…:L:C`, the
`stack backtrace:` lines and `snp panicked: this is a bug. Please report it at https://github.com/grok-rs/snp/issues
with this log.`

The scenario `tests/differential/divergences/D-MISC-08.yaml` produces this crash in both tools.
