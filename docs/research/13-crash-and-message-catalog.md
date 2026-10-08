# 13 — Crash (exit 3) and secondary-message catalog for snp

This file collects the uncaught-exception paths of pre-commit 4.6.2, the Python double faults and the secondary user-visible messages, each with a keep-or-fix decision for snp. Sections are appended as addenda.

## Addendum: One catalog of uncaught-exception (exit 3) paths and secondary messages, with a keep-or-fix decision for each

### X.0 Scope, provenance, conventions

- **Reference:** pre-commit 4.6.2 (`refs/pre-commit`). `diff -r` showed it is byte-identical to the installed oracle `scratchpad/venv-pc462`. Versions: CPython 3.12.3, PyYAML 6.0.3 (libyaml), identify 2.6.20, cfgv 3.5.0, git 2.43.0, Linux. Line numbers refer to 4.6.2.
- **Evidence tags:**
  - `✔cNN`: reproduced in this session in `scratchpad/tmp-critic/g4/cNN`, with helper `scratchpad/scripts-g4/env.sh` and an isolated `PRE_COMMIT_HOME=…/cNN.pch`.
  - `✔01`, `✔03`, `✔04b`: verified by the cited spec.
  - `src`: derived from the source only.
- **Authority.** This section is the **single source** for:
  - every path that ends in `An unexpected error has occurred` (exit 3);
  - every Python double fault (a traceback with exit 1 or 120);
  - every user-visible secondary message that no other spec quotes.

  It **supersedes 01 §16**, whose rows are merged into X.4.1. It also collects the scattered crash notes in 02 §9/§10/§11/§17/§21, 03 §1.4/§7.9/§7.10/§15/§20.4/§20.9, 04a §14.15, 04b §1.8 and 04b's robustness table item 9. Where those sections conflict with a decision here, this section wins (X.7).
- **Placeholders:**

  | Placeholder | Meaning |
  |---|---|
  | `<store>` | `realpath($PRE_COMMIT_HOME or $XDG_CACHE_HOME/pre-commit or ~/.cache/pre-commit)` |
  | `repoXXXXXXXX` | `tempfile.mkdtemp(prefix='repo', dir=<store>)`, with 8 random chars from `[a-z0-9_]` |
  | `<tmp>` | `/tmp/tmpXXXXXXXX` (`tempfile.TemporaryDirectory`) |
  | `patch<ts>-<pid>` | `int(time.time())` and the pid |
  | `<name>…` | the hook start segment `name` + dots, written **without** a newline (`run.py:186`) |

### X.1 The exit-3 envelope (what every K row prints)

Source: `error_handler.py:19-81`.

1. **Line 1** is `An unexpected error has occurred: <Type>: <text>`, written to **stdout** with `\n` and flushed.
   - `<Type>` is `type(exc).__name__`, unqualified: `CalledProcessError`, `JSONDecodeError`, `OperationalError`, `ExecutableNotFoundError`.
   - `<text>` is `bytes(exc)` when the class defines it (only `pre_commit.util.CalledProcessError`), otherwise `str(exc).encode()`. It may span several lines.
2. **Line 2** depends on `os.access(<store>, W_OK)`:
   - writable: `Check the log at <store>/pre-commit.log`;
   - not writable: `Failed to write to log at <store>/pre-commit.log`, then the whole log body on stdout (✔c13 ✔c22).
3. **Exit 3.** The log is overwritten (`'wb'`). Its skeleton is in 02 §17.

Placement rules (verified):

- If `<name>…` was already written, line 1 continues **on the same line** (✔c04 ✔c09 ✔c19 ✔c20):
  ```
  e........................................................................An unexpected error has occurred: IndexError: tuple index out of range
  ```
- Context managers unwind **before** the banner. The stash is restored first, so `[INFO] Restored changes from <store>/patch<ts>-<pid>.` comes before line 1 (✔c08 ✔c22). The temp dirs of autoupdate and try-repo are removed.
- After line 1, only line 2 and the optional log body (when the store is not writable) are printed. The remaining hooks do not run.
- Neighbours:
  - `FatalError` → `An error has occurred: …`, exit 1;
  - `KeyboardInterrupt` → `Interrupted (^C): KeyboardInterrupt: ` (trailing space), exit 130;
  - `SystemExit("msg")` is **not** caught: `msg` goes to **stderr**, exit 1;
  - argparse errors exit 2 on stderr.

### X.2 Formatting primitives (needed to reproduce K rows byte for byte)

#### X.2.1 OSError family

- With a filename: `[Errno <n>] <strerror>: <repr(filename)>`.
- With two filenames (rename/replace/move): append ` -> <repr(filename2)>`.
- Without a filename: `[Errno <n>] <strerror>`.

The class depends on the errno (CPython errno map):

| errno | Python class |
|---|---|
| ENOENT | `FileNotFoundError` |
| EACCES, EPERM | `PermissionError` |
| EEXIST | `FileExistsError` |
| EISDIR | `IsADirectoryError` |
| ENOTDIR | `NotADirectoryError` |
| EPIPE, ESHUTDOWN | `BrokenPipeError` |
| ECONNREFUSED / ECONNRESET / ECONNABORTED | `ConnectionRefusedError` / `ConnectionResetError` / `ConnectionAbortedError` |
| ETIMEDOUT | `TimeoutError` |
| EAGAIN, EWOULDBLOCK, EALREADY, EINPROGRESS | `BlockingIOError` |
| EINTR | `InterruptedError` |
| ECHILD | `ChildProcessError` |
| ESRCH | `ProcessLookupError` |
| anything else (E2BIG, ENOSPC, EROFS, ENOTEMPTY, ENAMETOOLONG, ELOOP, EXDEV…) | `OSError` |

- **strerror** comes from the C library. glibc and macOS agree for the common cases:

  | errno | strerror |
  |---|---|
  | 1 | `Operation not permitted` |
  | 2 | `No such file or directory` |
  | 7 | `Argument list too long` |
  | 13 | `Permission denied` |
  | 17 | `File exists` |
  | 20 | `Not a directory` |
  | 21 | `Is a directory` |
  | 28 | `No space left on device` |
  | 30 | `Read-only file system` |
  | 32 | `Broken pipe` |

  The numbers differ on macOS for some errnos (ENOTEMPTY 66, ENAMETOOLONG 63, ELOOP 62, EAGAIN 35). Always print the platform's number.
- **In Rust (no `unsafe`, no libc call):**
  - class: map `io::Error::raw_os_error()` through the table above;
  - strerror: `io::Error::from_raw_os_error(n).to_string()` with the trailing ` (os error n)` removed;
  - filename: Python `str` repr (01 §2.6). Non-UTF-8 bytes become surrogate escapes, as Python's `surrogateescape` does: `'bad\udcff.txt'`.
- **Windows:** Win32-API failures render as `[WinError <n>] <FormatMessage text>: '<f>'`, for example `[WinError 5] Access is denied` or `[WinError 32] The process cannot access the file because it is being used by another process`. CRT failures (`open`) keep `[Errno n] …`. Treat Windows texts as Tier B.

#### X.2.2 Two different `CalledProcessError` renderings

**A) `pre_commit.util.CalledProcessError`** (`cmd_output`/`cmd_output_b` with `check=True`; `util.py:49-78`):

```
command: <repr(tuple(cmd))>
return code: <rc>
stdout:<IND(stdout)>
stderr:<IND(stderr)>
```

- `IND(x)` is ` (none)` when `x` is `None` or empty. Otherwise it is `\n    ` + `x`, with every `\n` replaced by `\n    `, then `.rstrip()`.
- Effects of `IND`: the trailing newline disappears, and an **inner blank line becomes a line of exactly four spaces** (✔c15).
- `cmd[0]` is the **resolved absolute path** (`'/usr/bin/git'`), because `normalize_cmd` already ran (`util.py:98`).
- When the executable is missing, `cmd` is the original tuple, `return code: 1`, the stdout block is ``    Executable `<x>` not found`` and `stderr: (none)` (✔04b, lua).
- A death by signal gives a negative code: `return code: -9`.

**B) `subprocess.CalledProcessError`** (only `hook_impl.py:143-164`, `subprocess.check_output`):

- `Command '<repr(tuple(cmd))>' returned non-zero exit status <rc>.`
- For a signal: `Command '<…>' died with <Signals.SIGKILL: 9>.`. For an unknown signal: `… died with unknown signal <n>.`.
- `cmd[0]` is the bare `'git'` (no normalisation). stdout is captured but not shown.
- **stderr is not captured.** git's message reaches the terminal's stderr **before** the banner (✔c10).

#### X.2.3 Unicode errors

- **`UnicodeDecodeError`** has two forms:
  - `'utf-8' codec can't decode byte 0x<hh> in position <i>: <reason>`;
  - for a truncated multi-byte sequence at the end of the input: `'utf-8' codec can't decode bytes in position <i>-<j>: unexpected end of data`, where `j` is the last index.

  This corrects 01 §12.9, which lists only the single-byte form. Verified: `b'\xe2\x82'` → `bytes in position 0-1: unexpected end of data`; `b'\xe2'` → `byte 0xe2 in position 0: unexpected end of data`.
- `<i>` is the byte offset **within the whole decoded buffer**. For git `-z` output that is the full NUL-separated list (✔c04: position 27 = `len(".pre-commit-config.yaml\0") + 3`).
- **`UnicodeEncodeError`** (surrogate-escaped argv re-encoded strictly): `'utf-8' codec can't encode character '\udcff' in position 3: surrogates not allowed`. A run of such characters gives `characters in position <i>-<j>`.

#### X.2.4 Other texts reproduced verbatim by K rows

- **sqlite3** (✔c06 ✔c14 ✔c22):
  - `OperationalError: database is locked`, after the 5.0 s default busy timeout (measured 5.19 s wall time);
  - `OperationalError: no such table: repos`;
  - `OperationalError: unable to open database file`;
  - `OperationalError: attempt to write a readonly database`;
  - `DatabaseError: file is not a database`;
  - corrupted pages: `DatabaseError: database disk image is malformed` (sqlite's text; not reproduced).
- **Unpacking:** `ValueError: not enough values to unpack (expected 4, got <n>)`.

### X.3 Decision policy

| Code | Meaning |
|---|---|
| **K** | keep: same banner, Type and text, exit 3. Tier A for line 1 and the exit code; the log body is Tier B. |
| **K+** | K, then exactly one extra line after the log line (a recovery hint). Used only where user data is at stake. |
| **C-cfg** | convert to the validation channel: `An error has occurred: InvalidConfigError: ` (or `InvalidManifestError: `), the usual cfgv context lines, the given leaf, exit 1. `validate-config` prints the block and exits 1. |
| **C-hook** | convert to a failed hook: status `Failed`, `- exit code: 1`, output = the given text. The run continues and the overall exit code is 1. |
| **C-err** | convert to the stated different terminal error and exit code. |
| **F** | fix: do the obviously intended thing, with no error (optionally a `[WARNING]`). |
| **D** | drop: Python-internal noise that snp never emits. |

Rules, applied in order:

1. User-fixable config input that pre-commit *meant* to validate → **C-cfg**.
2. External or environmental failures (git, network, permissions, missing tools, a corrupt store) → **K**. Their text is informative and is a de-facto API (CI log greps, bug reports).
3. Obvious pre-commit bugs whose fix cannot weaken a check → **F**.
4. Errors in building one hook's argv → **C-hook**. This mirrors how pre-commit already treats ``Executable … not found`` (`xargs.py:150-152`) and a Popen `OSError` (`util.py:86-87,104-105`).
5. **Never fail open.** No conversion may turn a crash into exit 0 unless every selected hook actually ran and passed.
6. Never a double fault, a traceback, or a Rust panic exit 101.

### X.4 Master catalog

Columns: **ID** · **trigger** (minimal repro) · **where** (4.6.2 source) · **pre-commit text** (line 1 unless stated) · **exit** · **evidence** · **decision** · **snp text**.

#### X.4.1 Config and manifest load (01 §16 merged here)

| ID | trigger | where | pre-commit text | exit | evidence | decision | snp text |
|---|---|---|---|---|---|---|---|
| CFG-01 | `minimum_pre_commit_version: "abc"` (also `""`, `"4."`, `"4.6.3rc1"`) | `clientlib.py:50-61` | `ValueError: invalid literal for int() with base 10: 'abc'` | 3 | ✔c24 ✔01 | C-cfg | leaf `invalid literal for int() with base 10: 'abc'` under `==> At Config()` / `==> At key: minimum_pre_commit_version` (for manifest and local hooks, under the hook's context) |
| CFG-02 | non-string root key `1: x` | `clientlib.py:358-372` | `TypeError: sequence item 0: expected str instance, int found` | 3 | ✔01 | F | `[WARNING] Unexpected key(s) present at root: 1` (Python `str()` of the keys, sorted by (type rank, str); 01 §16) |
| CFG-03 | mixed root keys `1: x` + `foo: y` | same | `TypeError: '<' not supported between instances of 'str' and 'int'` | 3 | ✔01 | F | as CFG-02 |
| CFG-04 | unhashable `stages: [[a]]`, `language: [x]`, `types: [[a]]` | `clientlib.py:42-47,71-72,194-227` | `TypeError: unhashable type: 'list'` | 3 | ✔01 | C-cfg | `Expected one of … but got: ['a']` / `Type tag ['a'] is not recognized.  Try upgrading identify and pre-commit?` (01 §16) |
| CFG-05 | regex repeat bound ≥ 4294967295 in `files`/`exclude` (top level, hook or manifest): `a{4294967295}`, `a{4294967296}`, `a{0,4294967295}`, `a{99999999999,}`. `a{4294967294}` is accepted. | `cfgv.py:384-388` (catches only `re.error`) → `re/_parser.py:670-675` | `OverflowError: the repetition number is too large` | 3 | ✔c01 (validate-config and run) | C-cfg | leaf `'a{4294967296}' is not a valid python regex` (pattern repr, same context as any `re.error`) |
| CFG-06 | group nesting depth ≥ ~490, e.g. `'('*490+')'*490`; `(?:` behaves the same | same | `RecursionError: maximum recursion depth exceeded` | 3 | ✔c02 (validate-config, CPython 3.12.3: 480 accepted, 490 crashes; the limit moves by a few with call depth and Python version) | C-cfg | same leaf. snp accepts depth ≤ 480 and rejects > 480 (a fixed, documented constant). |
| CFG-07 | two global inline flags that conflict: `(?a)(?u)x`, `(?u)(?a)x`. `(?au)` is already a clean `re.error`; `(?u)(?a:x)` is valid. | `re/_parser.py:957-961` (`fix_flags`) | `ValueError: ASCII and UNICODE flags are incompatible` | 3 | ✔c03 | C-cfg | same leaf as CFG-05 |
| CFG-08 | `PYTHONWARNINGS=error` plus a pattern that warns (`[[]`, `[[:alpha:]]`, `[a&&b]`, `[a\|\|b]`, `[a~~b]`) | Python `warnings` | `FutureWarning: Possible nested set at position 1` | 3 | ✔c02 | F: snp ignores `PYTHONWARNINGS` | pattern accepted with Python's literal-set semantics |
| CFG-09 | config or manifest exists but is unreadable (EACCES): `run`, `validate-config`, `install`, `gc`… | `cfgv.py:439` (`open` is outside the `UnicodeDecodeError` handler) | `PermissionError: [Errno 13] Permission denied: '.pre-commit-config.yaml'` | 3 | ✔c15 | K | identical. `validate-*` stops at this file; later files are not processed. |
| CFG-10 | `migrate-config`, or `autoupdate` (which migrates first), with a missing config | `migrate_config.py:192` | `FileNotFoundError: [Errno 2] No such file or directory: '.pre-commit-config.yaml'` | 3 | ✔c24 ✔01 | C-cfg (agrees with 03 §20.6) | `An error has occurred: InvalidConfigError: ` / `==> File .pre-commit-config.yaml` / `=====> .pre-commit-config.yaml is not a file`, exit 1. This is the text `run` already prints. |
| CFG-11 | `migrate-config` on a non-UTF-8 config (UTF-8 locale) | `migrate_config.py:192` (locale encoding) | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe9 in position 15: invalid continuation byte` | 3 | ✔c24 | C-cfg | the envelope `run` prints for this file (`cfgv.py:441-442`), with that same text as the leaf. snp reads UTF-8 on every platform; pre-commit on cp1252 Windows decodes the file instead (01 Q9). |
| CFG-12 | `migrate-config` on a list-root config whose re-indented fallback is invalid YAML (`%YAML 1.1\n---\n-   repo: …`) | `migrate_config.py:135-136` (`yaml_compose` unguarded) | `ScannerError: while scanning for the next token` / `found character that cannot start any token` / `  in "<unicode string>", line 2, column 5` | 3 | ✔c24 ✔01 | C-cfg. The file is left untouched; pre-commit also crashes before writing. | `An error has occurred: InvalidConfigError: ` / `==> File .pre-commit-config.yaml` / `=====> <yaml error text>`, exit 1 |
| CFG-13 | any of CFG-01/04/05/06/07 inside a cloned repo's `.pre-commit-hooks.yaml` | `clientlib.py:282-287` | same Type and text | 3 | src | C-cfg (`InvalidManifestError` envelope) | `An error has occurred: InvalidManifestError: ` / `==> File <store>/repoXXXXXXXX/.pre-commit-hooks.yaml` / … |
| CFG-14 | knock-on effects: commands that load the config for a side purpose. `install`/`uninstall` `_hook_types` (`install_uninstall.py:35-44`) and `gc` (`gc.py:73-78`) catch only `InvalidConfigError`; meta hooks reload the config. | — | crash as above | 3 | src | follows C-cfg | After conversion, `install`/`uninstall` fall back to `['pre-commit']` (pre-commit's own fallback for invalid configs), `gc` treats the config as dead and untracks it, and meta hooks fail with the validation text as their output. These deviations are intended. |

Neighbouring non-crash behaviour stays as specified in 01: a config path that is a directory gives `… is not a file` (exit 1), and an undecodable config in `run`/`validate-config` gives a clean `InvalidConfigError`.

#### X.4.2 Store, database and lock (`store.py`, `main.py:377-384`)

| ID | trigger | where | pre-commit text | exit | evidence | decision | snp text |
|---|---|---|---|---|---|---|---|
| ST-01 | store dir missing and cannot be created (read-only parent). Affects **every** command, including `validate-config`, `sample-config` and `clean`, because `Store()` is always built (`main.py:380`). | `store.py:70-71` | stdout `An unexpected error has occurred: PermissionError: [Errno 13] Permission denied: '<store>'`. The handler then calls `Store()` again (`error_handler.py:31`) and fails again: stderr shows two tracebacks joined by `During handling of the above exception, another exception occurred:`. No log. | **1** | ✔c13 | F + K | snp creates the store lazily (03 Q12), so commands that do not need it succeed. When it is needed: the same line 1, then `Failed to write to log at <store>/pre-commit.log` and the log body on stdout, exit 3, no traceback. |
| ST-02 | `PRE_COMMIT_HOME` names a regular file | `store.py:80,108` (`open('<file>/.lock')`) | stdout `An unexpected error has occurred: NotADirectoryError: [Errno 20] Not a directory: '<file>/.lock'`, plus the double-fault tracebacks on stderr | **1** | ✔c14 | as ST-01 | line 1 as shown (the first failing path may differ in snp; Tier B), `Failed to write to log at <file>/pre-commit.log` + body, exit 3 |
| ST-03 | the store exists but is not writable ("read-only store"), and a new clone or `local` env is needed. Cached hooks still pass (✔). | `store.py:162` (`mkdtemp`) | `[INFO] Initializing environment for local:six.`, then `PermissionError: [Errno 13] Permission denied: '<store>/repoXXXXXXXX'`, `Failed to write to log at <store>/pre-commit.log` and the full log body on stdout | 3 | ✔c13 | K | identical (the random suffix is Tier B) |
| ST-04 | read-only store and unstaged changes during `run` | `staged_files_only.py:73-75` | `[WARNING] Unstaged files detected.` / `[INFO] Stashing unstaged files to <store>/patch<ts>-<pid>.` / `PermissionError: [Errno 13] Permission denied: '<store>/patch<ts>-<pid>'` + log body on stdout. The worktree is untouched, because the patch is written before `git checkout`. | 3 | ✔c22 | K | identical. snp must keep the order: write and fsync the patch first, then touch the worktree. |
| ST-05 | another process holds a sqlite write lock on `<store>/db.db` for more than 5 s | first DB statement, `mark_config_used` (`main.py:384` → `store.py:232-235`); also `_new_repo` (`store.py:144-171`) and gc | `OperationalError: database is locked` | 3 | ✔c06 | F for config tracking: any DB error in `mark_config_used` is logged at debug level and ignored. K for repo lookup, insert and gc (busy timeout 5000 ms, as Python). | identical when K |
| ST-06 | `db.db` is not a sqlite file | same | `DatabaseError: file is not a database` | 3 | ✔c14 | F for config tracking; K otherwise | identical |
| ST-07 | `db.db` is an empty file | `store.py:78-79` (if the file exists it is assumed initialised) | `run` with env-less hooks passes. Anything that needs `repos` (gc ✔, clone, local env) fails with `OperationalError: no such table: repos`. | 3 | ✔c14 | F: on open, run `CREATE TABLE IF NOT EXISTS` for both tables | — |
| ST-08 | `db.db` is a directory | `sqlite3.connect` | `OperationalError: unable to open database file` | 3 | ✔c14 | F for config tracking; K otherwise | identical |
| ST-09 | `db.db` is a read-only file inside a writable store (so `readonly` is false, `store.py:65-68`) | `mark_config_used` | `OperationalError: attempt to write a readonly database` | 3 | ✔c22 | F (config tracking is best-effort) | — |
| ST-10 | stale DB row: a `repoXXXXXXXX` dir was deleted (by the user or by a partial gc failure, CMD-07) | `store.py:142-153` returns the row without checking the path | local env language: `CalledProcessError: command: ('<store>/repoX/py_env-python3.12/bin/python', '-mpip', 'install', '.')` … `    ERROR: Directory '.' is not installable. Neither 'setup.py' nor 'pyproject.toml' found.` (exit 3). Remote repo: `InvalidManifestError … is not a file` (exit 1, 03 §7.6). | 3 / 1 | ✔c06 ✔03 | F: check that the cached dir exists (and, for clones, that it contains `.git`). Otherwise delete the row and re-create it, printing the normal `[INFO] Initializing environment for …` line. | — |
| ST-11 | `<store>/.lock` cannot be opened (a directory, EACCES) | `file_lock.py:345` | `IsADirectoryError` / `PermissionError: [Errno …] …: '<store>/.lock'` | 3 | src | K | identical |
| ST-12 | Windows: `msvcrt.locking` fails with an errno other than EDEADLOCK | `file_lock.py:296-308` | `PermissionError` / `OSError` | 3 | src | K (snp uses `File::lock`) | Tier B |

#### X.4.3 git subprocesses (format X.2.2 A unless stated)

| ID | trigger | where | pre-commit text | exit | evidence | decision | snp text |
|---|---|---|---|---|---|---|---|
| GIT-01 | clone of a missing or unreachable repo, or an unknown rev | `store.py:195-210` (the shallow fetch fails silently and the full clone runs) | `[INFO] Initializing environment for /nonexistent/repo.`, then `CalledProcessError: command: ('/usr/bin/git', 'fetch', 'origin', '--tags')` / `return code: 128` / `stdout: (none)` / `stderr:` / `    fatal: '/nonexistent/repo' does not appear to be a git repository` / `    fatal: Could not read from remote repository.` / `    ` / `    Please make sure you have the correct access rights` / `    and the repository exists.`. An unknown rev shows `…('/usr/bin/git', 'checkout', '<rev>')` (03 §1.4). | 3 | ✔c15 ✔03 | K | identical. The command shown is the **fallback** step that failed. |
| GIT-02 | autoupdate: the fetch fails for one repo | `autoupdate.py:48-53` | `CalledProcessError: command: ('/usr/bin/git', '-c', 'core.useBuiltinFSMonitor=false', '-C', '<tmp>', 'fetch', 'origin', 'HEAD', '--quiet', '--filter=blob:none', '--tags')` …. All repos are aborted and the config is not written. Other workers are joined first and their results discarded. | 3 | ✔c15 | C-err (03 §20.6 and Q7) | `[<repo>] ` + the X.2.2 A block (multi-line). The other repos are updated and written. Exit 1. |
| GIT-03 | `try-repo <bad repo>` | `git.py:170-172` | `CalledProcessError: command: ('/usr/bin/git', 'ls-remote', '--exit-code', '/nonexistent/repo', 'HEAD')` …, with the same stderr as GIT-01 | 3 | ✔c15 | K | identical |
| GIT-04 | `run --from-ref <bad> --to-ref <ref>` | `git.py:158-167` (`...` fails, then `..` is retried) | The stash is taken and restored first. Then `CalledProcessError: command: ('/usr/bin/git', 'diff', '--name-only', '--no-ext-diff', '-z', 'bogus..HEAD')` / `return code: 128` / `stdout: (none)` / `stderr:` / `    fatal: ambiguous argument 'bogus..HEAD': unknown revision or path not in the working tree.` / `    Use '--' to separate paths from revisions, like this:` / `    'git <command> [<revision>...] -- [<file>...]'` | 3 | ✔c22 | K | identical (the two-dot command is shown) |
| GIT-05 | non-UTF-8 path bytes in `git ls-files -z` (`-a`), `git diff --staged -z` (default), the merge-conflict diff or the intent-to-add list | `util.py:116-120` (strict `.decode()`), `git.py:135-155` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 27: invalid start byte` (`-a`); `… position 3 …` (staged) | 3 | ✔c04 | F (02 Q5): paths stay bytes/`OsString`. `files`/`exclude` and `types` match a lossy UTF-8 view; argv gets the raw bytes (Unix). Not applicable on Windows (UTF-16 paths). | hooks run normally |
| GIT-06 | `run --files <non-UTF-8 name>` that exists | `xargs.py:241-249` (strict `.encode(fsencoding)`), after `<name>…` | `h…An unexpected error has occurred: UnicodeEncodeError: 'utf-8' codec can't encode character '\udcff' in position 3: surrogates not allowed` | 3 | ✔c04 | F (as GIT-05) | — |
| GIT-07 | a `MERGE_MSG` conflict line with non-UTF-8 bytes | `git.py:104-111` | `UnicodeDecodeError: …` | 3 | src | F (as GIT-05) | — |
| GIT-08 | non-UTF-8 git **stderr** on a `cmd_output` call (e.g. messages in a legacy-locale encoding) | `util.py:118-119` also decodes stderr | `UnicodeDecodeError` replaces git's real error | 3 | src | F: decode stderr lossily for display | the underlying git error (a K row) or success |
| GIT-09 | `git write-tree`, `git ls-files --unmerged` or `git checkout -- .` fails | `staged_files_only.py:52,81`; `run.py:326` | `CalledProcessError: command: ('/usr/bin/git', 'write-tree')` … | 3 | src | K | identical |
| GIT-10 | `git rev-parse --git-dir` echoes `--git-dir` (git < 2.5) | `git.py:76-83` | `AssertionError: unreachable: no git dir` | 3 | src | C-err: snp requires a supported git (a version check → FatalError, exit 1) | `An error has occurred: FatalError: …` (wording owned by 03 §20.2) |

#### X.4.4 File classification (`run.py:73-126`, identify)

| ID | trigger | where | pre-commit text | exit | evidence | decision | snp text |
|---|---|---|---|---|---|---|---|
| CLS-01 | an earlier hook deletes a file whose tags were not computed yet. Repro: an `always_run` hook with `files: ^a\.txt$` and `entry: rm -f b.txt`. | `identify.py:40-44` via `run.py:77-79` | after the previous hook's block: `An unexpected error has occurred: ValueError: b.txt does not exist.` (the stash is restored first) | 3 | ✔c05 (`-a` and staged) | F (02 Q6): drop the vanished file from this hook and all later hooks. A file whose tags were cached before the deletion is still passed (parity). | — |
| CLS-02 | a regular file that is unreadable, is not executable, and whose name gives no `text`/`binary` tag (no or unknown extension) | `identify.py:135-139` (`open`) | `PermissionError: [Errno 13] Permission denied: 'noext'`. No hook line is printed yet, because classification runs before `<name>…`. | 3 | ✔c20 (a `.txt` name never opens the file ✔) | K, fail closed: a guard must not silently skip an unreadable staged file | identical |
| CLS-03 | a FIFO passed via `--files` (git cannot track FIFOs) | `identify.py:135-139` opens it in blocking mode | **hangs forever**, with no output | — | ✔c20 (`timeout 5` → 124) | F: for files that are not regular, a directory, a symlink or a socket, open with `O_RDONLY\|O_NONBLOCK` and read at most 1024 bytes. A FIFO without a writer is empty ⇒ `text`. `/dev/null` ⇒ `text` and `/dev/zero` ⇒ `binary`, both as pre-commit. | — |
| CLS-04 | an lstat error other than ENOENT during tagging (EACCES on a parent dir, ELOOP, ENAMETOOLONG), after the initial `lexists` filter | `identify.py:42-44` maps **every** `OSError` to this text | `ValueError: <f> does not exist.` | 3 | src | F, as CLS-01 | — |
| CLS-05 | implementation note: `file-identify` 0.4.0 also blocks on FIFOs (`fs::File::open`, `lib.rs:865`) | — | — | — | src | F in snp's wrapper or upstream | — |

#### X.4.5 Building and running the hook command (`lang_base.py`, `xargs.py`, `run.py`)

| ID | trigger | where | pre-commit text | exit | evidence | decision | snp text |
|---|---|---|---|---|---|---|---|
| EXE-01 | `entry: ""` or whitespace only, with `args: []` | `lang_base.py:174-178` → `parse_shebang.py:77` (`cmd[0]`) | `<name>…An unexpected error has occurred: IndexError: tuple index out of range` | 3 | ✔c09. Related, verified: `entry: ""` + `args: [echo, hi]` runs `echo`; `entry: "''"` fails the hook with ``Executable `` not found``; `entry: "#"` gives ``Executable `#` not found``. | C-hook (supersedes 04b robustness item 9 and 04a §14.15) | ``Executable `` not found`` |
| EXE-02 | unbalanced quote or trailing backslash in `entry` | `lang_base.py:175` (`shlex.split`) | `ValueError: No closing quotation` (✔c09) / `ValueError: No escaped character` (src) | 3 | ✔c09 | C-hook | `ValueError: No closing quotation` / `ValueError: No escaped character` (one line, in the style of 04a §12 for pygrep) |
| EXE-03 | `entry` + `args` alone exceed the xargs limit while files are passed. The limit is min(ARG_MAX−2048−env, 128 KiB) on POSIX, 32 KiB−2 KiB on Windows, and ≈ 8 KiB−len(cmd.exe)−1028 for `.bat`/`.cmd`. A single filename over the limit cannot happen on POSIX (PATH_MAX < limit). | `xargs.py:280-287` | `<name>…An unexpected error has occurred: ArgumentTooLongError: <first file in shuffled order>` | 3 | ✔c19 (`ArgumentTooLongError: .pre-commit-config.yaml`) | C-hook, with the text pre-commit already prints when exec fails with E2BIG (`pass_filenames: false`, ✔c19) | `[Errno 7] Argument list too long: '<resolved exe>'` |
| EXE-04 | `entry` is an executable script that cannot be read (mode 0111) | `parse_shebang.py:15-19` → `identify.py:205-219` | `<name>…An unexpected error has occurred: PermissionError: [Errno 13] Permission denied: './s.sh'` | 3 | ✔c20 | C-hook, rendered like `util.py:86-87` (Popen `OSError`) | `[Errno 13] Permission denied: './s.sh'` |
| EXE-05 | `log_file` whose directory does not exist, which is a directory, or which is unwritable, on a hook that prints output | `output.py:253-268` (`open(logfile, 'ab')` comes **before** writing the output) | after `- hook id`, `- exit code` and a blank line: `An unexpected error has occurred: FileNotFoundError: [Errno 2] No such file or directory: 'nodir/x.log'` or `IsADirectoryError: [Errno 21] Is a directory: 'logd'`. **The hook's output is lost.** The stash is restored. | 3 | ✔c08 (`log_file: ""` disables logging ✔) | F: print the output block first so it is never lost, then try the log file. On failure print the warning and continue; the exit code is unchanged. | `[WARNING] Could not write to log_file nodir/x.log: [Errno 2] No such file or directory: 'nodir/x.log'` |
| EXE-06 | pty allocation fails in color mode (e.g. a container without `/dev/ptmx`), or a pty `os.read` error other than EIO | `util.py:133,188-194` | `FileNotFoundError: [Errno 2] …` / `OSError: out of pty devices` | 3 | src | F: fall back to pipes for that hook | — |
| EXE-07 | a pygrep `entry` pattern that Python rejects with something other than `re.error`: `a{4294967295}`, nesting ≥ ~490, or `(?a)(?L)` on bytes | `pygrep.py:123`, in a subprocess | not exit 3: the hook output is a Python traceback ending in `OverflowError: the repetition number is too large`, `RecursionError: maximum recursion depth exceeded` or `ValueError: ASCII and LOCALE flags are incompatible`, with `- exit code: 1` | hook 1 | src (bytes probe ✔) | C-hook (native pygrep, 04a §12) | that last line only, e.g. `OverflowError: the repetition number is too large`; hook exit code 1 |
| EXE-08 | a language `in_env`/`run_hook` raises (e.g. lua without `luarocks`) | `languages/*` | `<name>…An unexpected error has occurred: CalledProcessError: command: ('luarocks', 'config', '--lua-ver')` / `return code: 1` / `stdout:` / ``    Executable `luarocks` not found`` / `stderr: (none)` | 3 | ✔04b §2.7 | K (environmental) | identical |

#### X.4.6 Stash (`staged_files_only.py`)

| ID | trigger | where | pre-commit text | exit | evidence | decision | snp text |
|---|---|---|---|---|---|---|---|
| STASH-01 | `git checkout -- .` fails while stashing | `:81` | `CalledProcessError: command: ('/usr/bin/git', '-c', 'submodule.recurse=0', 'checkout', '--', '.')` …. The `finally` block still applies the patch. | 3 | src | K | identical |
| STASH-02 | on restore, `git apply` fails twice: after `[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...` and the `checkout -- .` | `:85-96` | `CalledProcessError: command: ('/usr/bin/git', '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn', '<store>/patch<ts>-<pid>')` …. `[INFO] Restored changes…` is not printed. The user's unstaged changes now exist **only** in the patch file. | 3 | src | K+. The restore algorithm itself (02 §21.10 vs 06) is not decided here. | after the log line: `[ERROR] Your unstaged changes are saved in <store>/patch<ts>-<pid>; restore them with: git apply <store>/patch<ts>-<pid>` |
| STASH-03 | stdout fails during the restore (EPIPE). The `logger.warning` at `:88` raises **before** the rollback and the second apply run. | `:88-96` + `logging_handler.py:25-31` (no `handleError`) | the rollback and the second apply are skipped; tracebacks; exit 120 | 120 | src (EPIPE behaviour ✔c23) | F: the printer never raises, and the restore always runs to completion | — |

STASH-04: with a read-only store, see ST-04.

#### X.4.7 Environments and install state (`repository.py`, `languages/*`)

| ID | trigger | where | pre-commit text | exit | evidence | decision | snp text |
|---|---|---|---|---|---|---|---|
| ENV-01 | `.install_state_v1` malformed and `.install_state_v2` absent | `repository.py:37-62` | `JSONDecodeError: Expecting property name enclosed in double quotes: line 1 column 2 (char 1)` (`{bad`); `JSONDecodeError: Expecting value: line 1 column 1 (char 0)` (empty); `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 0: invalid start byte` (UTF-8 locale; with cp1252 on Windows this becomes a `JSONDecodeError`) | 3 | ✔c06 | F: unparsable or undecodable state means "not installed", so the env is reinstalled normally (`[INFO] Installing environment for …`). Valid JSON with a different value already triggers a reinstall in pre-commit. | — |
| ENV-02 | `.install_state_v1` unreadable | `repository.py:42` | `PermissionError: [Errno 13] Permission denied: '<store>/repoX/py_env-python3.12/.install_state_v1'` | 3 | ✔c06 | K | identical |
| ENV-03 | install, health or version failures in language backends | `languages/*`, `repository.py:84-106` | per language, e.g. `CalledProcessError …`, ``AssertionError: for now, pre-commit requires system-installed <x> -- you selected `language_version: <v>` ``, `AssertionError: BUG: expected environment for <lang> to be healthy immediately after install, please open an issue describing your environment\n\nmore info:\n\n<msg>`, `RuntimeError: failed to find interpreter for …`, `ValueError: Could not find a version matching your system requirements (os=…; arch=…)`, KeyError/IndexError in dotnet/R | 3 | ✔04a ✔04b | K. The per-language tables (04a §14.15, 04b §1.8) stay authoritative for the texts. | identical |
| ENV-04 | a stale env dir cannot be removed (EACCES) | `repository.py:81-82` | `PermissionError: [Errno 13] …` | 3 | src | K | identical |

#### X.4.8 `hook-impl` and pre-push (`hook_impl.py`)

| ID | trigger | where | pre-commit text | exit | evidence | decision | snp text |
|---|---|---|---|---|---|---|---|
| HI-01 | `--hook-type` is not a known type | `:211`, reached **after** the legacy hook ran and the config check passed | `AssertionError: unexpected hook type: bogus` | 3 | ✔c10 | K, keeping the order legacy → config check → assert. This rejects 03 §20.4's clap validation, which would exit 2 **before** running the legacy hook; that difference is observable for scripts installed by a newer pre-commit. | identical |
| HI-02 | wrong argument count | `:189-209` | stderr `hook-impl for pre-push expected 2 arguments but got 1: ['origin']` | 1 | ✔c10 | K | identical |
| HI-03 | a pre-push stdin line with fewer than 4 whitespace-separated fields, including an empty line after a skipped deletion line | `:128-130` | `ValueError: not enough values to unpack (expected 4, got 2)` / `(expected 4, got 0)` | 3 | ✔c10 | K, fail closed. This resolves 03 §20.4/Q9 ("skip"): git never produces such lines, and skipping one would let a malformed push through unchecked. | identical |
| HI-04 | pre-push stdin that is not valid UTF-8 (ref names may contain such bytes) | `:128` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 12: invalid start byte` | 3 | ✔c10 | F. Valid UTF-8 uses Python `str.splitlines()` + `str.rsplit(maxsplit=3)` exactly (02 §16.3). Invalid UTF-8 uses the same algorithm on bytes, with ASCII whitespace and `bytes.splitlines()` (`\n`, `\r`, `\r\n`). Env vars carry the raw bytes (Unix). | — |
| HI-05 | `local_sha` is unknown or garbage, and the remote sha is null or not present locally | `:143-146` | stderr from git (not captured): `fatal: bad object 1234…`. Then stdout: `An unexpected error has occurred: CalledProcessError: Command '('git', 'rev-list', '1234567890123456789012345678901234567890', '--topo-order', '--reverse', '--not', '--remotes=origin')' returned non-zero exit status 128.`. A non-hex sha gives stderr `fatal: ambiguous argument 'zzzz': unknown revision or path not in the working tree.` plus 2 lines. | 3 | ✔c10 | K (format X.2.2 B: bare `'git'`, stderr inherited) | identical |
| HI-06 | `git rev-list --max-parents=0 <sha>` or `git rev-parse <first>^` fails | `:151-164` | `CalledProcessError: Command '('git', 'rev-list', '--max-parents=0', '<sha>')' returned non-zero exit status 128.` | 3 | src | K | identical |
| HI-07 | the legacy hook's shebang interpreter is missing, or `<type>.legacy` is a directory (which passes `os.access(X_OK)`) | `:40-46` → `parse_shebang.py:46-62` | ``ExecutableNotFoundError: Executable `/nonexistent/interp` not found`` / ``ExecutableNotFoundError: Executable `<abs>/.git/hooks/pre-commit.legacy` is a directory`` | 3 | ✔c21 | C-err: treat it as a failed legacy hook. Print the message on stderr, set the legacy rc to 1 and continue with snp's hooks, so the exit code is `1 \| run`. This is what git itself does with a broken hook (fail, not crash). | stderr ``Executable `/nonexistent/interp` not found`` |
| HI-08 | `PRE_COMMIT_RUNNING_LEGACY` is set (migration-mode recursion) | `:22-29` | the SystemExit text from 03 §6.3, on stderr | 1 | ✔03 | K | identical |

#### X.4.9 `hazmat` (`commands/hazmat.py`): direct CLI vs hook `entry`

| ID | trigger | where | pre-commit text | exit | evidence | decision | snp text |
|---|---|---|---|---|---|---|---|
| HZ-01 | `pre-commit hazmat` with no tool | `:159-160`. `args.cmd` is read before the dispatch, so the `NotImplementedError` at `:168` is never reached. | `AttributeError: 'Namespace' object has no attribute 'cmd'` | 3 | ✔c11 | C-err: usage error, exit 2 (subcommand required) | usage + `error:` line (Tier B) |
| HZ-02 | `pre-commit hazmat n1\|cd\|ignore-exit-code <missing-exe> …` | `:140,145,152` | ``ExecutableNotFoundError: Executable `nonexistent-exe` not found`` | 3 | ✔c11 | K | identical |
| HZ-03 | an empty command: `hazmat ignore-exit-code`, `hazmat n1 --`, `hazmat cd sub --` | `normalize_cmd(())` | `IndexError: tuple index out of range` | 3 | ✔c11 (the first form); src | C-err, as EXE-01: exit 3 | ``An unexpected error has occurred: ExecutableNotFoundError: Executable `` not found`` |
| HZ-04 | `hazmat cd <missing dir> CMD --` | `:141` (`subprocess.call(cwd=…)`) | `FileNotFoundError: [Errno 2] No such file or directory: 'nodir'` (Windows: a CreateProcess error, Tier B) | 3 | ✔c11 | K | identical |
| HZ-05 | no `--` | `:119-124` | stderr ``hazmat entry must end with `--` `` | 1 | ✔c11 | K | identical |
| HZ-06 | the same failures **inside a hook `entry`**. `pre-commit hazmat …` is rewritten to `python -m pre_commit.commands.hazmat` (`lang_base.py:176-177`), which has no error handler. | — | **Never exit 3.** The hook output is a full Python traceback ending in ``pre_commit.parse_shebang.ExecutableNotFoundError: Executable `nonexistent-exe` not found``, with `- exit code: 1`. argparse failures print `usage: hazmat.py [-h] {cd,ignore-exit-code,n1} ...` / `hazmat.py: error: argument tool: invalid choice: '.pre-commit-config.yaml' (choose from 'cd', 'ignore-exit-code', 'n1')` with `- exit code: 2`. A bad prefix gives `unexpected file without prefix='nodir/': .pre-commit-config.yaml` with `- exit code: 1`; the prefix check runs before the cwd check. | hook 1 / 2 | ✔c11 | C-hook, without the traceback | ``Executable `nonexistent-exe` not found`` (exit code 1). The argparse text is kept with prog `hazmat.py` (Tier B, exit code 2). The SystemExit texts are identical (exit code 1). |

#### X.4.10 Commands

| ID | trigger | pre-commit text | exit | evidence | decision / snp text |
|---|---|---|---|---|---|
| CMD-01 | `install` when `.git/hooks/<type>` is a directory | `IsADirectoryError: [Errno 21] Is a directory: '.git/hooks/pre-commit'` | 3 | ✔03 §2.5 | K |
| CMD-02 | `install` when the hooks dir or script is unwritable | `PermissionError: [Errno 13] …` | 3 | src | K |
| CMD-03 | `install -f` when `<type>.legacy` is a directory (`install_uninstall.py:80-81`) | Linux: `IsADirectoryError: [Errno 21] Is a directory: '…/pre-commit.legacy'`. macOS: `PermissionError: [Errno 1] Operation not permitted: '…'`, because `unlink(2)` on a directory returns EPERM there. | 3 | src | K (the platform text follows the errno) |
| CMD-04 | `uninstall`: `os.remove`/`os.replace` fails (`:156,160`) | OSError family | 3 | src | K |
| CMD-05 | `autoupdate`: `AssertionError: could not find rev lines` (03 §11.3) | as quoted | 3 | ✔03 | K. 03 §20.6 makes it rarer by treating EOF as a line end. |
| CMD-06 | `autoupdate` git failures | see GIT-02 | 3 | ✔c15 | C-err |
| CMD-07 | `gc` when a tracked repo dir is missing | `FileNotFoundError: [Errno 2] No such file or directory: '<store>/repoXXXXXXXX'`. The DB transaction is rolled back, so **every later `gc` fails the same way** (sticky). Dirs removed before the failure stay removed while their rows come back (→ ST-10). | 3 | ✔c12 (two consecutive runs, row still present) | F: ignore ENOENT while removing and commit the row deletion. Output `<n> repo(s) removed.`, where `n` counts the rows removed. |
| CMD-08 | `gc`: other `rmtree` failures (`_handle_readonly` retries chmod but can still fail) | `PermissionError …` | 3 | src | K |
| CMD-09 | `gc` with a tracked config that crashes the loader (any CFG row) | the whole gc crashes | 3 | src | follows CFG-14: the config is treated as dead |
| CMD-10 | `clean`: `rmtree` failures | OSError family | 3 | src | K |
| CMD-11 | `validate-config` / `validate-manifest` on an unreadable file | see CFG-09 | 3 | ✔c15 | K |
| CMD-12 | `try-repo` | see GIT-03 | 3 | ✔c15 | K |

#### X.4.11 Error-handler and output faults (Python double faults; not exit 3)

| ID | trigger | pre-commit behaviour | exit | evidence | decision / snp text |
|---|---|---|---|---|---|
| EH-01 | the store is unusable inside the handler | see ST-01 and ST-02 | 1 | ✔c13 ✔c14 | F: never double-fault |
| EH-02 | `<store>/pre-commit.log` is a directory, or an unwritable file in a writable store | prints `Check the log at <store>/pre-commit.log`, then a stderr traceback ending in `IsADirectoryError: [Errno 21] Is a directory: '<store>/pre-commit.log'` | 1 | ✔c21 | F: open the log **before** printing line 2. On failure print `Failed to write to log at <path>` and the body on stdout. Exit 3. |
| EH-03 | stdout closed early (`pre-commit run \| head -c 20`, or a GUI closing git's stderr pipe; git sends hook stdout to its stderr) | `BrokenPipeError` on the next write. The handler's own write raises again. stderr shows 2–3 chained `BrokenPipeError: [Errno 32] Broken pipe` tracebacks plus `Exception ignored in: <_io.TextIOWrapper name='<stdout>' mode='w' encoding='utf-8'>` / `BrokenPipeError: [Errno 32] Broken pipe`. The remaining hooks do not run, the stash **is** restored (✔c23: staged and unstaged diffs intact), and no log is written. | **120** (CPython's status when flushing stdout fails at shutdown; 1 if nothing was left buffered) | ✔c18 ✔c23 | F for the noise, K for the semantics: stop writing to stdout, start no further hooks, finish the restore, exit 120, print nothing on stderr, write no crash log. This resolves 03 §20.9 (X.7 R4). |
| EH-04 | `KeyboardInterrupt` | 02 §17 | 130 | ✔02 | K |
| EH-05 | a Rust panic (snp only) | — | — | — | `An unexpected error has occurred: PanicException: <payload>`, then `Check the log at …`, exit 3. The log carries the location and the backtrace. The type token follows PyO3's name for Rust panics. |

### X.5 Secondary messages (no crash) that no other spec quotes

| ID | when | exact text | stream | evidence | snp |
|---|---|---|---|---|---|
| SM-01 | a store row for (key, rev) is created (`store.py:160`), during hook building (`all_hooks`, config order), before any `Installing environment` line | `[INFO] Initializing environment for <key>.`. Key forms (see below): `local`; `local:six,attrs` (deps in **config order**, joined by `,` without spaces or sorting); `<repo>`; `<repo>:six,attrs==23.1.0`. A remote repo with `additional_dependencies` prints **two** lines: the bare clone (for the manifest), then the deps clone. | stdout | ✔c06 ✔c13 ✔c17 | K, verbatim |
| SM-02 | an env is installed (`repository.py:66-68`) | `[INFO] Installing environment for <hook.src>.` / `[INFO] Once installed this environment will be reused.` / `[INFO] This may take a few minutes...`. `<hook.src>` is the repo URL or `local`, **without** deps. All `Initializing` lines come before all `Installing` lines. | stdout | ✔c17 | K |
| SM-03 | a `files`/`exclude` pattern triggers an `re` FutureWarning (CPython ≥ 3.7) | Two lines: `<site-packages>/cfgv.py:386: FutureWarning: Possible nested set at position <n>` and `  re.compile(v)`. Other messages: `Possible set difference at position <n>`, `Possible set intersection …`, `Possible set symmetric difference …`, `Possible set union …`. `<n>` is the index of the inner `[` or of the first doubled operator char (`[[]` → 1, `[a&&b]` → 2); for the range form `a--` it is `tell-2`. See below for when it is printed. | stderr | ✔c02 ✔c16 | D: snp prints nothing. The pattern keeps Python's literal semantics. |
| SM-04 | warnings inside meta-hook output. The meta hooks run as `python -m pre_commit.meta_hooks.*` with no logging handler, so Python's `logging.lastResort` prints WARNING/ERROR records **bare** (no `[WARNING]` prefix, no color) on the hook's stderr, which is merged into its output. INFO records (e.g. `Initializing environment`) are dropped. | `check-useless-excludes` output contains `Unexpected key(s) present at root: foo`. `check-hooks-apply` (it calls `all_hooks`) also contains `Unexpected key(s) present on local => e: bogus_key`. Mutable-rev and slash warnings appear the same way. These lines come before the hook's own lines (`e does not apply to this repository`). They are visible only when the hook fails or is `verbose`. | hook output | ✔c16 | K for the bare warning lines (native meta hooks replay the config-load warnings into their output buffer, in the same order). D for the SM-03 lines. |
| SM-05 | pre-push, when a remote sha is not present locally (common after someone else pushed) | `fatal: Not a valid object name <sha>^{commit}`, from `git cat-file -e` (`hook_impl.py:117`). It is harmless and the hook proceeds. | stderr (inherited) | ✔c10 | K (resolves 02 Q11: keep parity) |
| SM-06 | exec fails with E2BIG (`pass_filenames: false` + huge `args`) | hook output `[Errno 7] Argument list too long: '/usr/bin/echo'`, `- exit code: 1` | stdout | ✔c19 | K |
| SM-07 | `entry` whose first token is empty or `#` | ``Executable `` not found`` / ``Executable `#` not found`` (POSIX shlex without comments) | hook output | ✔c09 | K |
| SM-08 | EPIPE noise (EH-03) | the tracebacks and the `Exception ignored in: …` lines | stderr | ✔c18 | D |
| SM-09 | argparse (exit 2), re-verified | `PRE_COMMIT_COLOR=bogus pre-commit sample-config --color never` → exit 0 (the env default is converted only when `--color` is absent). Without the flag: `usage: pre-commit sample-config [-h] [--color {auto,always,never}]` / `pre-commit sample-config: error: argument --color: invalid use_color value: 'bogus'`, exit 2. `pre-commit run --bogus` → the **top-level** usage (3 lines) + `pre-commit: error: unrecognized arguments: --bogus`, exit 2. | stderr | ✔ (c02 dir) | as 10 CLI-07 (Tier B text, Tier A exit code and stream) |
| SM-10 | `PYTHONWARNINGS` (read by CPython, not by pre-commit) | `ignore` suppresses SM-03; `error` turns it into CFG-08; it is also inherited by Python hooks | — | ✔c02 | snp ignores it and passes it through to hooks unchanged |

- **SM-01 key forms:**
  - `local`, for the first local hook of an env language without deps;
  - `local:six,attrs`;
  - `<repo>` and `<repo>:six,attrs==23.1.0`.
- **SM-01 is not printed** for env-less local hooks (`system`, `script`, `unsupported*`, `fail`, `pygrep`, `docker_image`), because `_prefix` uses the cwd (`repository.py:152-159`).
- **SM-01 frequency:** once per store lifetime per key. `try-repo` uses a fresh temp store, so it prints the line on every run.
- **SM-03 deduplication:** each distinct message text is printed once per process, from the warnings registry and because `re` caches compiled patterns. The same pattern in two hooks or two files prints once. It is printed again by every meta-hook subprocess, inside their output. Its order relative to stdout lines is arbitrary.

### X.6 Complete exit-code map (pre-commit 4.6.2 → snp)

| Code | pre-commit | snp |
|---|---|---|
| 0 | success | same |
| 1 | hook failure; `FatalError`; `InvalidConfigError`/`InvalidManifestError`; `SystemExit("msg")` (hook-impl arg count, hazmat, migration mode); **any Python double fault** (ST-01, ST-02, EH-02) | the same, minus double faults. Also covers C-cfg and C-hook rows and GIT-02. |
| 2 | argparse usage errors | clap usage errors, plus HZ-01 |
| 3 | every K row and every row in X.4 that snp converts | K and K+ rows, plus EH-05 panics |
| 120 | EPIPE on stdout (EH-03), or 1 depending on Python's buffer state | always 120 |
| 130 | Ctrl-C | same |
| 101 | — | **never**: the Rust panic default must not leak |

### X.7 Contradictions resolved here

- **R1. hazmat exit code (03 §15 vs the critic).** Verified on 4.6.2 (✔c11):
  - only a **direct** `pre-commit hazmat …` call ends in exit 3 (HZ-02/03/04), or in AttributeError exit 3 when no tool is given (HZ-01);
  - inside a hook `entry` there is no error handler: the result is a Python traceback with `- exit code: 1`, or the argparse text with `- exit code: 2` (HZ-06).

  03 §15's "surface as exit 3" applies only to the direct call. **Decision:** HZ rows.
- **R2. Empty entry, shlex errors, ArgumentTooLongError.** 04a §14.15 ("same exit code; better message") and 04b robustness item 9 ("same type and text, exit 3") disagree with 01 Q3 ("clean errors"). **Decision:** C-hook (EXE-01…04).

  Rationale: these errors are deterministic and local to one hook, and pre-commit itself already turns `Executable … not found` and Popen `OSError` into hook failures. The texts reuse pre-commit's own strings (``Executable `` not found``, `[Errno 7] Argument list too long: '<exe>'`). The exit code stays non-zero, so this is fail-closed.
- **R3. Pre-push stdin.** 02 §16.3 describes the Python semantics correctly. 03 §20.4 (`\n`/`\r` split, `rsplitn` over ASCII whitespace, skipping malformed lines) is wrong on two counts:
  - **splitting:** valid UTF-8 must use Python `str.splitlines()` (`\n`, `\r`, `\r\n`, `\x0b`, `\x0c`, `\x1c`, `\x1d`, `\x1e`, `\x85`, ` `, ` `) and `str.rsplit(maxsplit=3)` over Unicode whitespace;
  - **malformed lines:** pre-commit crashes, and snp keeps that (HI-03, fail closed); it is not a skip.

  Only non-UTF-8 input gets the byte-level fallback (HI-04).
- **R4. EPIPE.** 03 §20.9 says "stop writing and exit with the code computed so far". That can exit 0 with hooks never run: a fail-open in a git hook whose output pipe closed. pre-commit aborts and exits 120 (✔c18). **Decision:** EH-03.
- **R5. Crash log (03 §20.5 item 13 `snp.log`, 02 Q7, 03 Q16, 10 Q9).** **Decision:** keep the file name `pre-commit.log`.
  - **Location:** in the root of whatever store directory the store decision picks (that decision stays with 03 and is not made here).
  - **Why this name:**
    - the banner `Check the log at …/pre-commit.log` stays byte-identical in shape;
    - pre-commit's own test assertions (10 ERR-04/05) and CI recipes that `cat` the log keep working;
    - in a shared `PRE_COMMIT_HOME`, overwriting it matches pre-commit, which overwrites it (`'wb'`) on every crash.
  - **Skeleton, per 10 Q9:**

    ```
    ### version information

    ```
    pre-commit version: <compat version> (snp <ver>)
    git --version: <`git --version` output, lossy, rstrip>
    sys.version:
        snp <ver> (<commit>, rustc <ver>) [<target triple>]
    sys.executable: <current_exe()>
    os.name: <posix|nt>
    sys.platform: <linux|darwin|win32|…>
    ```

    ### error information

    ```
    <banner line 1 bytes>
    ```

    ```
    <error source chain, one per line; backtrace if available>
    ```
    ```

  - **How to write it:** open with `O_NOFOLLOW` (Unix) plus truncate. Write it only when the store is writable; otherwise put the body on stdout behind `Failed to write to log at …` (X.1).
- **R6. hook-impl unknown `--hook-type`.** 03 §20.4 (clap error, exit 2) runs before the legacy hook, while pre-commit runs the legacy hook and the config check first and then asserts (✔c10). **Decision:** keep pre-commit's order and text (HI-01).
- **R7. 01 §16 / 01 Q3 crash parity.** Confirmed: the CFG rows become clean validation errors. Rows CFG-05/06/07 extend 01 §16 with the regex cases found by the critic and verified here.
- **R8. 01 §12.9 UTF-8 error mapping.** It is missing the `bytes in position <i>-<j>: unexpected end of data` form (X.2.3).
- **R9. `PRE_COMMIT_COLOR` and the argparse prefix (02 §2.2, 03 §1.1).** The critic's corrections were re-verified (SM-09): an invalid env value errors only when `--color` is absent, and unrecognized arguments are reported by the **top-level** parser.
- **R10. 02 Q11 (`git cat-file -e` noise).** Keep it (SM-05).
- **Evidence for the open regex-engine contradiction (01 §13 two-tier vs 07 D4 fancy-regex only vs 10 Q6).** This addendum does not decide it, but CFG-05/06 constrain it (verified with `scripts-g4/nestprobe`, regex 1.13.1 and fancy-regex 0.19.2):

  | Engine | Nesting limit | Notes |
  |---|---|---|
  | fancy-regex 0.19.2 | **hard-coded `MAX_RECURSION = 64`** (`src/lib.rs:142`), not configurable | ≥ 64 nested groups fails with `Parsing error at position 63: Pattern too deeply nested`. Python accepts up to ~480. |
  | regex 1.13.1 | `nest_limit` defaults to **250** | `RegexBuilder::nest_limit(1000)` accepts 480. |

  Counted repetition such as `a{100000}` compiles in both crates, but `a{1000000}` and `a{4294967294}` fail on the 10 MiB size limit, while Python accepts and matches them. So "fancy-regex as the single engine" diverges on accept/reject at depth 64–480. The Python-syntax front end must therefore:
  - enforce Python's verdicts itself: depth > 480 → CFG-06, repeat ≥ 2^32−1 → CFG-05;
  - send deep but fancy-free patterns to `regex` with `nest_limit ≥ 481`.

  Patterns that are valid Python but too large for any engine need a policy (Open questions).
- **Out of scope for this addendum, not decided:**
  - hook stdin;
  - YAML error-text parity;
  - async runtime;
  - toolchain pin;
  - env-dir naming;
  - install-state file names (ENV-01's policy applies to whichever v1 file is read);
  - store location and `PRE_COMMIT_HOME` (the crash log follows it);
  - executable lookup;
  - CPU count;
  - the fsmonitor flag;
  - the stash restore algorithm (STASH-02 only adds a hint);
  - in-process emulation granularity.

### X.8 Implementation notes for snp

1. **One error type** in `snp-core`:
   ```rust
   enum Failure {
       Config(ValidationError),          // C-cfg → exit 1
       Fatal(String),                    // FatalError → exit 1
       Unexpected(PyExc),                // K/K+ → exit 3
       Interrupted,                      // → 130
       StdoutClosed,                     // EH-03 → 120
   }
   struct PyExc { ty: &'static str, text: Vec<u8>, hint: Option<String> }
   ```
   - Constructors: `PyExc::os(&io::Error, path: &Path)`, `PyExc::os2(err, src, dst)`, `PyExc::called_process(cmd, rc, out, err)` (format A), `PyExc::check_output(cmd, rc)` (format B), `PyExc::utf8_decode(buf, Utf8Error)` and `PyExc::sqlite(rusqlite::Error)`. Each one emits the X.2 texts.
   - Every `?` across a subsystem boundary maps to one of these. No `anyhow` text reaches line 1.
2. **Errno to class** is a `match` on `io::ErrorKind` plus `raw_os_error()`. Use the raw errno first and `ErrorKind` only as a fallback (Windows). strerror is `io::Error::from_raw_os_error(n).to_string()` minus ` (os error n)`. Unit-test it against Python for errnos 1, 2, 7, 13, 17, 20, 21, 28, 30, 32, 36 and 39 on Linux and macOS CI.
3. **Python repr helper.** One `py_repr_str(&OsStr)` produces the `'…'`/`"…"` repr with `\udcXX` for non-UTF-8 bytes. The tuple repr `('a', 'b')` (and the 1-tuple `('a',)`) is used in both `CalledProcessError` formats.
4. **The handler is infallible:**
   - compute the log path **without** creating the store;
   - write every stdout line with `let _ =` on a shared `Printer` that latches `StdoutClosed` on EPIPE;
   - never call `Store::open` from the handler;
   - wrap `main` in `catch_unwind` (`panic = "unwind"`), and map a panic to `PyExc { ty: "PanicException", … }`.
5. **Ordering contract** (tested by the difftest):
   - a partial `<name>…` segment is followed directly by line 1 on K rows (EXE-08), but is completed with `Failed` on C-hook rows;
   - the stash restore and its `[INFO] Restored changes…` line come before line 1;
   - all of this goes to stdout.
6. **Store robustness:**
   - lazy creation (ST-01);
   - `CREATE TABLE IF NOT EXISTS` on every open (ST-07);
   - best-effort config tracking (ST-05/06/08/09);
   - verify cached paths (ST-10);
   - ENOENT-tolerant gc that commits its deletions (CMD-07);
   - sqlite busy timeout 5000 ms when sharing pre-commit's `db.db`.
7. **Classification:**
   - `lstat` once;
   - treat `NotFound` (and every lstat error, as identify does) as vanished → excluded (CLS-01/04);
   - open non-regular files with `O_NONBLOCK` (`std::os::unix::fs::OpenOptionsExt::custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)`, safe constants);
   - read at most 1024 bytes (CLS-03);
   - a `PermissionDenied` on a regular file is a K abort (CLS-02).
8. **Regex validation:**
   - the Python-syntax front end returns `Err(Rejected)` for `re.error` **and** for the CFG-05/06/07 cases;
   - the parser must be iterative or depth-capped *before* recursion (a Rust stack overflow aborts the process and cannot be caught);
   - snp sets `nest_limit(1000)` on the `regex` crate;
   - it never hands fancy-regex a pattern deeper than 63.
9. **hook-impl:** keep the order (HI-01). Parse pre-push lines per R3. Use format B for the three `rev-list`/`rev-parse` calls, with git's stderr inherited.
10. **Difftest scenarios.** Add `CRASH-*` scenarios, one per row with a ✔: compare line 1 and the exit code (Tier A); the path suffixes (`repoXXXXXXXX`, `<tmp>`, `patch<ts>-<pid>`) are normalised. For F/C rows, assert snp's new behaviour and record the oracle output as an expected deviation. Fuzz `pre-push stdin`, `entry` strings, regexes and store-file contents for "no exit 101, no hang > 5 s".
11. **Never mutate global env** to emulate `PYTHONWARNINGS`. snp simply has no such channel (SM-10).

### X.9 Open questions (this addendum)

1. **K+ hint lines** (STASH-02). Is one extra `[ERROR]` line after the log line acceptable for Tier A comparisons, or should it go to stderr?
2. **EH-03 exit code.** Is 120 (parity with CPython's flush failure) the right choice, or should snp use 141 (the 128+SIGPIPE convention)? Both are fail-closed.
3. **Python-valid patterns that are too big for snp's engines** (`a{1000000}`, nesting 64–480 with fancy features). Options:
   - reject with the standard `'…' is not a valid python regex` leaf (a small lie);
   - reject with a new leaf such as `'…' exceeds snp's regex limits`;
   - add a third, slow but complete backtracking engine.

   Recommendation: the new leaf, exit 1.
4. **CLS-02** (an unreadable staged file aborts the run, exit 3). Keep it fail-closed (recommended), or turn it into a per-hook failure?
5. **EXE-02 text.** Is `ValueError: No closing quotation` (Python-style) right, or a more explanatory `entry could not be parsed: No closing quotation`?
6. **PanicException** as the type token for Rust panics (EH-05), or a snp-branded token (`SnpBug`)?
7. **CFG-14 knock-on.** After conversion, `gc` untracks configs that pre-commit would have crashed on, and their cached repos become collectable. Is that acceptable, or should `gc` keep configs whose failure is a snp-only conversion?

### X.10 Reproduction recipes (oracle 4.6.2; template for the `CRASH-*` difftest scenarios)

**Common setup** (`scripts-g4/env.sh`):

- a fresh `git init` repo per case;
- `HOME`, `GIT_CONFIG_NOSYSTEM=1` and the author/committer env set;
- `PRE_COMMIT_HOME=<case>.pch`;
- `PRE_COMMIT_COLOR` unset;
- stdout and stderr captured separately.

`L` below is a local hook with `id: h`, `name: h`, `language: system`.

| Case | Setup | Command | Rows |
|---|---|---|---|
| c01 | `L` + `files: "a{4294967296}"` | `validate-config .pre-commit-config.yaml`; `run -a` | CFG-05 |
| c02 | `L` + `files: "[[]"`, `exclude: "[a&&b]"`; a second hook with `"[[]"` / `"[a\|\|b]"`; depth sweep `'('*n+')'*n` for n = 300…500 | `validate-config`; `run -a`; `PYTHONWARNINGS=error\|ignore validate-config` | CFG-06, CFG-08, SM-03, SM-09 |
| c03 | `L` + `files: "(?a)(?u)x"` | `validate-config` | CFG-07 |
| c04 | commit `ok.txt` and `$'bad\xff.txt'` | `run -a`; modify + stage, then `run`; `run --files $'bad\xff.txt'` | GIT-05, GIT-06 |
| c05 | hook 1: `entry: rm -f b.txt`, `files: ^a\.txt$`, `pass_filenames: false`, `always_run: true`; hook 2: `entry: echo` | `run -a`; staged `run` | CLS-01 |
| c06 | `language: python`, `entry: python -c "print('hi')"`, `pass_filenames: false` (install once, back up the store) | delete `.install_state_v2`, write `{bad` / `` / `\xff` / chmod 000 into `.install_state_v1`, then `run -a`; hold `BEGIN EXCLUSIVE` on `db.db` from another process, then `run -a`; `rm -rf <store>/repo*`, then `run -a` | ENV-01, ENV-02, ST-05, ST-10 |
| c08 | hook `entry: sh -c 'echo some-output; exit 1' --`, `log_file: nodir/x.log` (then a dir `logd`, then `""`); staged + unstaged changes | `run`; `run -a` | EXE-05 |
| c09 | `entry:` `""`, `"   "`, `"''"`, `"#"`, `""` + `args: [echo, hi]`, `"echo 'unterminated"` | `run -a` | EXE-01, EXE-02, SM-07 |
| c10 | `stages: [pre-push]`, one commit, `git remote add origin /nonexistent/remote` | `hook-impl --config=.pre-commit-config.yaml --hook-type=pre-push --hook-dir "$PWD/.git/hooks" -- origin /nonexistent/remote` with stdin lines: bad local sha + Z40; HEAD + unknown remote sha; 2 fields; `\xff`; `zzzz`; a deletion line + a blank line. Also `hook-impl --hook-type=bogus --`, and pre-push with 1 arg. | HI-01…HI-05, SM-05 |
| c11 | `sub/s.txt`; hooks with `entry: pre-commit hazmat[ …]` | `hazmat`; `hazmat n1 nonexistent-exe --`; `hazmat ignore-exit-code`; `hazmat cd nodir echo --`; `hazmat n1 echo`; the same as entries, then `run -a` | HZ-01…HZ-06 |
| c12 | copy of the c06 store plus a row `('https://example.invalid/x','v1','<store>/repoGONE')` | `gc` twice | CMD-07 |
| c13 | copy of the c06 store with paths rewritten, `chmod 555 <store>`; a config with `additional_dependencies: [six]`; separately `PRE_COMMIT_HOME=<ro-parent>/pch` | `run -a` (cached); `run -a -c c13.yaml`; `run -a` | ST-01, ST-03, SM-01 |
| c14 | `PRE_COMMIT_HOME=<regular file>`; `db.db` = garbage / empty / a directory | `run -a`; `gc` | ST-02, ST-06…ST-08 |
| c15 | `repo: /nonexistent/repo`, `rev: v1.0.0`; chmod 000 on the config | `run -a`; `autoupdate`; `try-repo /nonexistent/repo`; `run -a` and `validate-config` | GIT-01…GIT-03, CFG-09 |
| c16 | meta `check-useless-excludes`, `check-hooks-apply`, `identity` (all `verbose: true`); `L` + `files: "[[]"` + `bogus_key: 1`; root `foo: bar` | `run -a` | SM-03, SM-04 |
| c17 | a local-path hook repo (python hook, tag `v1.0.0`) used with `additional_dependencies: [six, 'attrs==23.1.0']`, plus a local python hook with `[six, attrs]` | `install-hooks` | SM-01, SM-02 |
| c18 / c23 | a `verbose` hook printing 2 MB; c23 adds staged + unstaged changes | `run[ -a] \| head -c 20` | EH-03, STASH-03 |
| c19 | `entry: echo` with `args: ['x'*140000]` (with and without `pass_filenames: false`), and 30×100 KB args | `run -a` | EXE-03, SM-06 |
| c20 | `noext` (chmod 000); `u.txt` (chmod 000); `s.sh` (chmod 111) as `entry: ./s.sh`; `mkfifo ff` | `run -a`; `timeout 5 run --files ff` | CLS-02, CLS-03, EXE-04 |
| c21 | `mkdir <store>/pre-commit.log` + a config that crashes (CFG-05); `.git/hooks/pre-commit.legacy` = `#!/nonexistent/interp` (then a directory) | `run -a`; `hook-impl --config=… --hook-type=pre-commit --hook-dir "$PWD/.git/hooks" --` | EH-02, HI-07 |
| c22 | `chmod 555 <store>` + staged and unstaged changes; `run --from-ref bogus --to-ref HEAD`; `chmod 444 <store>/db.db` | `run`; `run --from-ref …`; `run -a` | ST-04, GIT-04, ST-09 |
| c24 | no config; `repos: []\n# caf\xe9`; `%YAML 1.1\n---\n-   repo: local\n    hooks: []`; `minimum_pre_commit_version: "abc"` | `migrate-config` ×3; `validate-config` | CFG-10…CFG-12, CFG-01 |
