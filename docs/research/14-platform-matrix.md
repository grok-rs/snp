# 14 — Platform matrix (Linux, macOS, Windows)

This file collects platform-specific behaviour for snp. Sections are appended by gap-fill tasks; each addendum states which earlier sections it supersedes.

## Addendum: Windows and macOS behaviour that is only derived, never verified: list it and plan the checks

Gap-fill #6, written 2026-10-08. Audience: engineers implementing snp (Rust 1.98.0, edition 2024) who will not re-read the Python, Go, C or Rust std sources.

**Scope.** Every earlier observation was made on Linux. This addendum lists each Windows and macOS claim made in 01–10 and in the gap brief. For each claim it records the strongest evidence that could be obtained without Windows or macOS hardware, the decision for snp, and a CI test for anything that stays unverified.

**What it replaces.** The matrix in §A.2 **supersedes 01 §14, 02 §18, 03 §18 and 04a §9**. Those sections stay for context. Where they disagree with this addendum, the addendum wins.

---

### A.0 Evidence legend, method, reproducibility

| Tag | Meaning | How strong |
|---|---|---|
| **VL** | Observed on Linux with the pre-commit **4.6.2** oracle (`scratchpad/tmp-test-strategy/venv462/bin/pre-commit`), either directly or as an analog. For example, a symlinked cwd stands in for macOS `/tmp` → `/private/tmp`, and a missing shebang interpreter stands in for Windows `#!/bin/sh`. | Strong for the logic. The OS-specific part is reached by analogy. |
| **VN** | CPython's pure-Python `ntpath` and `shlex` executed on Linux (`python3 -I`, CPython 3.12.3). These are the modules Windows runs; only the `nt`-accelerated helpers (`_getfullpathname`, `_path_normpath`) differ, and they are semantically equal. | Strong |
| **VW** | Observed under **Wine 9.0** with probes cross-compiled by `rustc +1.98.0 --target x86_64-pc-windows-gnu`, plus mingw-w64 C probes. | Authoritative for logic inside Rust std that runs before any OS call: quoting, `InvalidInput`, `ExitStatus` conversion, lock ranges. Indicative only for OS or CRT behaviour, because Wine differs from Windows in error-message texts, the `_LK_LOCK` retry timing and cmd.exe's handling of `%`. |
| **VS** | Read in primary source code (versions below). | Strong for code paths. Runtime configuration can still change the outcome. |
| **VD** | Vendor documentation: docs.python.org, learn.microsoft.com, git docs. | Strong |
| **DER** | Derived by combining the sources above. Not executed. | Medium |
| **UNK** | Cannot be determined without the real OS or terminal. A CI or manual test is specified in §A.6. | — |

Source pins and citation prefixes:

| Prefix | Source |
|---|---|
| `pc:` | `refs/pre-commit` (4.6.2) |
| `id:` | `refs/identify` (2.6.20) |
| `cpy:` | CPython `3.12` branch |
| `git:` | git/git `master` @ 6de20f60 |
| `gfw:` | git-for-windows/git `main` @ cc4dbf75 |
| `msys2:` | msys2/MSYS2-packages `master` @ e555935d |
| `rs:` | rust-lang/rust tag `1.98.0`, `library/std/src/`. `sys/args/windows.rs`, `sys/io/is_terminal/windows.rs` and `sys/thread/windows.rs` are byte-identical to 1.99. The other cited files differ only in unrelated lines. |
| `crate:` | Crate sources from `~/.cargo/registry` |

Downloaded copies are in `scratchpad/dl-platform/{cpython312,git,rust-1.98.0}`. The probes are in `scratchpad/tmp-platform/src/`:

| Probe | What it does |
|---|---|
| `probe.rs` | exit codes, `.bat`/`.cmd` quoting, `File::lock`, spawn errors, `errfmt`, cpu count, `home_dir` |
| `pylock.c` | emulates `open(p,'a+')` + `msvcrt.locking(fd, mode, 0xffff)` |
| `isatty.c` | prints CRT `_isatty(0)` and `GetFileType` |
| `stdinprobe.rs` | runs a child with `Stdio::null()` and with `Stdio::piped()` + drop |

Reproduce (Linux host with wine and mingw-w64):

```sh
cd scratchpad/tmp-platform
rustc +1.98.0 --edition 2024 -O --target x86_64-pc-windows-gnu -C linker=x86_64-w64-mingw32-gcc src/probe.rs -o probe.exe
x86_64-w64-mingw32-gcc -O2 -o pylock.exe src/pylock.c; x86_64-w64-mingw32-gcc -O2 -o isatty.exe src/isatty.c
export WINEPREFIX=$PWD/wineprefix WINEDEBUG=-all WINEDLLOVERRIDES="mscoree=;mshtml=" DISPLAY=
wine probe.exe exitcodes; wine probe.exe bat "Z:$PWD/batdir"; wine stdinprobe.exe isatty.exe
```

---

### A.1 Headline results

Each item ends with its evidence tag.

1. **Windows hook scripts written by pre-commit are CRLF.** They work only because Git for Windows' `sh` is MSYS2 bash, which drops every `\r` while reading a script.
   - `open(hook_path, 'w')` (`pc:commands/install_uninstall.py:92`) translates `\n` to `os.linesep` = `\r\n` (VD: docs.python.org `open()` *newline*).
   - Git for Windows reads the first line up to `\r` or `\n`, so `#!/bin/sh\r` resolves to `sh` (`gfw:compat/mingw.c:1873-1911`, VS).
   - MSYS2 bash `shell_getc` has `#ifdef __MSYS__ if (c == '\r') continue;` (`msys2:bash/0005-bash-4.3-msys2-fix-lineendings.patch`, applied at `msys2:bash/PKGBUILD:53`, VS).
   - The same file under Linux git (a WSL clone on `/mnt/c`) fails with `fatal: cannot exec '.git/hooks/pre-commit': No such file or directory` and the commit is aborted (VL, reproduced with the exact Windows byte layout).
   - **snp writes LF on every OS**, as 03 §20.3 already proposed. That is now justified by evidence.
2. **Every existing file is `executable` on Windows.** CPython `os.access` on Windows is `GetFileAttributesW(path) != INVALID_FILE_ATTRIBUTES`, plus a read-only check for `W_OK` only (`cpy:Modules/posixmodule.c:3162-3178`, VS). Consequences:
   - `types: [executable]` matches every regular file, and `types: [non-executable]` matches none.
   - Extension-less files always get shebang tags (`id:identify/identify.py:56-71`).
   - `Executable … is not executable` never fires.
   - A `.legacy` hook always runs.
   - A *dangling* symlink also counts as executable, because `GetFileAttributesW` does not follow links.
3. **`#!/bin/sh` and `#!/bin/bash` scripts fail under pre-commit on Windows; Git itself runs them.**
   - pre-commit's `normexe('/bin/bash')` takes the path branch: `isfile('\bin\bash')` on the current drive is false, so it reports ``Executable `/bin/bash` not found``.
   - The same path in a **legacy** hook raises out of `hook_impl._run_legacy`, which gives `An unexpected error has occurred: ExecutableNotFoundError: …` and **exit 3** (VL analog with `#!/bin/doesnotexist`; DER for Windows).
   - Git for Windows instead looks up the interpreter's **basename** on `PATH` (`gfw:compat/mingw.c:1898-1910, 2437-2468`).
   - snp adds the Git-for-Windows basename fallback as a documented deviation (D-WIN-02).
4. **Windows exit codes are unsigned 32-bit in pre-commit and signed in Rust.** `_winapi.GetExitCodeProcess` returns `Py_BuildValue("k", DWORD)` (`cpy:Modules/clinic/_winapi.c.h:577`, VS), so pre-commit prints `- exit code: 3221225477`. Rust gives `ExitStatus::code() == Some(-1073741819)` and `Display` gives `exit code: 0xc0000005` (`rs:sys/process/windows.rs:762-788`; VW for 0, 1, 3, 130, 255, 256, 65535, -1, 0xC0000005 and 0xC000013A). snp formats `code as u32`.
5. **`[WinError N] msg` text.** CPython strips trailing whitespace **and dots** from the `FormatMessageW` text (`cpy:Python/errors.c:884-886, 994-996`). It formats `[WinError %S] %S`, with `: %R` added only when a filename is attached (`cpy:Objects/exceptions.c:1993-2013`). Rust trims whitespace only and appends ` (os error N)` (`rs:sys/io/error/windows.rs:96-148`). The text is **localized** on both sides (Tier B). Error 193 was VW-confirmed as the spawn error for a non-PE file.
6. **Rust's batch-file handling differs from pre-commit, and is safer.**
   - Rust spawns `cmd.exe /e:ON /v:OFF /d /c ""<script>" args"`, quotes every argument that contains an ASCII character outside `[A-Za-z0-9#$*+-./:?@\_]`, rewrites `%` as `%%cd:~,%`, and returns `InvalidInput` **only** for arguments that contain `\r` or `\n`, with the message `batch file arguments are invalid` (`rs:sys/args/windows.rs:219-333`; VW for 26 cases).
   - pre-commit lets `CreateProcess` launch the batch file implicitly, with `list2cmdline` quoting that only handles spaces and tabs. On Windows, a filename containing `&`, `^`, `|`, `<`, `>` or `%VAR%` is therefore interpreted by cmd.exe under pre-commit (VD: the subprocess docs "Security Considerations"; DER for the exact mangling).
7. **Store-lock interop is verified in both directions.** Rust `File::lock`/`try_lock` takes `LockFileEx` over `[0, 2^64-1)`. pre-commit's `msvcrt.locking(fd, LK_NBLCK, 0xffff)` locks from the current position of a file opened `'a+'`, which CPython seeks to EOF at open (`cpy:Modules/_io/fileio.c:484`).
   - VW results: CRT held → Rust `try_lock` = `WouldBlock`. Rust held → CRT `_locking` = -1. CRT held at a non-zero offset (file has content) → Rust `WouldBlock`. Rust blocking `lock()` acquired the lock about 3.8 s later, right after the CRT released it.
   - Wine does **not** reproduce the documented `_LK_LOCK` retry of 10 × 1 s (it returned within 10 ms). Windows timing stays UNK; it does not affect correctness.
8. **A hook's stdin must be an empty pipe, not `NUL`.** Rust `Stdio::null()` opens `\\.\NUL` (`rs:sys/process/windows.rs:643`). `NUL` is a character device, so the CRT's `_isatty(0)` returns **1** (VW; VD: learn.microsoft.com `_isatty`, "character device (a terminal, console, printer, or serial port)"). CPython's `isatty()` *is* the CRT `isatty` (`cpy:Modules/_io/fileio.c:1146`), so Python hooks would see `sys.stdin.isatty() == True`. A dropped pipe gives `_isatty(0) = 0` (VW). This settles the stdin contradiction (§A.4.1).
9. **Color on Windows is about stderr's console mode, not "is a terminal".**
   - pre-commit's `auto` requires CRT `isatty(stderr)` (true for console **and** `NUL`) **and** a successful `GetConsoleMode` + `SetConsoleMode(…|ENABLE_VIRTUAL_TERMINAL_PROCESSING)` on `STD_ERROR_HANDLE`, done once at import (`pc:color.py:7-57, 93-100`, VS).
   - Rust `IsTerminal` is `GetConsoleMode` **or** an MSYS/Cygwin pty pipe name (`rs:sys/io/is_terminal/windows.rs:5-70`, VS). It is therefore **true in mintty**, where pre-commit says false.
   - `anstyle-query 1.1.5` enables VT on **stdout first** and fails when stdout is redirected (`crate:anstyle-query-1.1.5/src/windows.rs:36-48`).
   - Neither matches pre-commit. snp needs a tiny audited `snp-sys` crate (§A.7.1).
10. **On Windows pre-commit prints paths with backslashes.** Examples: `pre-commit installed at .git\hooks\pre-commit`, `Running in migration mode with existing hooks at .git\hooks\pre-commit.legacy`, `--config=sub\cfg.yaml` (shell-quoted in the hook script), `Stashing unstaged files to C:\…\patch…`, `Check the log at C:\…\pre-commit.log`.
    - The cause is `os.path.normpath(os.path.join('.', <git output>))` (`pc:git.py:86-93`) and `ntpath.join` (VN).
    - Rust `Path::join` does **not** normalize the `/` that git prints. snp needs an `ntpath.normpath` port for displayed paths.
11. **A `relpath` across drives crashes pre-commit.** `ntpath.relpath` raises `ValueError("path is on mount 'D:', start on mount 'C:'")`. `_adjust_args_and_chdir` does not catch it (`pc:main.py:191-199`), so the result is `An unexpected error has occurred: ValueError: path is on mount 'D:', start on mount 'C:'` and exit 3. The message is identical in 3.12–3.14 (VN for D:/C: and UNC/C:). snp keeps the absolute path instead (D-WIN-06).
12. **macOS `/tmp` → `/private/tmp`.** `os.getcwd()` (and Rust `current_dir`) is physical, so `run --files /tmp/repo/a.py` gives hooks the argument `../../../tmp/repo/a.py`. A `PRE_COMMIT_HOME` reached through a symlink is printed with its **realpath** (`Check the log at …/private/…`) (VL with an equivalent symlink layout). snp must reproduce this lexically: never canonicalize file arguments, but realpath the store.
13. **The cygwin-mismatch warning is dead code on native Windows Python.** `get_root()` is `abspath(--show-cdup)`, and on win32 `ntpath.abspath` always returns a drive-absolute or UNC path, so `toplevel.startswith('/')` is never true (`pc:git.py:51-73, 210-230`; VS + DER). snp prints nothing (parity) and skips the two extra `git rev-parse` spawns.
14. **CPU count.**
    - Windows: CPython uses `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)` (`cpy:Modules/posixmodule.c:14448-14451`). Rust `available_parallelism` and `num_cpus 1.17` use `GetSystemInfo().dwNumberOfProcessors` (current processor group only, at most 64) (`rs:sys/thread/windows.rs:79-89`). They differ only on hosts with more than 64 logical CPUs.
    - macOS: both use `sysconf(_SC_NPROCESSORS_ONLN)` (VS).
    - Python 3.13+ also honours `PYTHON_CPU_COUNT` on macOS and Windows. On Linux pre-commit uses `sched_getaffinity`, which ignores it (VD).
15. **Git for Windows changes the hook environment when `MSYSTEM` is unset** (cmd, PowerShell, IDEs). It prepends `%HOME%\bin;<git>\mingw64\bin;<git>\usr\bin;` to `PATH`. It also sets `TERM=xterm-256color`, `COLORTERM=truecolor` (Windows ≥ 1703), `HOME`, `TMPDIR` and `LC_CTYPE=C.UTF-8` when they are absent (`gfw:compat/mingw.c:3748-3840`; `ENSURE_MSYSTEM_IS_SET` is set by `gfw:config.mak.uname:540,792`, VS).
    - As a result, `#!/usr/bin/env bash` resolves to Git's bash inside `git commit`, but may resolve to `C:\Windows\System32\bash.exe` (WSL) or to nothing for a direct `snp run` from cmd.
    - Parity is natural, because snp inherits the same environment.
16. **The Windows text-mode stdout prints CRLF for every `print()`/argparse output when redirected** (`cpy:Python/pylifecycle.c:2454-2463`). The affected outputs:
    - `sample-config`, `validate-config`/`validate-manifest` errors, the hook-impl missing-config messages, the `migrate-config` result line, `--version`, usage and help, and argparse errors.
    - The output of meta hooks, which run as Python children and `print()`.

    Everything written through `output.write*` (`sys.stdout.buffer`) stays LF (`pc:output.py:9-33`). snp emits LF everywhere (D-WIN-04). The harness ignores EOL style for those streams on Windows.
17. **Env-var names.**
    - `os.environ` upper-cases keys on Windows (`cpy:Lib/os.py:774-787`). Two consequences:
      - Children that receive a **copy** (`env=dict(os.environ, …)`, `no_git_env()`) see upper-cased names.
      - `no_git_env`'s `k.startswith('GIT_')` also strips a lower-case `git_dir`.
    - Rust preserves the original case. snp must compare env names ASCII-case-insensitively on Windows (DER; test PLAT-W17).

---

### A.2 Platform matrix (authoritative)

The columns follow the same order in every table:

- **L / M / W:** the behaviour on Linux, macOS and Windows.
- **Ev.:** the evidence tag (§A.0).
- **Src:** where it comes from.
- **snp:** the decision for snp.
- **Test:** the test ID in §A.6.

#### A.2.1 Hook installation and how git runs hooks

| ID | Topic | L | M | W | Ev. | Src | snp | Test |
|---|---|---|---|---|---|---|---|---|
| P-01 | Hook script first line (pre-commit) | `#!/usr/bin/env bash` | same | `#!/bin/sh\r\n#!/usr/bin/env bash\r\n…` | VS+VD | `pc:install_uninstall.py:92-108` | `#!/bin/sh`, POSIX sh (no arrays), **LF**, all OSes (03 §20.3) | W01, W02 |
| P-02 | Hook file newlines | LF | LF | **CRLF** (text mode) | VD (newline=None → `os.linesep`) | `pc:install_uninstall.py:92` | LF (D-WIN-01) | W01 |
| P-03 | Who interprets the shebang | kernel `execve` | kernel | Git: reads up to `MAX_PATH-1` bytes, requires `#!`, cuts at the first `\r`/`\n`, takes the basename after the last `/` or `\`, drops everything after the first space, then looks up `<name>.exe` on `PATH` (exe_only). `.exe` hooks are spawned directly. | VS | `gfw:compat/mingw.c:1873-1933,1998-2025,2437-2468` | Template must work when the interpreter is `sh` found on `PATH`. Use no options on the shebang line. | W02 |
| P-04 | Hook-executable check by git | `access(X_OK)`; non-executable → `hint: The '…' hook was ignored because it's not set as executable` | same | `X_OK` is masked (`mingw_access` does `mode & ~X_OK`): any existing file runs. Also tries `<hook>.exe` (`STRIP_EXTENSION`). | VS | `git:hook.c:36-60`; `gfw:compat/mingw.c:1171-1184`; `gfw:config.mak.uname:541,738` | `make_executable` is a no-op on Windows; never rely on modes there | W02 |
| P-05 | CR handling by the shell running the hook | bash/dash keep `\r` (a CRLF script breaks) | same | MSYS2 bash drops every `\r` from script input; `set_if_not` also trims a trailing `\r` | VS (MSYS2); DER that Git for Windows ships MSYS2's patched bash | `msys2:bash/0005-…-fix-lineendings.patch`; `PKGBUILD:30,53` | LF removes the dependency on this patch | W01 |
| P-06 | `sh` availability for hooks | system | system | Needs `sh.exe` on `PATH`. Git prepends `usr\bin` when `MSYSTEM` is unset. GUI clients with MinGit may lack it, in which case git prints `cannot spawn …` before snp is ever reached. | VS + DER | `gfw:compat/mingw.c:3815-3835` | Nothing to do in snp. Document it. | W02, manual |
| P-07 | BusyBox `sh` (MinGit-BusyBox) | – | – | Git maps `sh` to `busybox.exe` (`is_busybox_applet`). ash has **no bash arrays**, so pre-commit's `ARGS=(…)` template fails there. | VS + DER | `gfw:compat/mingw.c:1937-1996`, `pc:resources/hook-tmpl` | The POSIX template works (an improvement) | manual M-B1 |
| P-08 | Exec bit after install | `mode \| 0o111` | same | `os.chmod` changes only the read-only flag (no-op) | VD (os.chmod note) | `pc:util.py:43-46` | Skip chmod on Windows | – |
| P-09 | `INSTALL_PYTHON` quoting | `shlex.quote` POSIX | same | `'C:\Users\…\python.exe'` (single quotes, backslashes literal; `[ -x 'C:\…' ]` works in MSYS2) | VN (`shlex.quote`) | `pc:install_uninstall.py:105` | `SNP_EXE='C:/…/snp.exe'` (forward slashes) | W01 |
| P-10 | Printed hook path | `.git/hooks/pre-commit` (relative); absolute for worktrees and submodules | same, but absolute paths are **physical** (`/private/…`) | `.git\hooks\pre-commit`; worktree `C:\abs\main\.git\hooks\pre-commit` (normpath of git's `C:/…`) | VN | `pc:git.py:86-93`, `install_uninstall.py:47-53` | `nt_normpath` for display (§A.7.3) | W14 |
| P-11 | `--config=` in the script when the config is in a subdir | `--config=sub/cfg.yaml` | same | `'--config=sub\cfg.yaml'` (relpath uses `\`; `shlex.join` quotes it) | VN | `pc:main.py:191`, `install_uninstall.py:103-107` | Reproduce `\`; quote with the `shlex.quote` port | W14 |
| P-12 | MSYS2 argv conversion when sh `exec`s a native exe | – | – | `--hook-dir "/c/repo/.git/hooks"` arrives as `C:/repo/.git/hooks`. A relative `--config=` is untouched. | DER (MSYS2 runtime behaviour) | – | Accept `/` and `\` in `--hook-dir` | W02 |
| P-13 | `.legacy` overwrite on install | `os.rename` overwrites | same | `shutil.move` → `os.rename` raises `FileExistsError` → falls back to copy2 + unlink; same visible result | VS | `cpy:Lib/shutil.py:805-`; `pc:install_uninstall.py:76-77` | `fs::rename` (MoveFileExW with REPLACE_EXISTING); `.legacy` clobber policy per 03 §20.3 | – |
| P-14 | WSL using a Windows-installed hook | – | – | A CRLF hook gives `fatal: cannot exec '.git/hooks/pre-commit': No such file or directory` under Linux git | VL | – | LF script plus `command -v snp` fallback makes one hook file work from both sides | manual M-WSL |

#### A.2.2 Executable resolution, shebangs, spawning

| ID | Topic | L | M | W | Ev. | Src | snp | Test |
|---|---|---|---|---|---|---|---|---|
| P-20 | `find_executable(exe)` | `normpath(exe)`; if it contains `os.sep`, return it as-is; else for each `PATH.split(':')` entry try `exe` (`isfile && access(X_OK)`) | same | Dir-major: for each `PATH.split(';')` entry try `exe+ext` for each ext in `PATHEXT.split(';')`, **then** bare `exe`; `X_OK` is always true. Extension case comes from `PATHEXT` (`prettier.CMD`). Empty entry = relative name. **Quotes in PATH are not stripped.** Uses the child env's `PATH`/`PATHEXT` when `env` is given. | VS | `pc:parse_shebang.py:22-43` | Own port (§A.4.2); **not** `which` | W06 |
| P-21 | `normexe` error texts | `` Executable `x` not found `` / `` … is a directory `` / `` … is not executable `` | same | the third never occurs | VS | `pc:parse_shebang.py:46-62` | Same strings; on Windows the X_OK test is `true` | W03 |
| P-22 | Absolute POSIX interpreter in a shebang (`#!/bin/bash`) | kernel-equivalent lookup of the path | same | pre-commit: `normexe('/bin/bash')` → `isfile('\bin\bash')` on the current drive → ``Executable `/bin/bash` not found`` (hook rc 1). Git: basename lookup works. | VL (analog `#!/bin/doesnotexist`: `Failed`, `- exit code: 1`, ``Executable `/bin/doesnotexist` not found``) + VS | `pc:parse_shebang.py:50-58`; `gfw:mingw.c:1898-1910` | **D-WIN-02:** on Windows only, when a *shebang* interpreter starts with `/` and is not a file, retry `find_executable(basename)`. If that also fails, keep pre-commit's message. | W04 |
| P-23 | Legacy hook whose interpreter is missing | exit 3 crash | same | the same crash for **every** `#!/bin/sh` legacy hook | VL: `An unexpected error has occurred: ExecutableNotFoundError: Executable `/bin/doesnotexist` not found` + `Check the log at …`, rc 3 | `pc:hook_impl.py:40-46` | D-WIN-02 applies; never crash | W05 |
| P-24 | Script without a shebang (`entry: ./x.sh`, not PE) | `[Errno 8] Exec format error: './x.sh'` | same | `[WinError 193] %1 is not a valid Win32 application` (no filename; `%1` is literal because of `FORMAT_MESSAGE_IGNORE_INSERTS`) | VS + VW (code 193; Wine's text differs) | `pc:util.py:86-87,102-105`; `cpy:errors.c:977-998` | Format as in §A.7.4 | W09 |
| P-25 | Batch targets (`.bat`/`.cmd`, case-insensitive) | – | – | pre-commit: `CreateProcess(NULL, list2cmdline(argv))`; Windows launches `cmd.exe /c` implicitly (with **AutoRun**, delayed expansion per registry); `list2cmdline` quotes only args with space, tab or empty, and escapes `"` as `\"`. Rust: `<GetSystemDirectory>\cmd.exe` with `cmd.exe /e:ON /v:OFF /d /c ""<script>" <args>"`; quotes per P-26. | VS (both) + VD; DER for the implicit cmd.exe launch (documented by the BatBadBut advisories) | `cpy:Lib/subprocess.py:576-640,1455-1462`; `rs:sys/process/windows.rs:301-321,884-889`; `rs:sys/args/windows.rs:292-333` | **D-WIN-03**: rely on Rust's safe quoting; document the semantic difference | W07 |
| P-26 | Rust batch-argument quoting | – | – | Quote when the arg is empty, ends with `\`, or contains an ASCII char outside `[A-Za-z0-9#$*+-./:?@\_]` or a control char. Inside: `"` → `""`; backslashes doubled only before `"`; `%` → `%%cd:~,%` (+7 UTF-16 units each). Arg contains `\r` or `\n` → `InvalidInput "batch file arguments are invalid"`. Script path containing `"` or ending with `\` → `InvalidInput`. NUL → `InvalidInput` (`ensure_no_nuls`). | VS + **VW** (`plain`→`plain`, `a b`→`"a b"`, `a"b`→`"a""b"`, `a&b`→`"a&b"`, `a\b\`→`"a\b\\"`, `a\nb`/`a\rb`→Err; `%` expansion looked wrong under Wine because Wine's cmd lacks the `%cd:~,%` trick) | `rs:sys/args/windows.rs:219-288` | Batch budget check (§A.7.6) | W07, W08 |
| P-27 | Rust bare-name resolution | – | – | `Command::new("t")` appends **only `.exe`** and searches child PATH (only if `env` changed it), then the **exe's own dir**, then System32, Windows, then parent PATH. `t.bat`/`T.BAT` resolve as given. (VW: `"t"` → `NotFound "program not found"`.) | VS + VW | `rs:sys/process/windows.rs:468-590` | Always pass the **absolute path** from the `find_executable` port to `Command::new` | W06 |
| P-28 | Command-line budget | `clamp(ARG_MAX-2048-envsize, 4096, 131072)` bytes | same (ARG_MAX 1 MiB on current macOS; 256 KiB on older releases, still above the clamp) | `2**15-2048 = 30720` UTF-16 units; batch: `8192 - len(find_executable('cmd.exe')) - len(' /c ') - 1024`; hard OS limit 32767 incl. NUL (VD); overflow → `[WinError 206] The filename or extension is too long` | VS + VD | `pc:xargs.py:49-69,154-164` | Parity formula, then verify the real encoded length (§A.7.6) | W08 |
| P-29 | Hook stdin | empty pipe (closed by `communicate()`) | same | same (pipe) | VS + **VW** for the NUL hazard | `pc:util.py:81-84,107` | `Stdio::piped()` + drop on **all** OSes; `/dev/null` only for the POSIX pty path (`pc:util.py:176-178`) | W16 |
| P-30 | Hook output capture with `--color` | pty, `ONLCR|OPOST` cleared, stdin `/dev/null`, EIO = EOF | pty; EOF semantics UNK | pipes (`cmd_output_p = cmd_output_b`) | VS | `pc:util.py:123-202` | Pipes on Windows; macOS pty tested | M05 |
| P-31 | `#!/usr/bin/env bash` inside `git commit` vs a direct `run` | – | – | Inside hooks `PATH` has `<git>\usr\bin` first, so Git's bash is used. From cmd, `bash` may be `System32\bash.exe` (WSL) or missing. | VS (gfw PATH) + DER | `gfw:compat/mingw.c:3704-3731,3815-3835` | Parity (same env). Document it. | W02 |
| P-32 | Shebang reading | `id:parse_shebang`: reads 2 bytes, then one line, UTF-8, printable-only, `strip()` (so a CRLF shebang is fine), `/usr/bin/env [-S]` stripped | same | same, and done for **every** extension-less file (P-40) | VS | `id:identify.py:178-219` | Port exactly (08 F4) | W03 |

#### A.2.3 Exit codes, errors, messages

| ID | Topic | L | M | W | Ev. | Src | snp | Test |
|---|---|---|---|---|---|---|---|---|
| P-40 | `executable` tag | `access(X_OK)` (uid-aware, follows symlinks) | same | **True for every existing path** (including a dangling symlink) | VS | `cpy:posixmodule.c:3162-3178`; `id:identify.py:56-60` | `cfg(windows)` → `true` (08 F4) | W03 |
| P-41 | `- exit code:` value | `returncode`, `-N` for signals | same | unsigned DWORD: `exit /b -1` → `4294967295`; access violation → `3221225477`; Ctrl-C → `3221225786` | VS + VW (Rust side) | `cpy:clinic/_winapi.c.h:577`; `cpy:subprocess.py:1580-1595`; `rs:sys/process/windows.rs:762-764` | `i64::from(code as u32)` on Windows | W10 |
| P-42 | xargs aggregate | `max(abs)`, first wins | same | same, applied to unsigned values (so effectively `max`) | VS | `pc:xargs.py:179-182` | Compare on the `i64` from P-41 | W10 |
| P-43 | `hook-impl` process exit code | `retv \| run()`, then `SystemExit` → `& 0xFF` by the OS | same | `SystemExit(n)` with n > 0x7FFFFFFF → 0xFFFFFFFF before gh-125842 (fixed in 3.13.1); from 3.13.1 on, truncated to 32 bits | VS (CPython gh-125842 commit, read via web) + DER | `pc:hook_impl.py:269-275` | Windows: `std::process::exit(code as u32 as i32)` as the last statement of `main` after all guards have dropped; POSIX: `ExitCode::from(u8)` per 03 | W10 |
| P-44 | OSError text at spawn | `[Errno N] strerror: 'path'` | same | `[WinError N] <FormatMessageW text, trailing whitespace and '.' stripped>`, plus `: 'repr(path)'` only when the exception carries a filename (`os.*` calls, not `CreateProcess`); text localized | VS | `cpy:errors.c:884-886`; `cpy:exceptions.c:1993-2013` | §A.7.4; Tier B in tests (localization) | W09 |
| P-45 | Crash log availability | `os.access(store, W_OK)`; else `Failed to write to log at …` | same | `W_OK` on a directory is always True, so the `Failed…` branch never runs (`# pragma: win32 no cover`) | VS | `pc:error_handler.py:34-39` | Same | – |
| P-46 | Store `readonly` | `exists && !access(W_OK)` | same | never readonly (directory rule) | VS | `pc:store.py:65-68` | Same | – |
| P-47 | Interrupt | SIGINT → `KeyboardInterrupt` → `Interrupted (^C): KeyboardInterrupt: ` + 130 | same | Console CTRL_C_EVENT goes to **every process on the console**, hooks included; Python main thread → same message, 130. A Rust program without a handler dies with `0xC000013A`, **with no destructors run**. | VS + DER | `pc:error_handler.py:71-81`; `rs` (no default handler) | Install a console ctrl handler (tokio `signal::windows::ctrl_c` or `ctrlc`) → restore the stash, exit 130 | W19 |
| P-48 | Ctrl-C in mintty without ConPTY | – | – | MSYS2 kills native children with `TerminateProcess`; neither tool can clean up | DER | – | Document; recommend Windows Terminal or ConPTY | manual M-T3 |

#### A.2.4 Output, color, encoding, newlines

| ID | Topic | L | M | W | Ev. | Src | snp | Test |
|---|---|---|---|---|---|---|---|---|
| P-50 | `--color auto` | `isatty(stderr) && TERM != 'dumb'` | same | `CRT isatty(stderr)` (true for console **and NUL**) `&& vt_ok && TERM != 'dumb'`, where `vt_ok` = `GetConsoleMode(GetStdHandle(STD_ERROR_HANDLE))` and `SetConsoleMode(mode \| 4)` both succeed, **evaluated once at import** | VS | `pc:color.py:7-57,93-100` | Windows: `stderr_console_enable_vt()` from `snp-sys` (§A.7.2). Do **not** use `IsTerminal` or anstyle-query for this decision. | W11 |
| P-51 | Git Bash in mintty (no ConPTY) | – | – | stderr is an MSYS pty **pipe**: Python sees no console, so **no color**. Rust `IsTerminal` → **true** (pipe name `msys-…-pty…`). | VS | `rs:sys/io/is_terminal/windows.rs:25-70` | Parity: no color (D-WIN-07 open) | manual M-T1 |
| P-52 | Console VT side effect | – | – | `SetConsoleMode` persists on the console buffer after exit; children may reset it (prek #1237/#950) | DER | – | Re-enable VT before each colored write (no byte change) | W11 |
| P-53 | Writing bytes to a console | – | – | Python `_WindowsConsoleIO`: `MultiByteToWideChar(CP_UTF8, 0, …)`, so invalid UTF-8 becomes U+FFFD. Rust console stdout returns `InvalidData` ("Windows stdio in console mode does not support writing non-UTF-8 byte sequences"). | VS | `cpy:Modules/_io/winconsoleio.c:1085-1106`; `rs:sys/stdio/windows.rs:134,156` | Console only: `String::from_utf8_lossy` per write; pipes and files get raw bytes | W12 |
| P-54 | Newlines on stdout | LF | LF | `output.write*` → LF; `print()` and argparse via text `sys.stdout`/`stderr` → **CRLF** when redirected (`newline=NULL` on MS_WINDOWS) | VS | `cpy:Python/pylifecycle.c:2454-2463`; `pc:output.py:9-33`; print sites `pc:commands/sample_config.py:17`, `hook_impl.py:56-66`, `migrate_config.py:132-134`, `validate_config.py:15`, `validate_manifest.py:15`, `meta_hooks/*.py` | **D-WIN-04:** LF everywhere | W13 |
| P-55 | Encoding of `print()` when redirected | UTF-8 | UTF-8 | ANSI code page (cp1252), `errors='strict'` for stdout, unless UTF-8 mode (`PYTHONUTF8=1`; default from CPython 3.15, PEP 686) → `UnicodeEncodeError` possible | DER + VD | `cpy:pylifecycle.c` | UTF-8 always | W13 |
| P-56 | Files written in text mode | LF | LF | **CRLF**: hook script; `migrate-config` rewrite (also the **locale encoding**); autoupdate's reformat path `open(path,'w')` (`pc:autoupdate.py:135`, after which CRLF is *preserved* by the `newline=''` write at `:158`); try-repo temp config (`pc:try_repo.py:68`); store `README` (`pc:store.py:72`); `_make_local_repo` templates (`pc:store.py:55`) | VD + VS | as listed | LF for files snp creates; preserve the existing EOL for files snp rewrites (01 Q9) | W13 |
| P-57 | Hook-name width (`_len_cjk`) | – | – | No platform difference | – | – | – | – |

#### A.2.5 Paths and filesystem

| ID | Topic | L | M | W | Ev. | Src | snp | Test |
|---|---|---|---|---|---|---|---|---|
| P-60 | Candidate filename normalization | none (`\` is a literal character) | none | each `\` → `/` before filtering (`os.altsep == '/' and os.sep == '\\'`) | VS | `pc:commands/run.py:119-124` | Same, once at ingestion | W14 |
| P-61 | cwd and `abspath` | `getcwd` = physical | physical: `/tmp/x` is `/private/tmp/x` | `GetFullPathNameW` (lexical, keeps SUBST drive letters) | VL (symlink analog) | `pc:main.py:175-199` | `std::path::absolute` + a lexical relpath port; **never** `canonicalize` (which resolves SUBST and network drives) | M01, W20 |
| P-62 | `--files` given as a logical path through a symlink | relpath from the physical cwd → `../../../tmp/repo/a.py` passed to hooks | same, hit on every `/tmp`/`/var` path | n/a (junctions rarely matter) | **VL**: hook printed `[../../../tmp/repo/a.py]`, rc 0 | `pc:main.py:180,193` | Parity | M01 |
| P-63 | relpath across drives or UNC | – | – | `ValueError: path is on mount 'D:', start on mount 'C:'` → exit 3 | **VN** | `cpy:Lib/ntpath.py:738-768`; `pc:main.py:191-197` | **D-WIN-06:** keep an absolute path; no crash | W15 |
| P-64 | SUBST and mapped drives | – | – | `--show-cdup` + `abspath` keeps `X:` (`--show-toplevel` exposes the volume since git 2.25) | VS | `pc:git.py:51-73` | `rev-parse --show-cdup` (03 §20.2); no canonicalize | W20 |
| P-65 | Store dir | `realpath(PRE_COMMIT_HOME or join(XDG_CACHE_HOME or expanduser('~/.cache'), 'pre-commit'))` | same; realpath may give `/private/…`; **not** `~/Library/Caches` | `expanduser('~/.cache')` = `%USERPROFILE%/.cache` (mixed separators) → realpath gives `C:\Users\me\.cache\pre-commit`; `HOME` ignored (3.8+); empty `USERPROFILE` → `\.cache\…` on the current drive | VS + VN + VL (realpath printed) | `pc:store.py:25-36`; `cpy:ntpath.py:351-380` | Port a non-strict `realpath` (resolve the existing prefix, append the rest); Windows home = `USERPROFILE` (Rust ≥1.85 `home_dir` uses it too, but falls back to the profile API when it is empty) | W18, M01 |
| P-66 | `.pre-commit-config.yaml` path case | case-sensitive | **case-insensitive** (APFS default) | case-insensitive (NTFS) | DER | – | Parity (use OS open). Meta-hook regex `^\.pre\-commit\-config\.yaml$` stays case-sensitive | M03 |
| P-67 | Unicode normalization of filenames from git | bytes as stored | `git init`/`clone` probe and set `core.precomposeunicode=true` on decomposing filesystems (git ≥1.8.5), so git emits **NFC**; argv pathspecs are precomposed too | UTF-8 (Git for Windows converts from UTF-16) | VS (probe) + DER (APFS outcome) | `git:setup.c:2594-2640`; `git:compat/precompose_utf8.c:50-72` | **Never normalize**; pass git's bytes; `--files` passes through as given (NFD stays NFD) | M02 |
| P-68 | Symlinks in the worktree | real | real | `core.symlinks=false` is the usual default on user machines (the installer option "Enable symbolic links" is off), so links check out as small text files: tags `file,text,executable`; a symlinked config becomes a YAML **string** | VS (git probe) + DER | `git:setup.c:2624-2633` | Parity (the cfgv error for a non-map config, 06 row #858) | W21 |
| P-69 | Symlink vs junction classification | – | – | Python `lstat`: `S_IFLNK` **only** for `IO_REPARSE_TAG_SYMLINK`, so a junction counts as a directory. Rust `symlink_metadata().is_symlink()` is true for any name-surrogate reparse point (symlink **and** junction). | VS | `cpy:Python/fileutils.c:1140-1150`; `rs:sys/fs/windows.rs` `FileType::new` | Windows: `is_symlink() && is_dir()` is ambiguous; classify it as `directory`. Only directory *symlinks* then diverge (`symlink` in Python), which is rare in git paths. Low priority (A.9 Q7). | – |
| P-70 | Read-only files on delete | – | – | `rmtree` onexc: chmod `+w` on the path **and its parent**, then retry, on EACCES/EPERM | VS | `pc:util.py:205-235` | Port it; also retry `ERROR_SHARING_VIOLATION` (32) with backoff (antivirus; prek fs.rs) | W22 |
| P-71 | Filename illegal characters | – | – | `:`, `<>|?*"` and reserved names are illegal; `py_env-C:\Python311\python.exe` (a realistic `language_version`) becomes the component `py_env-C:` → invalid (ADS syntax) | VN (join) + DER (CreateDirectory failure) | `pc:lang_base.py` environment_dir; `cpy:ntpath.join` | Hashed env-dir names (§A.4.6) | W23 |
| P-72 | Long paths (>260) | – | – | nodeenv receives `\\?\…`; Rust std fs calls convert long paths to verbatim automatically; `cmd.exe` cannot use verbatim paths (Rust's `to_user_path` strips them) | VS + DER | 04a §3; `rs:sys/args/windows.rs:340-` | Use `dunce::simplified` for argv; keep store paths short | W24 |
| P-73 | Temp dir | `$TMPDIR` | `/var/folders/…` → `/private/var/…` | `%TEMP%`, often an 8.3 short name (`C:\Users\RUNNER~1\…`) | DER | – | Tests canonicalize roots (10 §7.3) | – |

#### A.2.6 Concurrency, locks, environment

| ID | Topic | L | M | W | Ev. | Src | snp | Test |
|---|---|---|---|---|---|---|---|---|
| P-80 | `cpu_count()` | `len(sched_getaffinity(0))` | `os.cpu_count()` = `sysconf(_SC_NPROCESSORS_ONLN)` (+ `PYTHON_CPU_COUNT` on 3.13+) | `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)` (+ `PYTHON_CPU_COUNT` on 3.13+) | VS + VD | `pc:xargs.py:26-38`; `cpy:posixmodule.c:14444-14474` | §A.4.3 | W18 |
| P-81 | Store lock | `flock(LOCK_EX\|LOCK_NB)`, then blocking | same | `open(.lock,'a+')`: position = EOF, then `msvcrt.locking(fd, LK_NBLCK, 0xffff)`; on failure `blocked_cb()` and loop on `LK_LOCK` (10×1 s per call), retrying while errno == `EDEADLOCK` | VS + VD | `pc:file_lock.py:10-48,73`; `cpy:fileio.c:484`; `cpy:PC/msvcrtmodule.c:123-140` | `File::lock` (LockFileEx over the full range) interoperates (**VW** both directions) | W25 |
| P-82 | Lock release on crash | kernel | kernel | The OS releases the lock on handle close or process exit, though not immediately (LockFileEx docs) | VD | – | Same | – |
| P-83 | Env var name case | case-sensitive | case-sensitive | Lookups are case-insensitive (`os.environ` upper-cases keys; Rust `var_os` → `GetEnvironmentVariableW`). **Copies** passed to children carry UPPER-cased names in Python. `no_git_env` matches a lower-case `git_dir` as `GIT_DIR`. | VS | `cpy:Lib/os.py:774-787`; `pc:git.py:27-48` | Windows: compare names with `eq_ignore_ascii_case` / upper-case before `starts_with("GIT_")`; preserve the original spelling in child envs (D-WIN-05) | W17 |
| P-84 | `__PYVENV_LAUNCHER__`, `PYTHONEXECUTABLE` | popped | popped (the macOS framework-python reason) | popped | VS | `pc:main.py:36-42` | `env_remove` both on every child | – |
| P-85 | fsmonitor | – | daemon (`core.fsmonitor=true`) can linger in temp clones | `core.useBuiltinFSMonitor` (deprecated Git for Windows key) or `core.fsmonitor=true`: the daemon holds directory handles, so temp-dir cleanup fails (#2046) | VS + DER | `pc:git.py:16,189,240`; `pc:autoupdate.py:40` | §A.4.4 | W26 |
| P-86 | macOS open-files limit | – | soft `RLIMIT_NOFILE` often 256 in GUI/launchd contexts | n/a | DER | – | Hold O(jobs) fds; optionally raise the soft limit to the hard limit with `rustix::process::setrlimit` (safe) | M07 |
| P-87 | macOS `/usr/bin/git` | – | an `xcrun` shim (extra ~10–20 ms per call, prek P12) | – | DER | 06 §3 P12 | If `git` resolves to `/usr/bin/git`, resolve once via `xcrun --find git` and cache it (no output change) | M06 |

#### A.2.7 Git defaults that change observable behaviour

| ID | Setting | L | M | W | Ev. | Consequence | Test |
|---|---|---|---|---|---|---|---|
| P-90 | `core.fileMode` | true | true | `false` (the `git init` probe fails because chmod on NTFS cannot toggle `S_IXUSR`) | VS `git:setup.c:2594-2606` | Git modes are **not authoritative** on Windows: a new script is 100644 unless `git update-index --chmod=+x`. pre-commit never consults git modes for the `executable` tag (P-40); pre-commit-hooks does for its shebang checks. | W03 |
| P-91 | `core.autocrlf` | unset | unset | `true` (installer default "Checkout Windows-style, commit Unix-style"). GHA runners install Git with the defaults except `/o:PathOption=CmdTools`, `/o:BashTerminalOption=ConHost` and `/o:EnableSymlinks=Enabled`, and they add `C:\Program Files\Git\bin` to `PATH`. | DER + VS (actions/runner-images `images/windows/scripts/build/Install-Git.ps1:25-52`) | Exercises the stash quirks (`pc:staged_files_only.py:26-32,62-66`) | W27 |
| P-92 | `core.symlinks` | true | true | false for typical users; **true on GHA** (`EnableSymlinks=Enabled`) | VS | CI must test both | W21 |
| P-93 | `core.ignorecase` | false | **true** (probe) | **true** | VS `git:setup.c:2635-2638` | Parity (git paths keep index case) | M03 |
| P-94 | `core.precomposeunicode` | – | **true** (probe on HFS+/APFS) | – | VS | NFC paths from git (P-67) | M02 |
| P-95 | `PATH` inside hooks | inherited | inherited | `+ %HOME%\bin;<git>\mingw64\bin;<git>\usr\bin;` when `MSYSTEM` is unset | VS | `sh`, `bash`, `env` resolvable in hooks (P-31) | W02 |

#### A.2.8 Languages: carried over from 04a §9 (unchanged) plus additions

04a §9 rows remain valid: venv `Scripts`, `py -X.Y`, node `default` on Windows, node `Scripts\` layout, the download archives, the rustup URL, ruby unsupported, docker `-u` omitted, mountinfo.

Additions:

- **node shims are `.cmd`.** They fall under P-25 and P-26. `find_executable('prettier')` returns `…\prettier.CMD` (dir-major, extension case taken from `PATHEXT`) before the extension-less sh shim. The `which` crate would return the **bare sh shim first** (§A.4.2), which would then fail with ``Executable `/bin/sh` not found``.
- **`language_version` given as a Windows absolute path.** This is broken in pre-commit (P-71).
- **macOS Rosetta.** pre-commit downloads for `platform.machine()` of the *running Python*. snp's arch is fixed at compile time (`std::env::consts::ARCH`). An x86_64 snp under Rosetta therefore downloads x86_64 toolchains, which matches an x86_64 Python. Ship native arm64 builds.

---

### A.3 Item-by-item analysis of the claims flagged in the gap brief

Each item has the form: claim → evidence (tag) → status → consequence for snp → test.

**W-1. "The hook script is written in text mode, so CRLF" (03 §2.4/§18, previously "derived").**
- Evidence: `open(hook_path, 'w')` with no `newline` argument (`pc:install_uninstall.py:92`) translates `\n` to `os.linesep` = `\r\n` on win32 (VD: docs.python.org `open()`).
- Status: **confirmed by documentation**; byte layout reproduced in VL.
- Exact bytes on win32: `#!/bin/sh\r\n#!/usr/bin/env bash\r\n# File generated by pre-commit: https://pre-commit.com\r\n# ID: 138fd403232d2ddd5efb44317e38bf03\r\n\r\n# start templated\r\nINSTALL_PYTHON='C:\…\python.exe'\r\nARGS=(hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit)\r\n# end templated\r\n…`
- `is_our_script` searches the hash in raw bytes, so it works with either EOL.
- Test: W01 asserts the oracle's bytes (CRLF) and that a commit works through Git for Windows.

**W-2. "How Git for Windows sh runs it".**
- Evidence: VS (`gfw:compat/mingw.c`):
  - `find_hook` ignores `X_OK` (`mingw_access` masks it) and also tries `<hook>.exe`.
  - `mingw_spawnvpe` → `parse_interpreter(prog)`: it skips `.exe` files, reads up to `MAX_PATH-1` bytes, requires `#!`, ends the line at the first `\r` or `\n`, takes the text after the last `/` or `\`, and cuts it at the first space (so `#!/usr/bin/env bash` → `env`).
  - Then `path_lookup(interp, exe_only=1)` → `<dir>\<interp>.exe` on `PATH` (or a BusyBox applet). The final command line is `<interp> <hook path> <args>`.
  - pre-commit's script starts with `#!/bin/sh`, so the interpreter is `sh.exe`. In Git for Windows that is MSYS2 bash, which handles the CRLF (W-1) and the bash arrays (the pre-commit comment calls it "POSIXLY_CORRECT" bash).
- Status: code paths confirmed. Whether every Git for Windows release ships the patched MSYS2 bash is **DER**.
- snp: LF, POSIX sh and an option-free shebang make every one of these paths moot.
- Test: W01, W02, manual M-B1 (BusyBox).

**W-3. "`os.access(X_OK)` true for every file → every file executable → shebang parsing for extension-less files" (08 §4.3).**
- Evidence: VS `cpy:posixmodule.c:3162-3178`. The `mode` argument is consulted only for bit 2 (`W_OK`), so `X_OK` is ignored.
- Status: **confirmed by source**.
- Corollaries:
  - `types: [non-executable]` never matches on Windows.
  - Directories are "accessible" too, but identify returns `{directory}` before that check.
  - A dangling symlink counts as accessible (link attributes).
  - `parse_shebang_from_file` also gates on `X_OK` (`id:identify.py:209`), so every extension-less file is opened and its first two bytes read.
- Test: W03.

**W-4. "`msvcrt.locking` region 0xffff vs Rust File::lock (LockFileEx) interop" (07 D7).**
- Evidence: VS for both sides, plus VW (§A.1 #7).
- Additional facts:
  - Python seeks to EOF on `'a+'` open (`cpy:fileio.c:484`), so pre-commit locks `[size, size+0xffff)`. For the normal empty file that is `[0, 0xffff)`. The lock is mandatory, but neither tool reads or writes `.lock`.
  - Rust uses `LockFileEx(.., 0, u32::MAX, u32::MAX)` from offset 0 (`rs:sys/fs/windows.rs:409-478`). Every pre-commit region is contained in it.
  - pre-commit's `blocked_cb` message (`[INFO] Locking pre-commit directory`) prints once, and then it waits indefinitely: an `EDEADLOCK` after 10 failed 1 s attempts loops (`pc:file_lock.py:28-38`).
- Status: **mutual exclusion verified (VW)**; Windows retry timing is UNK and not needed.
- Test: W25, on real Windows with the real Python oracle.

**W-5. "Unsigned NTSTATUS exit codes in `- exit code:`" (10 Q13).**
- Evidence: VS (`Py_BuildValue("k")`), plus VW for Rust's `code()`, `as u32` and `Display`.
- Status: **confirmed**.
- Values (VW): `exit(-1)` → `code()=Some(-1)`, `as u32 = 4294967295`, `Display` = `exit code: 0xffffffff`. `0xC0000005` → `-1073741819` / `3221225477` / `exit code: 0xc0000005`. `0xC000013A` → `-1073741510` / `3221225786`.
- Test: W10.

**W-6. "`[WinError N]` text" (02 §11).**
- Evidence: VS for the format and the stripping.
- Status: format **confirmed**. The message text is OS-localized and **UNK** per machine, so it is Tier B in tests.
- Exact rules:
  - `OSError_str` on Windows prefers `winerror`: `[WinError {n}] {strerror}` plus `: {filename!r}` and ` -> {filename2!r}` when present.
  - `strerror` = `FormatMessageW(FROM_SYSTEM|IGNORE_INSERTS, lang=MAKELANGID(LANG_NEUTRAL,SUBLANG_DEFAULT))` with every trailing char `<= ' '` or `== '.'` removed.
  - English examples: 193 `%1 is not a valid Win32 application`; 2 `The system cannot find the file specified`; 5 `Access is denied`; 32 `The process cannot access the file because it is being used by another process`; 206 `The filename or extension is too long`; 123 `The filename, directory name, or volume label syntax is incorrect`.
  - `subprocess.Popen` errors come from `_winapi.CreateProcess` → `PyErr_SetFromWindowsErr(0)` with **no filename**. Errors from `os.remove`, `os.rename` and `shutil` carry `: 'C:\\…'`, with backslashes doubled by `repr`.
- Test: W09.

**W-7. "BatBadBut InvalidInput for .cmd shims" (04a §14.6).**
- Evidence: VS + VW.
- Status: **confirmed, with a narrower trigger than 04a implied.** Only CR or LF in an argument (and NUL anywhere) gives `InvalidInput`, plus a script path containing `"` or ending with `\`.
- Every other argument is quoted. For example `a&b.js` reaches node intact as `"a&b.js"`, whereas pre-commit passes it to cmd.exe unquoted, where `&` splits the command (DER from `list2cmdline` and cmd.exe parsing).
- Git allows filenames with newlines, so CR/LF can occur; the snp behaviour is D-WIN-03.
- Test: W07.

**W-8. "VT enabling on stderr for colour".** See P-50 to P-52. Status: the code path is **confirmed (VS)**; the terminal-specific results are UNK. Test: W11 (ConPTY harness) plus the manual terminal matrix M-T1/2.

**W-9. "`relpath` across drives".** P-63. The `ValueError` text is **verified (VN)**. GHA Windows runners place the workspace on `D:\a\…` and `%TEMP%` on `C:`, which makes a natural test. Test: W15.

**W-10. Store and paths on Windows** (03 §18 "drive letter case; SUBST").
- `ntpath.realpath` uses `GetFinalPathNameByHandleW`. It canonicalizes case and resolves SUBST and junctions for the **store path only**, which is printed in `Check the log at …`.
- The cwd and config path are never realpath'd (P-61, P-64).
- Status: DER, with the Python source read. Test: W18 and W20.

**W-11. Text-mode CRLF in other outputs.** P-54 and P-56 (VS). This is new: no earlier document listed it. Test: W13.

**W-12. Env-var case.** P-83 (VS). This is new. Test: W17.

**M-1. "macOS `/tmp` realpath".**
- VL analog (layout `B/tmp -> private/tmp`, cwd reached through the link; script in §A.0 artifacts):
  - `run --files $LOGICAL/tmp/repo/a.py` → hook argv `../../../tmp/repo/a.py`, rc 0.
  - `run -c $LOGICAL/…/.pre-commit-config.yaml` → works.
  - `PRE_COMMIT_HOME=$LOGICAL/tmp/pchome` → `Check the log at $PHYSICAL/private/tmp/pchome/pre-commit.log`.
- Status: **logic verified**. On macOS the trigger is any path through `/tmp`, `/var` or `/etc`. Test: M01.

**M-2. "NFD/precomposeunicode filenames from git".**
- VS: `git init`/`clone` creates `ä` NFC, tests `access()` of the NFD spelling, and writes `core.precomposeunicode=true` if that succeeds, unless a global value exists (`git:compat/precompose_utf8.c:50-72`).
- With `true`, readdir results and argv pathspecs are precomposed. The index stores NFC, so `git diff --name-only -z` emits NFC.
- pre-commit decodes UTF-8 and never normalizes. Hooks get NFC from git, but whatever the user typed for `--files`.
- Status: code **confirmed**; the APFS outcome is DER.
- Test: M02, which covers NFC repo, NFD argv and `files:` regexes with NFC literals.

---

### A.4 Contradictions resolved (platform-related)

#### A.4.1 Hook stdin: 02 §21.3 and 06 §8 (`Stdio::null()`) vs 04a §14.3 (empty pipe)

- **Decision: 04a §14.3 and 02's TL;DR #9 are correct. 02 §21.3 and 06 §8 are wrong.** Use `Stdio::piped()` and drop the `ChildStdin` immediately, on every OS, for the non-pty path. Use `/dev/null` only for the POSIX pty path.
- Evidence:
  - `pc:util.py:81-84` sets `stdin=PIPE`, and `communicate()` closes it.
  - `pc:util.py:176-178` uses `os.devnull` only in `cmd_output_p`.
  - **New, Windows-specific:** `Stdio::null()` is `\\.\NUL` (`rs:sys/process/windows.rs:643`), and `NUL` is a character device.
    - Under Wine a child's CRT `_isatty(0)` returned **1** with `Stdio::null()` and **0** with the dropped pipe (VW; `GetFileType` = `FILE_TYPE_CHAR` vs `FILE_TYPE_PIPE`).
    - Microsoft documents `_isatty` as "character device (a terminal, console, printer, or serial port)" (VD).
    - CPython's `FileIO.isatty` calls CRT `isatty` (`cpy:fileio.c:1146`).
    - So with `NUL`, Python hooks (and every MSVCRT/UCRT program) would believe stdin is interactive. Prompts, `click.confirm` and `input()` would behave differently from pre-commit.
- Update 02 §21.3, 06 §8 and 07 §7.15 accordingly.

#### A.4.2 Executable lookup: 04a §14.18 (no `which`) vs 06 §8 ("PATHEXT via `which`")

- **Decision: 04a is correct.** Port `find_executable`, `normexe` and `normalize_cmd` (about 60 lines).
- Concrete `which 8.0.6` differences on Windows (VS, `crate:which-8.0.6/src/{finder.rs:250-290, sys.rs:150-325, helper.rs:4-12}`):
  1. It tries the **bare name first**, then the `PATHEXT` extensions. pre-commit tries extensions first, then the bare name. In a node env dir that holds both `prettier` (sh shim) and `prettier.cmd`, `which` returns the sh shim. On Windows that is "executable", gets shebang-parsed to `/bin/sh`, and fails.
  2. `PATHEXT` is read **once per process** from the parent env (a `OnceLock`). pre-commit reads the **child** env's `PATHEXT` when `env=` is given.
  3. It skips `PATHEXT` segments without a leading dot. pre-commit uses all segments.
  4. It skips appending extensions when the name already has an executable extension. pre-commit always tries `name+ext` first (harmless, but a different probe order).
  5. It resolves relative `PATH` entries against the cwd and returns absolute paths. pre-commit returns the relative join.
  6. It splits `PATH` with `std::env::split_paths`, which on Windows honours `"`-quoted entries. pre-commit uses `.split(';')`, so a quoted entry never matches.

#### A.4.3 CPU count: 02 §21.4 / 04a §14.4 (`sched_getaffinity`) vs 03 §20.6 (`available_parallelism` for `autoupdate -j 0`)

- **Decision: one function, `py_cpu_count()`, used by `target_concurrency` (`pc:lang_base.py:135-143`) and by `autoupdate -j 0` (`pc:autoupdate.py:187`).** 03 §20.6 must call it.
  - Linux: `rustix::thread::sched_getaffinity(None)?.count()`. `available_parallelism` also applies cgroup quotas, which Python does not (07 §7.13).
  - macOS: `available_parallelism()`. It is the same `sysconf(_SC_NPROCESSORS_ONLN)` that CPython uses, because `HAVE_SYSCONF` wins over the `__APPLE__` sysctl branch in `cpy:posixmodule.c:14454-14468`.
  - Windows: `snp_sys::active_processor_count()` = `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`. If `snp-sys` is rejected, fall back to `available_parallelism()` and accept a divergence on hosts with more than 64 CPUs (documented).
  - macOS and Windows only: if `PYTHON_CPU_COUNT` is a positive integer, pre-commit running on CPython 3.13+ uses it. On Linux it does not, because `sched_getaffinity` comes first. snp ignores it by default (A.9 Q3).
  - `PRE_COMMIT_NO_CONCURRENCY` (any value, even empty; case-insensitive name on Windows) gives 1, and `TRAVIS` gives 2, before the count is consulted.

#### A.4.4 fsmonitor flag: 06 §8 (always) vs pre-commit (3 sites) vs 03 §20.2 (store and autoupdate)

- **Decision: 03 §20.2, extended.** Pass `-c core.useBuiltinFSMonitor=false -c core.fsmonitor=false` on **every** git command that runs inside an snp-owned repository: store clones, autoupdate temp dirs, try-repo temp clones, and `git tag --points-at` on a store repo. Never pass them for commands in the user's repository.
- Rationale:
  - pre-commit passes `NO_FS_MONITOR` only to `init` and `remote add` (`pc:git.py:189-193`, a per-command `-c` that is **not** persisted, so the later `fetch`/`checkout` in `store.py` run without it), to `tag --points-at` (`:240`) and to all autoupdate temp-dir commands (`pc:autoupdate.py:40`).
  - The key `core.useBuiltinFSMonitor` is Git for Windows' pre-2.36 experimental key. Modern Git uses `core.fsmonitor=true`, which pre-commit does **not** neutralize, so #2046-style cleanup failures can recur on Windows.
  - On macOS the daemon just lingers.
  - In the user's repo, fsmonitor speeds up `diff` and `status`, and disabling it changes no output.
- Test: W26.

#### A.4.5 Stash restore: 02 §21.10 (pre-commit's algorithm) vs 06 §7 item 9 / §8 (read-tree + verify)

- **Decision: 02 §21.10.** Keep `git apply --whitespace=nowarn <patch>`, the retry with `git -c core.autocrlf=false apply`, then `checkout -- .` and re-apply (`pc:staged_files_only.py:26-32,80-98`). Add only fsync and an `O_EXCL` patch file.
- Platform reason:
  - Both quirk branches exist **because of Windows `core.autocrlf`**: the `retcode == 1 and not diff_stdout.strip()` early exit (`:62-66`) and the #570 retry.
  - With `autocrlf=true`, `git diff-index --binary <tree>` is not stable across a CRLF re-checkout: a stat-dirty, CRLF-only difference can appear or vanish.
  - So 06's post-restore verification ("`diff-index` equals the patch") would report false mismatches on Windows. Its `read-tree`/`checkout` restore also leaves a different index and worktree state on conflict than pre-commit does.
- 06's idea can live as an opt-in `snp`-only self-check that only **warns**. Test: W27.

#### A.4.6 Env-dir naming: 03 §20.5 item 9 (sanitize or hash) vs 04a §14.2 (literal nested dirs)

- **Decision: 03 §20.5 item 9.** Use `<ENV>-<version>` only when the version matches `^[A-Za-z0-9._+-]{1,64}$`; otherwise use `<ENV>-h<hash>`.
- Platform reason:
  - A realistic Windows `language_version: C:\Python311\python.exe` becomes `ntpath.join(prefix, 'py_env-C:\\Python311\\python.exe')` = `…\py_env-C:\Python311\python.exe` (VN). The component `py_env-C:` is invalid on Windows (`:` is reserved; ADS syntax), so pre-commit cannot create that env. That is DER; test W23.
  - snp does not share env dirs with pre-commit (03 §20.5 item 2), so the literal scheme buys no compatibility.

#### A.4.7 Store location, crash log and lock sharing (platform facts only)

The decision itself stays open in 03, 10 Q5 and 10 Q9. The platform facts that bear on it:

- **pre-commit's default is the same on all three OSes.** It is `realpath(join(expanduser('~/.cache'), 'pre-commit'))`, which gives `%USERPROFILE%\.cache\pre-commit` on Windows. It is never `%LOCALAPPDATA%` (prek's choice) or `~/Library/Caches`.
- **If snp ever takes pre-commit's `<store>/.lock`, interop is verified.** This holds in both directions, under Wine and with `flock` on Linux (07 D7).
- **On Windows, the crash-log "Failed to write" branch cannot happen** (P-45).

---

### A.5 snp target matrix

#### A.5.1 Build and support tiers

| Target | Tier for snp | CI runner | Notes |
|---|---|---|---|
| `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | T1 | ubuntu-24.04, ubuntu-24.04-arm | reference platform |
| `*-linux-musl` | T1 (release), T2 (tests) | container | static; WSL users may prefer it |
| `aarch64-apple-darwin` | T1 | **macos-26** (= `macos-latest` after the June–July 2026 migration), macos-15 | sign and notarize the release binary (Gatekeeper quarantines browser downloads) |
| `x86_64-apple-darwin` | T2 (Rust demoted it in 1.90) | macos-15-intel while available | Rosetta note (A.2.8) |
| `x86_64-pc-windows-msvc` | T1 | **windows-2025** (pin it; `-latest` moves) | static CRT (`+crt-static`) so no VC++ redistributable is needed |
| `aarch64-pc-windows-msvc` | T1 | windows-11-arm | – |
| `*-windows-gnu` | not shipped | – | used only for the Wine probes |
| cygwin, MSYS2 (`*-pc-cygwin`) | unsupported | – | A Cygwin or MSYS2 **git** (not Git for Windows) prints POSIX paths (`/c/…`). snp detects `rev-parse --show-cdup`/`--git-common-dir` output that starts with `/` on Windows and fails with `snp does not support cygwin/msys2 git; use Git for Windows` (exit 1). pre-commit's warning is dead code (A.1 #13), so this is an addition (D-WIN-08). |

#### A.5.2 How hooks get invoked on Windows (support contract)

| Context | Who runs the hook | Hook stderr | `PATH` extras | pre-commit `auto` color | snp `auto` color (parity) | Status |
|---|---|---|---|---|---|---|
| cmd.exe (conhost or Windows Terminal) → `git commit` | `git.exe` → `sh.exe` (Git's) → `snp.exe` | console | `usr\bin`, `mingw64\bin` (MSYSTEM unset) | yes (VT enabled on stderr) | yes | supported, CI-tested via ConPTY (W11) |
| PowerShell 5.1 / 7 → `git commit` | same | console | same | yes | yes | supported, CI W02/W11 |
| Git Bash in **mintty** (default) | same | MSYS pty **pipe** | MSYS2 env (MSYSTEM set) | **no** | **no** (do not use `IsTerminal`) | supported; manual M-T1 |
| Git Bash with ConPTY (`MSYS=enable_pcon`), or bash.exe inside Windows Terminal | same | console | MSYS2 env | yes | yes | manual M-T2 |
| IDEs and GUIs (VS Code, JetBrains, Visual Studio, GitHub Desktop/dugite, SourceTree, TortoiseGit, Fork) | their git: full Git for Windows, **MinGit**, or MinGit-BusyBox | pipe | varies; MinGit may lack `sh`, so git fails before snp | no | no | supported; BusyBox works thanks to the POSIX template (P-07) |
| Direct `snp run` from cmd/PowerShell | – | console | user `PATH` only (no `usr\bin`) | yes | yes | supported; `#!/usr/bin/env bash` may find WSL's bash (P-31) |
| CI (GHA Windows) | pipes everywhere | pipe | runner adds `Git\bin`, `Git\usr\bin` (`PathOption=CmdTools` + `Git\bin`) | no | no | supported |
| **WSL1/WSL2** with Linux git + Linux snp | Linux | Linux | Linux | Linux rules | Linux rules | supported (it is Linux). Pitfalls: a repo on `/mnt/c` shared with Windows git (P-14); DrvFs exec bits (every file `0777` unless mounted with `metadata`) make every file `executable`, as on Windows; drvfs I/O is slow. |
| WSL git calling Windows `snp.exe` (interop) | – | – | – | – | – | unsupported (path translation); hooks installed from WSL call the Linux snp |

#### A.5.3 macOS invocation contexts

| Context | Note |
|---|---|
| Terminal / iTerm2 / VS Code terminal | Linux rules. A pty for color works (M05). |
| GUI git clients (Tower, SourceTree, Fork, GitHub Desktop, Xcode) | Hooks inherit the **launchd PATH** (`/usr/bin:/bin:/usr/sbin:/sbin`), so `command -v snp` fails for Homebrew installs. The hook script's embedded absolute `SNP_EXE` (03 §20.3) is therefore mandatory, not optional. A Homebrew `Cellar` path changes on upgrade: prefer the `/opt/homebrew/bin/snp` symlink path (03 §20.3 note). |
| `/usr/bin/git` shim | `xcrun` per call (P-87) |

---

### A.6 Verification plan

#### A.6.1 Workflow `platform-compat.yml`

- **Matrix:**
  - `windows-2025`, `windows-11-arm`, `macos-26`, `macos-15-intel`, and `ubuntu-24.04` as the control.
  - Python `3.12` and `3.13`. 3.13 matters for `PYTHON_CPU_COUNT` and the gh-125842 exit-code fix.
- **Runs on:** every PR touching `crates/snp-{exec,platform,sys,output,install}` or `tests/platform/**`, and nightly.
- **Steps:**
  1. Before checkout: `git config --global core.autocrlf false`, `git config --global core.fsmonitor false`, and `git config --global init.defaultBranch master`. Without these, GHA Windows checks the repo out with CRLF, and fixture bytes and SHAs differ.
  2. `actions/checkout`, `setup-python`, `dtolnay/rust-toolchain@1.98.0`.
  3. `cargo build --release -p snp -p snp-testkit --bins`.
  4. `cargo xtask oracle install`. On Windows it uses `py -3.12 -m venv`; the oracle is `target\oracle\pc-4.6.2\Scripts\pre-commit.exe`.
  5. `cargo xtask platform-facts` (A.6.3).
  6. `cargo xtask difftest --suite platform --keep-failures`.
  7. Upload `target/difftest/**` as an artifact.

#### A.6.2 Harness extensions (add to 10 §7.4.3)

New scenario keys:

```yaml
requires:
  os: [windows]                   # existing key; values linux|macos|windows
  console: conpty                 # none (default: pipes) | conpty (Windows pseudo console via portable-pty 0.9) | pty (posix) | nul-stderr
  launcher: git-cmd               # direct (default) | git-cmd | git-pwsh | git-bash  (how `git:` steps are spawned:
                                  #   cmd.exe /d /c git …, pwsh -NoProfile -Command git …, "C:\Program Files\Git\bin\bash.exe" -lc "git …")
  path_profile: runner            # runner (default) | user-minimal (PATH = System32;Windows;Git\cmd only, like a fresh Git for Windows
                                  #   install with the default "Git from the command line" option) | no-usr-bin
  python: "3.12"                  # pick the oracle venv
expect_divergence: D-WIN-02       # accepted, listed in known-divergences.yaml; harness checks it still diverges
compare:
  eol: insensitive                # new: \r\n == \n for this stream (Windows print()/argparse outputs, D-WIN-04)
  winerror_text: tierB            # new: [WinError N] keep N, drop message text (localization)
```

New `snp-probe` subcommands:

| Subcommand | Output | Purpose |
|---|---|---|
| `envnames` | All env var names, sorted, raw case | P-83 |
| `stdin-kind` | `file-type: char\|pipe\|disk\|other` and `crt-isatty: 0\|1`. Unix uses `rustix::fs::fstat(stdin)` (`S_ISCHR`/`S_ISFIFO`). Windows uses the `snp-sys` `probe` feature (`GetFileType`, `_isatty`). | P-29 |
| `ntstatus HEX` | `std::process::exit(u32::from_str_radix(HEX,16) as i32)` | P-41 |

Python stdin check (no Rust FFI needed): a `language: system` hook with entry `<oracle-python> -c "import sys;print(sys.stdin.isatty())"`, using the same absolute python for both tools.

#### A.6.3 `cargo xtask platform-facts` (always printed, never fails)

- `git --version`
- `git config --show-origin --get-all` for `core.autocrlf`, `core.filemode`, `core.symlinks`, `core.ignorecase`, `core.precomposeunicode`, `core.fsmonitor`, `core.useBuiltinFSMonitor`, `core.longpaths`
- `PATHEXT`, `where sh bash env python`
- `python -c "import os,sys;print(os.cpu_count(), getattr(os,'process_cpu_count',lambda:None)(), sys.flags.utf8_mode)"`
- `snp __debug platform`, a hidden subcommand. It prints:
  - `py_cpu_count`, `arg_budget`, `batch_budget(cmd.exe path)`
  - `stderr_is_console`, `vt_enable_ok`, `IsTerminal(stderr)`
  - `home`, `store_dir (realpath)`
  - `PATHEXT parsed`, `git resolved path`
- `ver` and `systeminfo | findstr /B /C:"OS Version"` on Windows; `sw_vers` and `ulimit -n` on macOS.

#### A.6.4 Scenario catalogue

Columns:

- **ID:** the test ID used in §A.2.
- **OS:** where the scenario runs.
- **Setup and steps:** what the scenario builds and runs.
- **Oracle 4.6.2 (predicted):** the expected pre-commit result. A prediction is a hypothesis; the test records the truth.
- **snp expectation:** the expected snp result.
- **Compare:** how the two are compared.

| ID | OS | Setup and steps | Oracle 4.6.2 (predicted) | snp expectation | Compare |
|---|---|---|---|---|---|
| W01 | win | `install`; dump `.git\hooks\pre-commit` bytes; `git commit` via `launcher: git-cmd` with a passing probe hook | Bytes start `#!/bin/sh\r\n#!/usr/bin/env bash\r\n`; commit succeeds | Script LF, `#!/bin/sh`; commit succeeds | hooks_dir: behavior; record oracle bytes in `facts/` |
| W02 | win | Same as W01, ×3 launchers (`git-cmd`, `git-pwsh`, `git-bash`) × 2 path profiles (`runner`, `user-minimal`); probe hook prints `env PATH TERM MSYSTEM HOME` | Commit succeeds in all; `TERM=xterm-256color` when the launcher had none; under cmd/pwsh, `PATH` begins `%HOME%\bin;<git>\mingw64\bin;<git>\usr\bin;…` and `MSYSTEM=MINGW64` | Identical probe output | stdout exact (`PATH` cut to its first 3 entries) |
| W03 | win | Files: `noext` (`#!/usr/bin/env python`, mode 100644), `script.sh`, `b.bin`; hooks with `types: [executable]`, `[non-executable]`, `[python]`, `[shell]` (probe `args`) | executable: all files; non-executable: **no files → `(no files to check)Skipped`**; python: `noext`; shell: `script.sh` | Identical | exact |
| W04 | win | `language: script`, entry `./s.sh` with `#!/bin/bash` (Git bash present) | `Failed`, `- exit code: 1`, ``Executable `/bin/bash` not found`` | **Passed** (basename fallback finds Git's bash.exe) | `expect_divergence: D-WIN-02` |
| W05 | win | Foreign `pre-commit` hook `#!/bin/sh\necho legacy` → `install` (migration) → `git commit` | Legacy: `An unexpected error has occurred: ExecutableNotFoundError: Executable `/bin/sh` not found`, rc 3, commit aborted | `legacy` printed, run proceeds | `expect_divergence: D-WIN-02` |
| W06 | win | Dir with `tool` (sh shim) + `tool.cmd` + `tool.ps1` on the child `PATH`; hook `entry: tool`; also `PATH` with a quoted entry `"C:\x y"` holding `t2.exe`; `PATHEXT` lower-case | Runs `tool.CMD`; quoted entry not found → ``Executable `t2` not found`` | Identical | exact |
| W07 | win | node-style `.cmd` shim (`@echo off` + `"%~dp0\snp-probe.exe" args %*`); staged files `a b.js`, `a&b.js`, `50%PATH%.js`, `x^y.js`, `p(1).js`, `é.js` (`"` cannot occur in NTFS names). Separately, an argument containing `\n`, passed through hook `args:` because Windows filenames cannot contain it. | Unquoted `&` and `^` reach cmd.exe: the command splits at `&` and `^` is dropped. `%PATH%` expands. The newline argument truncates the command line. | Each argument arrives byte-intact. The newline argument gives per-partition output (text: A.9 Q1), rc 1. | `expect_divergence: D-WIN-03`; store both outputs |
| W08 | win | 3000 files with 20-char names, 300 files with names needing quotes (`a,b_000.js`), and 50 names with `%`, through the `.cmd` shim; probe `count` | Partition sizes per `8192-len(cmd.exe)-4-1024` | Same partition boundaries, **and** no `The command line is too long.` errors (snp re-splits if the encoded line exceeds 8191) | stdout exact if no re-split occurred; else `expect_divergence: D-WIN-09` |
| W09 | win | `entry: ./noshebang.sh` (text, no shebang) | `[WinError 193] %1 is not a valid Win32 application` (English runner) rc 1 | Identical text (FormatMessage + strip `.`) | `winerror_text: tierB` + an exact assertion on English runners |
| W10 | win | probe `ntstatus C0000005`, `exit -1` (via `cmd /c exit -1` batch); legacy hook exiting 0xC0000005 + install + commit; echo `%ERRORLEVEL%` after `snp hook-impl` | `- exit code: 3221225477`, `- exit code: 4294967295`; hook-impl exit = `0xFFFFFFFF` (3.12) / `0xC0000005` (3.13.1+) | `3221225477`, `4294967295`; hook-impl exit = full u32 OR | stdout exact; exit: nonzero-equal |
| W11 | win | `console: conpty` (portable-pty ConPTY) running `run --color auto` with a passing and a failing hook; then `console: none`; then stderr redirected to `NUL` (`2>NUL` via cmd) | ConPTY: ANSI colors; pipes: none; `2>NUL`: none (isatty true but VT fails) | Identical bytes on stdout | exact |
| W12 | win | ConPTY; hook prints `b'\xff\xfe ok'` and `é` | Console shows `�� ok`; no crash | Same visible text (lossy), exit as oracle | Screen-scrape ConPTY output |
| W13 | win | `sample-config > out.yaml`; `hook-impl` with a missing config (stdout captured); `validate-config bad.yaml`; `migrate-config` on a legacy list config with LF input; meta hook `check-useless-excludes` failing | CRLF in all of these streams and files; `migrate-config` writes CRLF (locale encoding) | LF | `eol: insensitive` + `expect_divergence: D-WIN-04` (files: compare after EOL normalization, and record the oracle's CRLF in facts) |
| W14 | win | `install` (prints path); worktree `install`; `-c sub/cfg.yaml install`; `run` with `--files sub\a.py` | `pre-commit installed at .git\hooks\pre-commit`; worktree `C:\…\main\.git\hooks\pre-commit`; script contains `'--config=sub\cfg.yaml'`; hook argv `sub/a.py` | Identical, except the script path is not byte-compared (check the `--config` argv by running a commit) | exact |
| W15 | win | Repo on `D:\a\…` (GHA workspace); `run --files C:\Users\…\Temp\x.py`; `install -c C:\…\cfg.yaml` | `An unexpected error has occurred: ValueError: path is on mount 'C:', start on mount 'D:'`, rc 3 | No crash: the absolute path is passed to the classifier, `--config=C:/…` (normalized) is embedded | `expect_divergence: D-WIN-06` |
| W16 | win+mac+linux | Python stdin hook (A.6.2); also probe `stdin-kind` | `False` / `pipe` | Identical | exact |
| W17 | win | Tool-step env only: `git_index_file=C:\nonexistent\idx` (lower-case name) and `MyVar=1`. Run `install-hooks` with a `file://` hook repo, which clones into the store under `no_git_env`. Then a `run` whose probe hook prints `envnames`. | `install-hooks` succeeds: `os.environ` upper-cases the name, so `no_git_env` drops it. The probe lists `MyVar`, with case preserved, because hooks inherit the process env and not a copy. | Same. A naive case-sensitive filter would leak `git_index_file` into the store clone and break `checkout`. | exact |
| W18 | win+mac | `platform-facts`: compare `py_cpu_count` with `os.cpu_count()`; also with `PYTHON_CPU_COUNT=3` on 3.13 | equal; 3 under 3.13 | equal; ignores 3 unless `--compat-python-env` | assert |
| W19 | win | ConPTY; hook `probe sleep 5000` with unstaged changes; send `\x03` into the ConPTY input after 1 s | `Interrupted (^C): KeyboardInterrupt: ` rc 130; stash restored | Same; stash restored | exact + tree |
| W20 | win | `subst X: D:\a\…\repo`; from `X:\` run `install`, `run --files a.py`, and with an existing `.legacy` | All paths printed with `X:`, never the underlying volume | Identical | exact |
| W21 | win | `core.symlinks=false` clone of a repo whose `.pre-commit-config.yaml` is a symlink to `cfg/real.yaml`; and `core.symlinks=true` | false: `InvalidConfigError` text for a string config, rc 1; true: works | Identical (cfgv message) | exact |
| W22 | win | `gc`/`clean` with a read-only git pack in the store and with a file held open by another process (a probe `hold FILE 3000` in the background) | Read-only: succeeds; held: `An unexpected error has occurred: PermissionError: [WinError 32] … : 'C:\\…'` rc 3 | Read-only: succeeds; held: retries ~10 s then the same error class | read-only exact; held `winerror_text: tierB` |
| W23 | win | `language: python`, `language_version: <abs python.exe>` | Failure creating `py_env-C:\…` (`[WinError 123]` or a virtualenv error) | Works (hashed env dir) | `expect_divergence: D-ENV-HASH` |
| W24 | win | Store path near 200 chars (`PRE_COMMIT_HOME` deep); node hook install | Works (nodeenv `\\?\`) or fails on MAX_PATH | Works | record |
| W25 | win+mac | **Lock interop**: (a) `snp __debug hold-store-lock 8` in the background, then oracle `install-hooks` → oracle prints `[INFO] Locking pre-commit directory` and finishes after release; (b) the reverse with the oracle holding the lock via `python -c "from pre_commit.store import Store; s=Store(); import time; ctx=s.exclusive_lock(); ctx.__enter__(); time.sleep(8)"` | (a) waits ≈8 s | (b) snp prints the same INFO line and waits | timing ≥ 7 s + the message |
| W26 | win+mac | Global `core.fsmonitor=true`; run `autoupdate` against a local `file://` repo, `try-repo`, and `gc` | Possible `PermissionError` on temp cleanup (Windows); daemons left (`git fsmonitor--daemon status`) | No error; no daemon in snp-owned dirs | record oracle; assert snp |
| W27 | win+linux | `core.autocrlf=true`; staged LF file + unstaged CRLF-only change; staged + unstaged real change + a hook that rewrites EOL (probe `write`); a conflicting autofix | Exact pre-commit sequence (`Unstaged files detected.` / `Stashing…` / `Restored changes…` / `Stashed changes conflicted…`) | Identical messages and final tree and index | exact + tree + index |
| M01 | mac | Repo at the *logical* path `/tmp/snp-m01/repo`, entered with `cd /tmp/snp-m01/repo` (not canonicalized, on purpose); `run --files /tmp/snp-m01/repo/a.py`; `PRE_COMMIT_HOME=/tmp/snp-m01/pch` plus a forced FatalError (run outside a repo) | Hook argv `../../../tmp/snp-m01/repo/a.py` (cwd is `/private/tmp/snp-m01/repo`); `Check the log at /private/tmp/snp-m01/pch/pre-commit.log` | Identical | exact |
| M02 | mac | Create `ä.txt` (NFC), commit; config `files: 'ä'` (NFC in YAML); run `-a`; run `--files "$(printf 'a\xcc\x88.txt')"` (NFD); repo variant with `core.precomposeunicode=false` | `-a`: matched (NFC); NFD `--files`: hook gets NFD and the regex does not match → `(no files to check)Skipped` | Identical | exact (byte-level argv via probe `args` hex) |
| M03 | mac | `-c .PRE-COMMIT-CONFIG.YAML`; file `Readme.md` staged; hook `files: ^readme` | Config opens; hook skipped (regex case-sensitive) | Identical | exact |
| M04 | mac | 5000 files, probe `count`, `PRE_COMMIT_NO_CONCURRENCY` unset | Partitions per `ARG_MAX` 1 MiB → clamp 131072 and `jobs` = sysconf count | Identical | exact |
| M05 | mac | `console: pty`, `--color always`; hook writes 200 KiB then exits immediately; a hook whose grandchild keeps the pty open 2 s | All bytes captured, no `\r\n`; waits for the grandchild | Identical | exact |
| M06 | mac | Benchmark: `run` with 10 noop hooks, `git` = `/usr/bin/git` vs Homebrew git | – | snp ≤ oracle; xcrun caching effective | timing |
| M07 | mac | `ulimit -n 256`; 20 000 files; 8 jobs | Works | Works (no `EMFILE`) | exit |

Manual matrix (run before each release; checklist in `docs/platform-manual.md`):

- **M-T1:** mintty (Git Bash default): `git commit` with a failing hook. Expect no ANSI color from either tool, readable text, and the same lines.
- **M-T2:** Git Bash with `MSYS=enable_pcon`, and Windows Terminal + cmd/pwsh: color from both tools.
- **M-T3:** Ctrl-C in mintty during a hook with a stash active. Record the state for both tools. Known: no cleanup is possible.
- **M-B1:** MinGit-BusyBox (download the `MinGit-*-busybox-64-bit.zip` used by some GUIs). pre-commit's hook should fail with an ash syntax error at `ARGS=(`; snp's POSIX script works.
- **M-WSL:** Ubuntu WSL2 with the repo on `/mnt/c`. Install from the Windows side with the oracle, then `git commit` from WSL: expect `cannot exec`. Install from the Windows side with snp, then commit from WSL: works if a Linux snp is on `PATH`. The reverse direction works too.
- **M-IDE:** VS Code (Git for Windows) and GitHub Desktop (dugite): a commit with a hook. Expect it to run, with no color.

#### A.6.5 Exit criteria

- A platform row in §A.2 may change from UNK or DER to "verified" only when its test (or manual checklist) passed on the named runner. The result goes into `tests/differential/known-divergences.yaml` or `facts/` with the runner image version.
- Any **oracle prediction** that turns out wrong must be fixed in this matrix in the same PR.

---

### A.7 Implementation notes for snp (platform layer)

#### A.7.1 Crate and module layout

- `crates/snp-platform` (`#![forbid(unsafe_code)]`) holds all `cfg(windows)`/`cfg(target_os="macos")` logic behind plain functions:
  - `py_cpu_count`, `arg_budget`, `batch_budget`
  - `nt_normpath`, `py_relpath`, `py_realpath_nonstrict`
  - `find_executable`, `normexe`, `normalize_cmd`
  - `env_name_eq`
  - `format_os_error`, `display_exit_code`
- `crates/snp-sys` is Windows-only, built with `[lints] unsafe_code = "deny"` plus `#[expect(unsafe_code, reason = "…")]` per function, and `windows-sys 0.61` with features `Win32_System_Console`, `Win32_System_Threading` and `Win32_Foundation`. This is the answer to 07 Q6. It exposes exactly:
  1. `pub fn stderr_console_enable_vt() -> bool`:
     ```rust
     // SAFETY: handle from std's stderr; GetConsoleMode/SetConsoleMode only read/write a DWORD.
     let h = std::io::stderr().as_raw_handle() as HANDLE;
     let mut mode = 0u32;
     if h.is_null() || unsafe { GetConsoleMode(h, &mut mode) } == 0 { return false; }
     unsafe { SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0 }
     ```
     It mirrors `pc:color.py:46-48`. Call it lazily on first use and cache it in a `OnceLock<bool>`, so the side effect happens once, as at pre-commit's import time. Later `ensure_vt()` calls re-run `SetConsoleMode` only, with no new decision (P-52).
  2. `pub fn active_processor_count() -> u32`: `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`, falling back to `available_parallelism` when it returns 0.
  3. Test-only, behind feature `probe`: `crt_isatty(fd) -> bool` (`_isatty`) and `std_handle_file_type(n) -> u32` (`GetFileType`), used by `snp-probe stdin-kind`.
- If the team refuses any FFI:
  - Color: use `anstyle-query::windows::enable_virtual_terminal_processing()` (it fails when stdout is redirected). That is a **documented divergence**: colors are missing when only stdout is piped.
  - CPU: use `available_parallelism`. That is a divergence only on hosts with more than 64 CPUs.

#### A.7.2 Color decision (exact)

```rust
fn use_color(setting: ColorSetting /* from --color or PRE_COMMIT_COLOR */) -> bool {
    match setting {
        Always => true, Never => false,
        Auto => stderr_isatty_parity() && std::env::var_os("TERM").as_deref() != Some("dumb".as_ref()),
    }
}
#[cfg(unix)]    fn stderr_isatty_parity() -> bool { std::io::stderr().is_terminal() }
#[cfg(windows)] fn stderr_isatty_parity() -> bool { snp_sys::stderr_console_enable_vt() } // = CRT isatty ∧ VT ok; NUL fails GetConsoleMode
```

- **Never** use `IsTerminal` on Windows for this decision: it is true for MSYS ptys.
- `TERM`: Git for Windows sets `xterm-256color` for hooks, so in practice it is never `dumb` there.

#### A.7.3 Displayed paths on Windows

- Implement `nt_normpath(&str) -> String` as a port of `ntpath.normpath`: convert `/` to `\`, collapse `.` and `..` and repeated separators, and keep the `\\?\` and UNC prefixes. Unit-test it against VN vectors generated by `xtask oracle vectors` with `import ntpath`.
- Use it for every path pre-commit derives via `os.path.join`/`normpath` and then prints or embeds:
  - the hook path and the legacy path (P-10)
  - the `--config` relpath (P-11)
  - the patch path
  - the log path
  - `Initializing environment for <local dir>`
- **Do not** normalize filenames passed to hooks; they are `/`-normalized (P-60).
- `py_relpath(path, start)` returns `Result<String, CrossDrive>`. pre-commit crashes on `CrossDrive`; snp keeps the absolute (normalized) path (D-WIN-06).
- `py_realpath_nonstrict` is for the store only:
  - Windows: `dunce::canonicalize` on the longest existing prefix, then append the rest.
  - Unix: resolve symlinks component by component.
  - Never call it on cwd, config or file arguments (SUBST, P-64; macOS, P-61).

#### A.7.4 Error and exit-code formatting

```rust
#[cfg(windows)]
fn format_spawn_error(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(n) => {
            let full = io::Error::from_raw_os_error(n).to_string();         // "msg (os error n)"
            let msg = full.strip_suffix(&format!(" (os error {n})")).unwrap_or(&full);
            let msg = msg.trim_end_matches(|c: char| c <= ' ' || c == '.'); // cpy:errors.c:885
            format!("[WinError {n}] {msg}")                                  // + ": {py_repr(path)}" only for fs ops
        }
        None => e.to_string(), // e.g. Rust's own "batch file arguments are invalid" → see D-WIN-03
    }
}
#[cfg(windows)] fn display_exit_code(s: ExitStatus) -> i64 { i64::from(s.code().unwrap_or(1) as u32) }
#[cfg(unix)]    fn display_exit_code(s: ExitStatus) -> i64 { s.code().map(i64::from).unwrap_or_else(|| -i64::from(s.signal().unwrap_or(0))) }
```

- `ErrorKind` is `Uncategorized` for 193 (VW), so match on `raw_os_error()`, never on `kind()`.
- Hook-impl's final exit on Windows: `std::process::exit(combined as u32 as i32)` as the last expression of `main`, after every guard has been dropped (P-43). Elsewhere, return an `ExitCode`.

#### A.7.5 Spawning

- Always spawn the **absolute path** returned by the `find_executable` port (P-27), with `.stdin(Stdio::piped())`. Drop the stdin pipe immediately (A.4.1).
- Merge stdout and stderr with `std::io::pipe()` (07 §7.15).
- Shebang fallback (D-WIN-02), Windows only:
  - In `normalize_cmd`, after `parse_shebang`, if `interp.starts_with('/')` and `!is_file(interp)`, try `find_executable(basename(interp))`.
  - If found, replace `cmd[0]`. Otherwise keep pre-commit's error string.
  - Never apply this to an explicit `entry:` path, only to shebang-derived interpreters. This matches Git (`parse_interpreter` is used only for shebangs).
- Ctrl-C handler on Windows: required (P-47). Children share the console and receive CTRL_C themselves. Wait for them with a grace period, then restore the stash and exit 130.

#### A.7.6 Batch budget and the real command line

```rust
// parity budget (pc:xargs.py:154-164): computed only when cmd[0] ends with .bat/.cmd (ASCII case-insensitive)
let budget = 8192 - find_executable("cmd.exe", env)?.chars().count() - 4 - 1024;  // Python len() = code points; cmd.exe not found → pre-commit TypeError crash (exit 3); snp: hook error "Executable `cmd.exe` not found"
// after parity partitioning, verify each partition with Rust's real encoding:
// len = len("cmd.exe /e:ON /v:OFF /d /c \"") + 2 + utf16_len(script) + Σ(1 + bat_arg_len(a)) + 1
// bat_arg_len: port rs:sys/args/windows.rs:219-288 (quotes if needed: +2; each '%' +7; each '"' +1; backslashes before '"' doubled)
// if len > 8191: split that partition in halves (repeat) — D-WIN-09 (only for exotic names)
```

- For non-batch programs, the Windows command line is `make_command_line` (MSVCRT rules, comparable to `list2cmdline`). The 30 720 budget leaves 2 047 units of headroom for the quotes; keep the parity budget.
- Count lengths with `encode_utf16().count()`.

#### A.7.7 Environment handling on Windows

- Store child env overrides in a `Vec<(OsString, OsString)>`, keyed by **ASCII upper-case** for dedupe but keeping the first-seen spelling (04a §14.6).
- `no_git_env`: test `name.to_ascii_uppercase().starts_with("GIT_")` and the allow-list on upper-cased names (P-83).
- `env_remove("__PYVENV_LAUNCHER__")`, `env_remove("PYTHONEXECUTABLE")` on every child (P-84).

#### A.7.8 Files snp writes

- Hook script: LF, POSIX sh, `#!/bin/sh` without options (P-03), both hash markers (03 §20.3). `SNP_EXE` uses forward slashes on Windows.
- Rewrites of user files (`migrate-config`, `autoupdate`): preserve the existing EOL style byte-for-byte. Read as bytes, detect `\r\n`, and write back the same. Always UTF-8.
- Store metadata: LF.

#### A.7.9 Console output

- On Windows, if stdout is a console (`snp_sys` or `IsTerminal` on stdout, where an MSYS pty is fine because it is a pipe), write `from_utf8_lossy` chunks. Otherwise write raw bytes (P-53).
- Keep one `StdoutLock` and flush after each logical write (07 §7.5).

#### A.7.10 Git invocation

- Resolve `git` once:
  - Windows: `PATH` × `PATHEXT` via the port, giving `…\cmd\git.EXE`.
  - macOS: if the result is `/usr/bin/git`, call `xcrun --find git` once and use that path (P-87).
- For snp-owned repos, add `-c core.useBuiltinFSMonitor=false -c core.fsmonitor=false` (A.4.4).
- Never call `git rev-parse --show-toplevel` for path math. Use `--show-cdup` (P-64).

---

### A.8 Deviation register additions (append to `known-divergences.yaml`)

| ID | Deviation | Direction | Why |
|---|---|---|---|
| D-WIN-01 | The hook script is LF and POSIX sh on Windows (pre-commit: CRLF + bash arrays) | improvement | works with BusyBox `sh`; WSL-safe; independent of the MSYS2 `\r` patch |
| D-WIN-02 | Windows: a missing absolute shebang interpreter falls back to a `PATH` basename lookup (Git for Windows semantics); applies to hooks and legacy hooks | improvement | pre-commit fails or crashes (rc 1 / rc 3) on `#!/bin/sh` and `#!/bin/bash` |
| D-WIN-03 | Batch-file arguments are quoted safely; `%VAR%`, `&`, `^`, `|` are passed literally; CR/LF arguments → rc 1 + message (exact text: A.9 Q1) | safety | CVE-2024-24576 class; pre-commit is unsafe here |
| D-WIN-04 | LF instead of CRLF for `print()`/argparse-style outputs on Windows | consistency | byte-stable logs; harness `eol: insensitive` |
| D-WIN-05 | Child env names keep their original case (pre-commit upper-cases them in env copies) | neutral | Windows names are case-insensitive |
| D-WIN-06 | Cross-drive config or files: keep an absolute path instead of exit 3 `ValueError` | improvement | a crash with no value |
| D-WIN-07 | (Open) colors in mintty | – | parity = off |
| D-WIN-08 | A Cygwin/MSYS2 git detected on Windows → clear error (pre-commit: dead warning, then undefined behaviour) | improvement | – |
| D-WIN-09 | Batch partitions re-split when the real encoded line exceeds 8191 | improvement | avoids `The command line is too long.` |
| D-ENV-HASH | Hashed env-dir names for non-simple versions (A.4.6) | improvement | Windows `:` in versions |

---

### A.9 Open questions

1. **D-WIN-03 message.** What should snp print when a filename containing CR or LF must be passed to a `.bat`/`.cmd` hook? Proposal: per-partition output ``Executable `{cmd0}` cannot receive an argument containing a line break: {py_repr(arg)}``, rc 1, hook marked Failed. Alternative: skip such files with a warning.
2. **D-WIN-04 (LF vs CRLF).** Byte parity would require emitting `\r\n` for exactly the `print()`-based messages on Windows, and only when stdout is not a console. Is any downstream tool known to depend on that? The proposal is LF; revisit if W13 shows that users diff the outputs.
3. **`PYTHON_CPU_COUNT` parity** (CPython 3.13+, macOS and Windows only). Should snp honour it by default? It changes partitioning only on those OSes, and only for users who set a Python-specific variable. Proposal: ignore it by default. Add `--compat-python-env`, or simply document that snp honours `SNP_JOBS`.
4. **Colors in mintty (D-WIN-07).** Parity says no color, although mintty renders ANSI well. Should snp offer `SNP_COLOR=auto-msys`, or treat an MSYS pty as a terminal by default (a visible deviation)? Proposal: parity by default, and also honour `FORCE_COLOR`/`CLICOLOR_FORCE` as snp-only extensions (they are not pre-commit variables).
5. **`snp-sys` approval.** It needs about 40 lines of `unsafe` FFI (A.7.1). Decide this before implementing color. The fallback divergences are listed there.
6. **WSL support level.** Should `snp install` running under WSL in a repo on `/mnt/*` print a warning when it detects a CRLF hook installed from the Windows side (`.git/hooks/<t>` starting with `#!/bin/sh\r`)? Proposal: yes. Print `[WARNING] Hook script has CRLF line endings (installed from Windows?); reinstalled with LF`, and rewrite it, since it is "ours" by hash.
7. **Junction classification (P-69).** Should snp mirror Python exactly? That needs the reparse tag, which stable std does not expose (`windows_by_handle` is unstable), so it would need FFI. Proposal: classify `is_symlink() && is_dir()` as `directory` and accept the rare divergence for directory symlinks, which git never lists as files.
8. **Oracle predictions to confirm on real runners:**
   - W08 batch partition counts.
   - W10 hook-impl exit code under 3.12.x on Windows (gh-125842 backport status for 3.12).
   - W23 exact failure text.
   - W24 long-path behaviour.
   - W26 fsmonitor daemon behaviour on macOS 26.
   - The GHA Windows `core.autocrlf` value (DER: installer default `true`; verify via `platform-facts`).
