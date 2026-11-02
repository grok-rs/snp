# Platform support

## Targets

| Target | Support | Notes |
|---|---|---|
| Linux x86_64, arm64 (glibc) | tier 1 | the reference platform |
| Linux x86_64, arm64 (musl, static) | tier 1 for releases | any distribution, containers, WSL |
| macOS Apple silicon | tier 1 | |
| macOS Intel | tier 2 | built and tested while GitHub offers Intel runners |
| Windows x86_64, arm64 (MSVC, static CRT) | tier 1 | with [Git for Windows](https://gitforwindows.org) |
| Windows with Cygwin or MSYS2 git | unsupported | such a git prints POSIX paths (`/c/…`); snp stops with `snp does not support cygwin/msys2 git; use Git for Windows` |
| WSL 1/2 | supported as Linux | use the Linux snp with the Linux git inside WSL |

Every pull request runs the test suite on Linux x86_64 and arm64, macOS arm64 and Intel, and Windows x86_64 and arm64,
and runs the git-dependent tests against git 2.18.5, 2.30, 2.31, 2.34, 2.39, 2.43, 2.54 and the latest release (and
the latest Git for Windows).

## git versions

| git | Behaviour |
|---|---|
| ≥ 2.54 | everything, including config-based hooks (`snp install --git-config-hooks`) |
| 2.31 – 2.53 | everything except config-based hooks |
| 2.19 – 2.30 | one warning per run on stderr (`SNP_ALLOW_OLD_GIT=1` silences it); fallbacks where pre-commit crashes (`run --all-files`, pre-push of a new root, meta hooks: [D-CRASH-26](divergences.md#d-crash-26)) |
| < 2.19 | as 2.19 – 2.30, and a commit with unstaged changes is refused before anything is touched ([D-DATA-17](divergences.md#d-data-17)) |

## Windows

How hooks get started on Windows, and what that means:

| Context | Colour under `--color auto` | Notes |
|---|---|---|
| cmd.exe or PowerShell (console or Windows Terminal) → `git commit` | yes | git runs the hook script with its bundled `sh`, which runs `snp.exe`; Git's `usr\bin` and `mingw64\bin` are on `PATH` |
| Git Bash in mintty (the default Git Bash window) | no | mintty's terminal is a pipe for native programs; pre-commit shows no colour there either |
| Git Bash with ConPTY (`MSYS=enable_pcon`) or inside Windows Terminal | yes | |
| IDEs and GUI clients (VS Code, JetBrains, Visual Studio, GitHub Desktop, SourceTree, TortoiseGit, Fork) | no | their git may be MinGit or MinGit-BusyBox; snp's POSIX hook script works with BusyBox `ash` (pre-commit's bash script does not) |
| `snp run` directly from cmd or PowerShell | yes | only your `PATH` (no Git `usr\bin`): `#!/usr/bin/env bash` may find WSL's `bash` |
| GitHub Actions Windows runners | no | stdout and stderr are pipes |
| WSL git calling the Windows `snp.exe` | unsupported | hooks installed from WSL must call a Linux snp |

Differences from pre-commit on Windows (each one has its entry in the [DIVERGENCES](divergences.md) book):

* `script` hooks and legacy hooks whose shebang names an absolute interpreter that does not exist (`#!/bin/bash`) fall
  back to the interpreter's basename on `PATH`, as Git for Windows does; pre-commit fails with
  ``Executable `/bin/bash` not found`` ([D-PLAT-02](divergences.md#d-plat-02));
* file names with `&`, `^` or `%VAR%` reach `.cmd`/`.bat` hooks intact instead of being re-parsed by `cmd.exe`, and an
  argument containing a line break fails the hook ([D-PLAT-03](divergences.md#d-plat-03)); command lines of batch
  hooks are re-split when quoting makes them longer than 8191 characters ([D-PLAT-09](divergences.md#d-plat-09));
* a config or `--files` path on another drive than the repository does not crash
  ([D-PLAT-06](divergences.md#d-plat-06));
* a Cygwin or MSYS2 git is rejected with a clear error ([D-PLAT-08](divergences.md#d-plat-08));
* the CPU count differs only on hosts with more than 64 logical CPUs ([D-PLAT-10](divergences.md#d-plat-10)), and a
  directory junction is tagged `symlink` rather than `directory` ([D-PLAT-12](divergences.md#d-plat-12)).

Exit codes of crashed Windows processes are reported as unsigned values (`- exit code: 3221225477`), as pre-commit
does.

## macOS

* GUI git clients start hooks with launchd's minimal `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`). snp's hook script embeds
  the absolute path of snp, so commits work; for Homebrew installs the embedded path is `/opt/homebrew/bin/snp` (stable
  across `brew upgrade`), never the versioned `Cellar` path.
* `/tmp` is a symbolic link to `/private/tmp`; paths printed by snp match pre-commit's, which resolve it.
* The default file system is case-insensitive and git records `core.ignorecase=true`; file patterns (`files:`,
  `exclude:`) remain case-sensitive regular expressions, as in pre-commit.
* File names are kept as git reports them (NFC with `core.precomposeunicode=true`, git's default on macOS).

## Verified platform facts

Much of pre-commit's platform behaviour can only be predicted from its source on Linux. The `platform` CI workflow runs
`cargo xtask platform-facts` on every runner image: it records git's configuration, `PATHEXT`, the environment a
git-spawned program sees, Python's view of stdin, exit codes and line endings, file-system facts (case sensitivity,
Unicode normalisation, symbolic links, temp-dir resolution) and checks each prediction the design relies on. A
contradicted prediction is reported as an issue and corrected together with any behaviour that depends on it. The
reports are published as workflow artifacts.

## Manual release checklist

Some contexts cannot run in CI. Before each release they are checked by hand, with snp and with pre-commit 4.7.0 for
comparison:

* **mintty (Git Bash default):** `git commit` with a failing hook — no ANSI colour from either tool, the same lines.
* **ConPTY:** Git Bash with `MSYS=enable_pcon`, and Windows Terminal with cmd and PowerShell — colour from both tools.
* **Ctrl-C in mintty** during a hook with unstaged changes stashed: record the state for both tools (mintty sends no
  console event, so no clean-up is possible for either).
* **MinGit-BusyBox** (the `MinGit-*-busybox-64-bit.zip` some GUIs bundle): pre-commit's hook fails with an `ash` syntax
  error at `ARGS=(`; snp's POSIX script works.
* **WSL 2** with the repository on `/mnt/c`: install from Windows with each tool, then commit from WSL — pre-commit's
  script cannot run, snp's works when a Linux snp is on `PATH`; and the reverse direction.
* **IDEs:** VS Code (Git for Windows) and GitHub Desktop (dugite): a commit with a hook runs, without colour.
