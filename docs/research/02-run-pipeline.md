# 02 — The `run` execution pipeline of pre-commit (spec for snp)

Target reference: **pre-commit 4.6.2** (`refs/pre-commit`, commit c429238). Behaviour was also checked by
running **pre-commit 4.1.0** (`~/.local/bin/pre-commit`, Python 3.12.3, git 2.43.0, Linux, 22 CPUs) in
throwaway repos. The two versions differ only in the points listed in §20, and §20 says which version
each one follows.

Citation convention: `pre_commit/<file>.py:LINE` is relative to `refs/pre-commit/`.
`identify/identify.py:LINE` is relative to `refs/identify/identify/`.
"Observed" means the output was captured from 4.1.0 with `cat -A`/`xxd`.

---

## 0. TL;DR — the 15 facts that matter most

1. **All user-facing output goes to stdout**, including `[ERROR]`/`[WARNING]`/`[INFO]` log lines, the crash message and the `git diff` of `--show-diff-on-failure`. Only argparse usage errors (exit 2), `SystemExit("msg")` texts (hook-impl argument errors) and the hook script's "not found" line go to stderr.
2. **Color "auto" is decided by `isatty(stderr)`**, not stdout, plus `TERM != "dumb"` and Windows VT support (`pre_commit/color.py:84-100`). `PRE_COMMIT_COLOR` provides the default for `--color`. `NO_COLOR` is **not** honoured.
3. Status line width: `cols = max(80, maxwidth(hook names) + 3 + 19 + 1 + 7)`. Every status line is exactly `cols-1` display columns. Widths use **Python `unicodedata.east_asian_width`**: W/F count as 2, everything else (including A, combining marks and controls) counts as 1 (`run.py:36-57,238-250`).
4. Unstaged changes are stashed **as a binary patch file in the store directory** (`$PRE_COMMIT_HOME` or `~/.cache/pre-commit`/`$XDG_CACHE_HOME/pre-commit`), named `patch<unix_secs>-<pid>`. The worktree is reset with `git -c submodule.recurse=0 checkout -- .`, the hooks run, and the patch is re-applied. If it conflicts, hook changes are rolled back first. **The patch file is never deleted** (`staged_files_only.py:51-104`).
5. Stashing happens whenever `--all-files` and `--files` are both absent, **for every stage** (commit-msg, post-commit, pre-push with refs, …). Merge-conflict and unstaged-config checks are skipped when not stashing.
6. File-modification detection compares the **full stdout bytes of `git diff --no-ext-diff --no-textconv --ignore-submodules`** before and after each executed hook. Any difference means the hook failed, even with exit code 0.
7. Before partitioning, file arguments are **shuffled deterministically** (Python `random.Random().seed(1542676187)`, i.e. MT19937 `init_by_array([1542676187])` plus CPython's `shuffle`/`_randbelow`) unless `require_serial: true`. snp must reproduce this bit-exactly; the algorithm and test vectors are in §11.7.
8. Partitioning (`xargs.py:76-117`): `max_args = max(4, ceil(n/jobs))`, a byte-length budget (POSIX: `clamp(ARG_MAX-2048-envsize, 4096, 131072)`; Windows: 30720 UTF-16 units, or `8192-len(cmd.exe)-4-1024` for .bat/.cmd), and `jobs = 1 | 2 (TRAVIS) | cpu_count`. Partitions run in a thread pool of `min(#partitions, jobs)` threads. Outputs are concatenated **in partition order**. The exit code is the **one with the largest absolute value** (first wins on ties).
9. Hooks get **stdin = empty** (closed pipe, or `/dev/null` in color mode), **stdout+stderr merged into one stream**, cwd = repo root, and the full inherited environment plus `PRE_COMMIT=1` and the `PRE_COMMIT_*` variables in §6. With `--color=always` (or auto→true) on POSIX, **hooks run under a pty** with OPOST/ONLCR off.
10. `entry` is split with **Python `shlex.split` (POSIX mode, no comments)**, which is not POSIX-sh compatible (e.g. `"a\$b"` gives `a\$b`). `args` are appended verbatim, then the files. A leading `pre-commit hazmat` is rewritten to an internal invocation.
11. The executable is resolved via PATH (and PATHEXT), and then **its shebang is parsed and the interpreter is exec'd explicitly** (`parse_shebang.py:65-85`). Error texts that appear as hook output: ``Executable `X` not found``, ``… is a directory``, ``… is not executable``, plus Python `OSError` strings such as `[Errno 8] Exec format error: './x.sh'`.
12. Signal deaths are reported as **negative exit codes** (`- exit code: -9`).
13. Return codes: `run` → 0/1. argparse error → 2. FatalError → 1. Unexpected exception → 3. Ctrl-C → 130, and the stash is still restored. `hook-impl` returns `legacy_rc | run_rc` (bitwise OR).
14. `SKIP` holds comma-separated ids **or aliases**; entries are whitespace-stripped and empty ones dropped. Skipped hooks print `Skipped` (yellow) and are not installed. `fail_fast` stops after the **current** failing hook when the config-level flag, the hook-level flag or `--fail-fast` (4.4+) is set.
15. `git commit` hook path: the installed script runs `hook-impl --config=… --hook-type=… [--skip-on-missing-config] --hook-dir "$HERE" -- "$@"`. hook-impl validates the argument count per hook type, runs a `<type>.legacy` hook if one is present, and turns `pre-push` stdin into from/to refs, using only the **first** pushable ref.

---

## 1. End-to-end pipeline (process start → exit)

| # | Step | Source |
|---|------|--------|
| 1 | At import time, remove `__PYVENV_LAUNCHER__` and `PYTHONEXECUTABLE` from the process environment. Hooks therefore never see them. | `pre_commit/main.py:39,42` |
| 2 | At import time, compute the xargs max command length (POSIX: from `SC_ARG_MAX` and **the env size at import time**). | `pre_commit/xargs.py:49-58,137` |
| 3 | Parse argv. Empty argv becomes `['run']`. `help [cmd]` re-parses as `cmd --help`. | `main.py:367-375` |
| 4 | Enter `error_handler()` (crash log, exit codes), then `logging_handler(args.color)` (INFO+ to stdout). | `main.py:377` |
| 5 | `git.check_for_cygwin_mismatch()` runs on win32/cygwin only. | `git.py:210-230` |
| 6 | `Store()` creates the store dir + `README` + `db.db` (sqlite, under `.lock`) if they are missing. | `store.py:62-101` |
| 7 | For commands that need git (`run`, `hook-impl`, `try-repo`, `install`, …): run `_adjust_args_and_chdir`, then `store.mark_config_used(config)`. | `main.py:382-384,175-199` |
| 8 | Dispatch to `run(config, store, args)` or to `hook_impl(...)`, which builds a Namespace and calls `run`. | `main.py:400-429` |
| 9 | `run`: preflight checks (§5), then export env vars (§6). | `run.py:338-417` |
| 10 | If stashing, enter `staged_files_only(store.directory)` (§7). | `run.py:419-421` |
| 11 | `load_config` (cfgv validation + defaults), then `all_hooks` (clones every remote repo in the config), then filter by hook-id/alias and stage. | `run.py:423-429` |
| 12 | If a hook id was given and nothing matched, print `No hook with id …` and return 1 (the stash is restored). | `run.py:431-435` |
| 13 | `SKIP` parse; install environments for the non-skipped selected hooks. | `run.py:437-443` |
| 14 | `_run_hooks`: compute cols, select the files (§9), run the global filter, take the initial diff, run each hook in order (§11), check fail-fast, then show-diff-on-failure. | `run.py:282-322` |
| 15 | Leave the stash context: restore the patch, then re-add intent-to-add files. | `staged_files_only.py:84-98,42-45` |
| 16 | Return the code to `main`, which exits with it. | `main.py:454` |

---

## 2. CLI

### 2.1 Top level (`main.py:202-375`)

- `prog='pre-commit'`. Top-level options are only `-h/--help` and `-V/--version`. `--version` prints `pre-commit 4.6.2` to stdout and exits 0.
- **`--color` and `-c/--config` are sub-command options.** They must come after the sub-command: `pre-commit --color never run` is an argparse error.
- Sub-commands: `autoupdate, clean, gc, hazmat (4.5+), init-templatedir, install, install-hooks, migrate-config, run, sample-config, try-repo, uninstall, validate-config, validate-manifest, help, hook-impl` (hook-impl is hidden from the description list but appears in the choices).
- **No arguments ⇒ `run`** (`main.py:368-369`). `pre-commit` alone equals `pre-commit run`. Any other argv (for example `pre-commit -a`) is **not** rewritten.
- `help` → `parser.parse_args([help_cmd, '--help'])`. Without an argument it shows the top-level `--help`. Exit 0.
- Commands that do **not** chdir to the git root: `clean, gc, hazmat, init-templatedir, sample-config, validate-config, validate-manifest` (`main.py:44-47`).

### 2.2 `run` options (`main.py:50-54,64-172`; also used by `try-repo`)

| Flag(s) | dest | Type/default | Notes |
|---|---|---|---|
| `hook` (positional, `nargs='?'`) | `hook` | `None` | Matches `hook.id` **or** `hook.alias`. All matches run. |
| `--color {auto,always,never}` | `color` | **bool** after conversion. Default is the string `os.environ.get('PRE_COMMIT_COLOR','auto')`, which argparse also passes through `type=use_color`. | Values are case-sensitive. Invalid value or invalid env value → exit 2 with `argument --color: invalid use_color value: 'bogus'`. `PRE_COMMIT_COLOR=` (empty) is also invalid. |
| `-c, --config` | `config` | `.pre-commit-config.yaml` | See §3 for path handling. |
| `--verbose, -v` | `verbose` | `False` | |
| `--all-files, -a` | `all_files` | `False` | Mutually exclusive with `--files`. |
| `--files [FILES ...]` | `files` | `[]`, `nargs='*'`, action `store` | **Greedy**: `run --files a b myhook` treats `myhook` as a file (observed). `--` ends it. Repeating `--files` replaces the list (last wins). `--files` with zero values ⇒ `[]` ⇒ behaves exactly like no `--files` (staged files **and** stash), but it still conflicts with `-a`. |
| `--show-diff-on-failure` | | `False` | |
| `--fail-fast` | | `False` | **4.4.0+** (missing in 4.1.0). |
| `--hook-stage` | `hook_stage` | `'pre-commit'`; `type=transform_stage`, then `choices=STAGES` | Accepts the legacy names `commit`→`pre-commit`, `merge-commit`→`pre-merge-commit`, `push`→`pre-push`, which are not listed in help. STAGES order: `commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg, manual` (`clientlib.py:26-39,64-72`). |
| `--remote-branch` | | None | |
| `--local-branch` | | None | |
| `--from-ref, --source, -s` | `from_ref` | None | |
| `--to-ref, --origin, -o` | `to_ref` | None | **Naming trap:** the CLI alias `--origin` sets *to_ref*, but env `PRE_COMMIT_ORIGIN` is set from *from_ref* (§6). |
| `--pre-rebase-upstream` | | None | |
| `--pre-rebase-branch` | | None | |
| `--commit-msg-filename` | | None | |
| `--prepare-commit-message-source` | | None | |
| `--commit-object-name` | | None | |
| `--remote-name` | | None | |
| `--remote-url` | | None | |
| `--checkout-type` | | None | |
| `--is-squash-merge` | | None | |
| `--rewrite-command` | | None | |

argparse behaviours that users rely on and snp must emulate (all observed):

- **Unique-prefix abbreviation of long options**: `--all`, `--show-diff`, `--hook-st`, `--verb`, `--fi`, `--co` (ambiguous → error) all work. clap: `infer_long_args(true)`. Ambiguous example: `pre-commit run: error: ambiguous option: --co=never could match --color, --config, --commit-msg-filename, --commit-object-name`.
- Combined short flags: `-av`. Attached values: `-cfoo.yaml`, `--color=always`, `--color always`.
- Options and the positional may be interleaved: `run myhook -a`, `run -a myhook`.
- Usage errors print the usage block plus `pre-commit run: error: <msg>` to **stderr** and exit **2**. Observed messages:
  - `argument --files: not allowed with argument --all-files/-a`
  - `argument --hook-stage: invalid choice: 'bogus' (choose from 'commit-msg', 'post-checkout', 'post-commit', 'post-merge', 'post-rewrite', 'pre-commit', 'pre-merge-commit', 'pre-push', 'pre-rebase', 'prepare-commit-msg', 'manual')` (the quoting style depends on the Python version)
  - `argument --color: invalid use_color value: 'bogus'`

Full `run --help` text was captured (it matches 4.1.0 except for the 4.4 `--fail-fast` line `--fail-fast           Stop after the first failing hook.`). Help strings are verbatim in `main.py:64-172`. The `--hook-stage` help reads `The stage during which the hook is fired. One of <comma-separated STAGES>`. The `--color` help reads ``Whether to use color in output. Defaults to `auto`.`` (the `%(default)s` is the env-derived string).

### 2.3 `hook-impl` options (`main.py:356-365`, not intended for users)

`pre-commit hook-impl [--color C] [-c CONFIG] --hook-type T [--hook-dir DIR] [--skip-on-missing-config] [rest...]`

- `--hook-type` is **required** in 4.6.x. It has no `choices`, so an unknown value fails later with exit 3.
- `--hook-dir` is optional in 4.6.0+. `None` means "git ≥ 2.54 config-based hooks: no legacy hook".
- `rest` is `argparse.REMAINDER`. The code passes `args.rest[1:]`, which **unconditionally drops the first remainder token** (expected to be `--`). Observed: `hook-impl --hook-type=commit-msg --hook-dir D .git/COMMIT_EDITMSG` (no `--`) gives `hook-impl for commit-msg expected 1 argument but got 0: []`.

### 2.4 Exit codes (whole program)

| Code | When |
|---|---|
| 0 | All selected hooks passed or were skipped. No hooks selected. `post-checkout` with `_PRE_COMMIT_SKIP_POST_CHECKOUT` set. hook-impl: nothing to push. hook-impl: missing config with skip allowed (returns legacy rc). |
| 1 | Any hook failed or modified files. Preflight errors (§5). `No hook with id`. FatalError (`An error has occurred: …`), including InvalidConfigError, a missing config and not being in a git repo. `exit(1)` in repository.py (unknown hook id in a remote repo, language_version/additional_dependencies on env-less languages). hook-impl: missing config not allowed. hook-impl argument-count `SystemExit(str)`. hook-impl migration-mode bug message. |
| 2 | argparse usage error. |
| 3 | Unexpected exception (e.g. `ValueError: No closing quotation` from shlex, identify `ValueError` for a vanished file, `ArgumentTooLongError`, `CalledProcessError` from a checked git call). |
| 130 | `KeyboardInterrupt` (SIGINT). |
| other | hook-impl: `legacy_rc \| run_rc` (e.g. 5\|1 = 5). |

---

## 3. Startup: paths, chdir, store

`_adjust_args_and_chdir` (`main.py:175-199`), run only for git-requiring commands:

1. If `os.path.exists(config)` (relative to the **invocation cwd**), make it absolute.
2. For `run`/`try-repo`: `files = [abspath(f) for f in files]`. `commit_msg_filename` is also made absolute when given. These are **lexical** operations (`normpath`): `..` collapses and symlinks are not resolved.
3. `toplevel = git.get_root()`, then `os.chdir(toplevel)`.
4. `config = relpath(config)`. A config path that did not exist in step 1 stays relative and is now interpreted relative to the **root**. `files`/`commit_msg_filename` become `relpath(...)` relative to the root and may start with `../`. On Windows, `relpath` across drives raises ValueError, which gives exit 3.
   - Observed: from `sub/`, `--files ../b.txt ../a.txt ../b.txt nonexist ./../a.txt` → hook receives `b.txt a.txt b.txt a.txt`. **Duplicates and CLI order are kept; nonexistent files are dropped later by the Classifier.**

`git.get_root()` (`git.py:51-73`):
- `git rev-parse --show-cdup` → `root = abspath(stdout.strip())`. An empty string means the cwd. `--show-cdup` is used instead of `--show-toplevel` to keep Windows SUBST drives.
- `git rev-parse --is-inside-git-dir` must print `false`, otherwise FatalError:
  `git toplevel unexpectedly empty! make sure you are not inside the `.git` directory of your repository.`
- If either git call fails (or git is missing): FatalError `git failed. Is it installed, and are you in a Git repository directory?`
- Both are printed as `An error has occurred: FatalError: <msg>` + `Check the log at …`, exit 1 (observed).

Store directory (`store.py:25-36`): `realpath($PRE_COMMIT_HOME or join($XDG_CACHE_HOME or ~/.cache, 'pre-commit'))`. Empty env values count as unset (`or`). `store.mark_config_used(config)` inserts `realpath(config)` into the sqlite `configs` table if the file exists and the store is writable (`store.py:225-235`). This is not part of the run semantics, but the gc topic depends on it.

---

## 4. Color

- `use_color(setting)` (`color.py:84-100`): `always` → True. `never` → False. `auto` → `sys.stderr.isatty() and terminal_supports_color and os.getenv('TERM') != 'dumb'`. Any other value raises ValueError, which argparse turns into an error.
  - **stderr**, not stdout: observed `pre-commit run > file` in a pty still writes ANSI codes into the file, and `2>/dev/null` disables color even when stdout is a tty.
  - An unset `TERM` passes. Only the exact value `dumb` disables.
  - Windows: `terminal_supports_color` is computed at import by enabling `ENABLE_VIRTUAL_TERMINAL_PROCESSING` on the **stderr** console handle. If that fails it is False. On other platforms it is always True (`color.py:7-57`).
- ANSI constants (`color.py:59-64`), exact bytes:

| Name | Bytes | Used for |
|---|---|---|
| RED | `\x1b[41m` (red **background**) | `Failed`, `[ERROR]` |
| GREEN | `\x1b[42m` | `Passed` |
| YELLOW | `\x1b[43;30m` | `Skipped` (SKIP env), `[WARNING]` |
| TURQUOISE | `\x1b[46;30m` | `Skipped` (no files) |
| SUBTLE | `\x1b[2m` | `- hook id:` / `- duration:` / `- exit code:` / `- files were modified…` |
| NORMAL | `\x1b[m` | reset after each colored span |

- `format_color(text, color, on)` = `f'{color}{text}{NORMAL}'` if on, else `text`. **Even an empty color string appends NORMAL**, so with color on, `[INFO]` prints as `[INFO]\x1b[m` (observed).
- The `color` bool is also passed to hooks (pty mode §11.9), to docker (`--tty`), and to `git diff --color=always|never` for show-diff.

---

## 5. `run()` preflight, in exact order (`run.py:338-373`)

`stash = not args.all_files and not args.files` (`run.py:344`). An empty `files` list counts as false.

| Order | Condition | Output (stdout) | Return |
|---|---|---|---|
| 1 | `stash` and `git ls-files --unmerged` stdout non-blank (checked call: a git failure gives exit 3) | `[ERROR] Unmerged files.  Resolve before committing.` (two spaces) | 1 |
| 2 | `bool(from_ref) != bool(to_ref)` (empty strings count as unset) | `[ERROR] Specify both --from-ref and --to-ref.` | 1 |
| 3 | `stash` and `git diff --quiet --no-ext-diff <config>` exits **exactly 1** (no `--` separator; other codes such as 128 for a missing or outside path are ignored) | `[ERROR] Your pre-commit configuration is unstaged.` + `\n` + ``\`git add <config>\` to fix this.`` (config = root-relative path) | 1 |
| 4 | stage ∈ {prepare-commit-msg, commit-msg} and no `commit_msg_filename` | ``[ERROR] `--commit-msg-filename` is required for `--hook-stage commit-msg` `` | 1 |
| 5 | stage == post-checkout and env `_PRE_COMMIT_SKIP_POST_CHECKOUT` is non-empty | (nothing) | 0 |

With color on, `[ERROR]` becomes `\x1b[41m[ERROR]\x1b[m`. Each message is one `write_line`, so the embedded `\n` breaks the line with no prefix on the second line.

A missing config file is not checked here. It surfaces in `load_config` as `An error has occurred: InvalidConfigError: \n=====> .pre-commit-config.yaml is not a file` (note the trailing space before the newline). This happens **inside** the stash, so stash/restore messages surround it.

---

## 6. Environment variables

### 6.1 Set by `run` for hooks (`run.py:375-417`)

They are set on `os.environ`, i.e. the whole process. They are inherited by every later subprocess, including git, and never unset. Each is set only if the value is truthy (non-empty).

| Variable | Value | Condition |
|---|---|---|
| `PRE_COMMIT` | `1` | always (after the preflight checks) |
| `PRE_COMMIT_COMMIT_MSG_SOURCE` | `--prepare-commit-message-source` | if set |
| `PRE_COMMIT_COMMIT_OBJECT_NAME` | `--commit-object-name` | if set |
| `PRE_COMMIT_ORIGIN` (legacy) | **from_ref** | from_ref and to_ref both set |
| `PRE_COMMIT_SOURCE` (legacy) | **to_ref** | same |
| `PRE_COMMIT_FROM_REF` | from_ref | same |
| `PRE_COMMIT_TO_REF` | to_ref | same |
| `PRE_COMMIT_PRE_REBASE_UPSTREAM` | upstream | **only if upstream AND branch are both set**. A one-argument pre-rebase therefore exports neither (observed). |
| `PRE_COMMIT_PRE_REBASE_BRANCH` | branch | same |
| `PRE_COMMIT_LOCAL_BRANCH` | local_branch | **all four** of remote_name, remote_url, remote_branch, local_branch are set |
| `PRE_COMMIT_REMOTE_BRANCH` | remote_branch | same |
| `PRE_COMMIT_REMOTE_NAME` | remote_name | same |
| `PRE_COMMIT_REMOTE_URL` | remote_url | same |
| `PRE_COMMIT_CHECKOUT_TYPE` | `--checkout-type` (`0`/`1`) | if set |
| `PRE_COMMIT_IS_SQUASH_MERGE` | `--is-squash-merge` (`0`/`1`; `"0"` is truthy, so it is set) | if set |
| `PRE_COMMIT_REWRITE_COMMAND` | `--rewrite-command` (`amend`/`rebase`) | if set |

Other variables set for subprocesses:
- `_PRE_COMMIT_SKIP_POST_CHECKOUT=1`: only in the env of the stash `git checkout` calls (`staged_files_only.py:78`).
- `PRE_COMMIT_RUNNING_LEGACY=1`: only while running `<hook>.legacy` (`hook_impl.py:44`).
- Language `in_env` contexts (python/node/…) further modify `os.environ` (PATH etc.) around `run_hook` and restore it afterwards (`envcontext.py:32-62`). See the languages spec.
- Git-provided variables (e.g. `GIT_INDEX_FILE=.git/index`, `GIT_PREFIX=`, `GIT_EXEC_PATH`, `GIT_AUTHOR_*`, `GIT_EDITOR=:` during commit) are **inherited unchanged** by hooks and by pre-commit's own git calls. `no_git_env()` is used only for cloning, never in the run pipeline.

Observed env during `git commit` → prepare-commit-msg: `PRE_COMMIT=1`, `PRE_COMMIT_COMMIT_MSG_SOURCE=message`. During `git push` to an existing branch: `PRE_COMMIT_FROM_REF/ORIGIN=<remote sha>`, `PRE_COMMIT_TO_REF/SOURCE=<local sha>`, `PRE_COMMIT_LOCAL_BRANCH=refs/heads/main`, `PRE_COMMIT_REMOTE_BRANCH=refs/heads/main`, `PRE_COMMIT_REMOTE_NAME=origin`, `PRE_COMMIT_REMOTE_URL=../remote6.git`. During `git checkout -b feat`: `PRE_COMMIT_CHECKOUT_TYPE=1` plus FROM/TO/ORIGIN/SOURCE = old/new HEAD.

### 6.2 Read by the run pipeline

| Variable | Semantics | Source |
|---|---|---|
| `SKIP` | comma list. `{s.strip() for s in SKIP.split(',') if s.strip()}`. Python `str.strip()` strips Unicode whitespace. | `run.py:129-131` |
| `PRE_COMMIT_COLOR` | default for `--color` (must be auto/always/never) | `color.py:105` |
| `TERM` | `dumb` disables auto color | `color.py:98` |
| `PRE_COMMIT_HOME`, `XDG_CACHE_HOME`, `HOME` | store dir (patches, crash log) | `store.py:32-36` |
| `PRE_COMMIT_NO_CONCURRENCY` | **presence** (even empty) ⇒ jobs=1 | `lang_base.py:135-143` |
| `TRAVIS` | presence ⇒ jobs=2 | same |
| `_PRE_COMMIT_SKIP_POST_CHECKOUT` | non-empty ⇒ post-checkout run returns 0 immediately | `run.py:369-373` |
| `PRE_COMMIT_ALLOW_NO_CONFIG` | hook-impl only. Non-empty (`os.getenv` truthiness, so `"0"` counts) ⇒ a missing config is OK. | `hook_impl.py:55` |
| `PRE_COMMIT_RUNNING_LEGACY` | hook-impl only. Non-empty ⇒ the "migration mode" bug exit | `hook_impl.py:22-29` |
| `PATH`, `PATHEXT` | executable resolution (§11.5) | `parse_shebang.py:22-43` |
| `__PYVENV_LAUNCHER__`, `PYTHONEXECUTABLE` | **removed** at startup | `main.py:39,42` |

---

## 7. `staged_files_only` (stash unstaged changes) — exact algorithm

Entered only if `stash` (§5). The context order is `with _intent_to_add_cleared(), _unstaged_changes_cleared(patch_dir)`: intent-to-add files are cleared **first** and restored **last** (`staged_files_only.py:107-113`). `patch_dir = store.directory`.

### 7.1 Intent-to-add (`staged_files_only.py:35-47`, `git.py:146-151`)

1. `git diff --no-ext-diff --ignore-submodules --diff-filter=A --name-only -z` (checked; worktree vs index; `git add -N` files show up as `A`). Parse it with `zsplit`.
2. If the list is non-empty:
   - `[WARNING] Unstaged intent-to-add files detected.`
   - `xargs(('git','rm','--cached','--'), files)`: length-partitioned, serial, **exit code ignored**.
   - yield.
   - finally: `xargs(('git','add','--intent-to-add','--'), files)`, exit code ignored.
3. Otherwise yield. The ITA files become untracked while hooks run. They are therefore neither passed to hooks nor included in the patch (observed: no "Unstaged files detected" for an ITA-only change).

### 7.2 Unstaged changes (`staged_files_only.py:50-104`)

1. `tree = git write-tree` (checked → exit 3 on failure, e.g. an unmerged index, which is normally prevented by §5).
2. `git diff-index --ignore-submodules --binary --exit-code --no-color --no-ext-diff <tree> --` (unchecked). This compares tree = index with the worktree, so it captures only unstaged changes to tracked files. Untracked files are ignored.
3. Branch on the result:
   - `rc == 0` ⇒ nothing to do. yield.
   - `rc == 1` and `stdout.strip()` empty ⇒ yield (CRLF/autocrlf quirk: git reports a difference but prints no diff).
   - `rc == 1` and stdout non-empty ⇒ stash:
     1. `patch_filename = join(patch_dir, f'patch{int(time.time())}-{os.getpid()}')`, e.g. `/home/u/.cache/pre-commit/patch1791403647-1780651`. The path is absolute because the store is a realpath.
     2. `[WARNING] Unstaged files detected.`
     3. `[INFO] Stashing unstaged files to <patch_filename>.` (with a trailing period).
     4. `os.makedirs(patch_dir, exist_ok=True)`. Write the raw diff bytes to the patch file in `'wb'` mode (no fsync, no O_EXCL).
     5. env = os.environ + `_PRE_COMMIT_SKIP_POST_CHECKOUT=1`.
     6. try: `git -c submodule.recurse=0 checkout -- .` (checked, with the env above), then **yield** (hooks run).
     7. finally (also on exceptions and KeyboardInterrupt, and also if the checkout itself failed):
        - `_git_apply(patch)`: `git apply --whitespace=nowarn <patch>`. On failure, retry `git -c core.autocrlf=false apply --whitespace=nowarn <patch>`. Both use captured output (silent).
        - If both fail: `[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...`, then `git -c submodule.recurse=0 checkout -- .` (same env; discards ALL hook modifications to tracked files), then `_git_apply` again. If that also fails, the CalledProcessError propagates (exit 3) and the patch stays on disk.
        - `[INFO] Restored changes from <patch_filename>.`
     8. **The patch file is never removed** (observed: `patch*` files accumulate in the store; `pre-commit clean` removes the whole store).
   - Any other rc ⇒ `FatalError('pre-commit failed to diff -- perhaps due to permissions?\n\n' + str(CalledProcessError(rc, diff_cmd, b'', stderr)))`. Printed as `An error has occurred: FatalError: pre-commit failed to diff -- perhaps due to permissions?` + blank line + `command: ('git', 'diff-index', '--ignore-submodules', '--binary', '--exit-code', '--no-color', '--no-ext-diff', '<tree>', '--')` / `return code: N` / `stdout: (none)` / `stderr:` + 4-space-indented stderr (`util.py:63-75`).

Observed conflict transcript (no color):
```
[WARNING] Unstaged files detected.
[INFO] Stashing unstaged files to /…/home2/patch1791403656-1781218.
fixer....................................................................Failed
- hook id: fixer
- files were modified by this hook
[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...
[INFO] Restored changes from /…/home2/patch1791403656-1781218.
```
After a conflict the index still holds the user's staged content, the worktree holds the user's original unstaged content, and the hook's fixes are lost. In the non-conflict case the hook's fixes remain as unstaged modifications on top of the restored changes (observed `MM a.txt`).

Ctrl-C while hooks run (observed): the start line is already printed without a newline, so the restore message lands on the same line:
```
sleepy...................................................................[INFO] Restored changes from /…/patch1791403833-1793278.
Interrupted (^C): KeyboardInterrupt: 
Check the log at /…/pre-commit.log
```
Exit 130.

Interaction notes:
- The config is loaded **inside** the stash, so the staged config version is used (an unstaged config is rejected earlier anyway).
- A config deleted in the worktree but still in the index: `git diff --quiet <path>` → 128 (ambiguous argument), so no error is raised. The deletion is stashed, the checkout restores the file, and the run proceeds (observed).
- The stash also applies to commit-msg, prepare-commit-msg, post-commit, post-checkout, post-merge, post-rewrite, pre-rebase, pre-merge-commit and pre-push with from/to refs. It does **not** apply to pre-push "push everything" (`all_files=True`).

---

## 8. Hook list construction and selection (`run.py:423-443`, `repository.py:109-237`)

- `load_config(config)`: cfgv validation + defaults. Errors raise `InvalidConfigError` (FatalError → exit 1), formatted as `An error has occurred: InvalidConfigError: \n==> File X\n==> At Config()\n…\n=====> msg` (observed; see the config spec).
- `all_hooks(config, store)`: iterates `repos` in config order and their `hooks` in order, and **clones/initialises every remote repo**, even if only one hook id was requested (`[INFO] Initializing environment for <repo>.`). For `local`/`meta`, env-less languages use `Prefix(os.getcwd())` = repo root (`repository.py:152-160`).
- Per hook: `language_version` 'default' → `default_language_version[lang]` → the language default. `stages` empty → `default_stages` (whose default is **all STAGES, including `manual`**). Hooks without `stages` therefore run on `--hook-stage manual` (observed).
- Selection: `[h for h in all_hooks if (not args.hook or h.id == args.hook or h.alias == args.hook) if args.hook_stage in h.stages]`. Order is preserved and duplicates (the same id twice) all run.
- If `args.hook` is set and nothing matched: `output.write_line(f'No hook with id `{hook}` in stage `{stage}`')`. This is plain text with **no `[ERROR]` prefix**. `stage` is the transformed name (e.g. `pre-commit` for `--hook-stage commit`). Return 1.
- `skips = _get_skips(environ)`. `to_install = [h for h in hooks if h.id not in skips and h.alias not in skips]`, then `install_hook_envs(to_install, store)`. Envs are installed even for hooks that will later have no files. Skipped hooks are not installed.
- `_compute_cols(hooks)` uses **all selected hooks**, including skipped ones.

---

## 9. File selection (`run.py:253-271`, `git.py`)

First matching rule wins:

| Priority | Condition | Files | Git command |
|---|---|---|---|
| 1 | stage ∈ {post-checkout, post-commit, post-merge, post-rewrite, pre-rebase} | `()` | — |
| 2 | stage ∈ {prepare-commit-msg, commit-msg} | `(commit_msg_filename,)`, e.g. `.git/COMMIT_EDITMSG` | — |
| 3 | from_ref and to_ref | changed files | `git diff --name-only --no-ext-diff -z <from>...<to>` (checked). On CalledProcessError retry `git diff --name-only --no-ext-diff -z <from>..<to>` (no merge base). **No diff-filter**, so deleted paths appear and are dropped by `lexists`. |
| 4 | `files` non-empty | `args.files` (order and duplicates kept) | — |
| 5 | `all_files` | all tracked | `git ls-files -z --deduplicate` (4.6.1+; 4.1.0 has no `--deduplicate`, so unmerged paths appear up to 3 times). Includes index entries whose worktree file is deleted (dropped by lexists) and gitlinks (submodule dirs). |
| 6 | in merge conflict | conflicted ∪ merge-diff | see 9.1 |
| 7 | default | staged | `git diff --staged --name-only --no-ext-diff -z --diff-filter=ACMRTUXB` (excludes D only). Works on an unborn branch. |

Notes:
- Stages 1–2 ignore `--files/--all-files` for **selection**, but those flags still disable stashing.
- `pre-push`, `pre-merge-commit` and `manual` use rules 3–7.
- `zsplit(s)`: `s.strip('\0')`, then split on `\0`. Empty ⇒ `[]` (`git.py:19-24`).
- Output is decoded as **strict UTF-8** (`cmd_output` → `.decode()`). A non-UTF-8 filename causes UnicodeDecodeError → exit 3.

### 9.1 Merge-conflict mode (`git.py:96-132`)

- `is_in_merge_conflict()`: `git_dir = git rev-parse --git-dir` (normalised and joined with `.`). True iff both `<git_dir>/MERGE_MSG` and `<git_dir>/MERGE_HEAD` exist. It is evaluated only after rules 1–5 fail.
- `get_conflicted_files()`:
  1. `[INFO] Checking merge-conflict files only.` (printed before any hook line).
  2. Read `MERGE_MSG` bytes. Take lines starting with `\t` or `#\t`, apply `.lstrip(b'#').strip().decode()`. Git writes `# Conflicts:\n#\tc.txt`.
  3. `tree = git write-tree`. `git diff --name-only --no-ext-diff -z -m <tree> HEAD MERGE_HEAD --` (4.2.0+ adds `--` for a file named `HEAD`). This is a combined diff, so it lists paths that differ from **both** parents (conflict-resolved files, evil merges). Observed: a cleanly merged file changed on one side is **not** listed.
  4. Return a Python `set` union. **Iteration order is hash-randomised per process**; the shuffle step reorders it anyway.
- Unresolved conflicts (unmerged index entries) are rejected earlier by the "Unmerged files" check when stashing.

---

## 10. Filtering — `Classifier` (`run.py:60-126`)

1. `Classifier.from_config(filenames, config.files, config.exclude)`:
   - **Windows only** (`os.altsep == '/' and os.sep == '\\'`): replace every `\` in each filename with `/`. On POSIX a backslash is a literal filename character.
   - Global include/exclude: keep `f` if `re.search(include, f)` and not `re.search(exclude, f)`. Defaults are `files: ''` (matches all) and `exclude: '^$'` (matches nothing).
   - `Classifier.__init__`: keep only `os.path.lexists(f)`. Broken symlinks are kept; deleted files, unexpanded names and nonexistent `--files` entries are dropped. This happens **once**, before any hook runs.
2. Per hook, `filenames_for_hook(hook)`:
   - `filter_by_include_exclude(self.filenames, hook.files, hook.exclude)` (same `re.search` semantics; defaults `''` and `'^$'`).
   - `by_types(names, types, types_or, exclude_types)`: keep `f` iff `tags ⊇ types` **and** (`types_or` empty **or** `tags ∩ types_or ≠ ∅`) **and** `tags ∩ exclude_types = ∅`. Defaults: `types: [file]`, `types_or: []`, `exclude_types: []`.
   - Order is preserved and duplicates are preserved.
3. Tags come from `identify.tags_from_path(f)`, memoised per filename for the whole run (`functools.cache` on the method). The cache is **lazy**: tags are computed only when a hook's include/exclude passes the file. Consequences:
   - `lstat` decides the type: directory ⇒ `{directory}`, symlink ⇒ `{symlink}` (even if the target is a regular file), socket ⇒ `{socket}`. Otherwise `{file, executable|non-executable, …ext/name tags…, text|binary}`. The executable bit uses `os.access(path, X_OK)` (on Windows every existing file counts as executable). Shebang tags are read only if the file is executable and the filename gives no tags. text/binary is decided by the first 1024 bytes (`identify/identify.py:40-83,121-139`).
   - Observed with default `types: [file]`: symlinks (valid or broken), submodule gitlinks (directories) and directories are **not passed**. `types: [symlink]` passes `broken link`. `types: [directory]` passes `subm`. `types: []` passes everything that `lexists`.
   - If an earlier hook **deletes** a file whose tags were not yet computed, a later hook's classification raises `ValueError('<f> does not exist.')`, giving a crash with exit 3. Tags are never refreshed after hooks modify files (e.g. a chmod).
4. Regex dialect: **Python `re`**, `search` (unanchored), on the `/`-normalised relative path. Patterns were validated at config load. See the config spec for Rust regex translation (`\Z`, lookarounds, `$` before a final `\n`, etc.).
5. The selected file list is computed for **every** hook, including skipped ones, but only used if the hook runs (`run.py:151`).

---

## 11. Hook execution

### 11.1 `_run_single_hook` decision table (`run.py:142-235`)

| Case | Printed status line | Hook runs? | diff_after | retcode |
|---|---|---|---|---|
| `hook.id in skips or hook.alias in skips` | `_full_msg(name, end_msg='Skipped', color=YELLOW)` | no | = diff_before (no git call) | 0 |
| `not filenames and not hook.always_run` | `_full_msg(name, postfix='(no files to check)', end_msg='Skipped', color=TURQUOISE)` | no | = diff_before | 0 |
| otherwise | `_start_msg(name)` (dots, no newline, **flushed before running**), then `Passed`/`Failed` + `\n` | yes | `_get_diff()` | from the hook |

Running a hook:
1. `output.write(_start_msg(start=hook.name, end_len=6, cols=cols))`.
2. If `not hook.pass_filenames`: `filenames = ()`. This happens **after** the no-files check, so `pass_filenames: false` still needs matching files unless `always_run: true`.
3. `time_before = time.monotonic()`.
4. `with language.in_env(prefix, language_version): retcode, out = language.run_hook(prefix, entry, args, filenames, is_local=hook.src=='local', require_serial=..., color=use_color)`.
5. `duration = round(time.monotonic() - time_before, 2) or 0`. This includes the in_env setup and all partitions, and excludes the git diff.
6. `diff_after = _get_diff()`. `files_modified = diff_before != diff_after`.
7. Status is `Failed` (RED) if `retcode or files_modified`, else `Passed` (GREEN), written with `write_line`.

Details block (`run.py:217-233`), printed iff `verbose or hook.verbose or retcode or files_modified` (this includes skipped hooks when verbose):
1. `- hook id: {id}` (SUBTLE)
2. `- duration: {d}s` (SUBTLE), only if (`verbose or hook.verbose`) and the hook actually ran
3. `- exit code: {retcode}` (SUBTLE), if retcode ≠ 0. Can be negative.
4. `- files were modified by this hook` (SUBTLE), if files_modified
5. If `out.strip()` is non-empty: an empty line, then `out.strip()` + `\n` (also **appended** to `hook.log_file` if set), then an empty line.

Return value: `(files_modified or bool(retcode), diff_after)`. `retval |= current` gives the final code 0/1.

### 11.2 `_run_hooks` loop (`run.py:282-322`)

```
cols = _compute_cols(hooks)
classifier = Classifier.from_config(_all_filenames(args), config.files, config.exclude)
retval = 0
prior_diff = _get_diff()             # once, before the first hook (even if all are skipped)
for hook in hooks:                   # config order
    cur, prior_diff = _run_single_hook(classifier, hook, skips, cols, prior_diff, verbose, color)
    retval |= cur
    if cur and (config.fail_fast or hook.fail_fast or args.fail_fast):
        break                        # remaining hooks print nothing at all
if retval and args.show_diff_on_failure and prior_diff:   # prior_diff = diff after last executed hook
    ... (§15)
return retval
```
Per-hook `fail_fast` is evaluated on the hook that just failed. Earlier failures from non-fail-fast hooks do not stop later hooks (3.7.0 semantics).

### 11.3 `entry` + `args` → argv (`lang_base.py:174-178`)

`cmd = shlex.split(entry)`. If `cmd[:2] == ['pre-commit','hazmat']`, replace them with `[sys.executable, '-m', 'pre_commit.commands.hazmat']` (snp: `[current_exe, 'hazmat']`). Then `(*cmd, *hook.args)`. `args` are appended as-is (no splitting, no globbing) and the filenames go last. No `--` is inserted. An empty `entry` gives `cmd == ()`, then IndexError in `normexe`, exit 3.

**Python `shlex.split(s)` semantics** (posix=True, whitespace_split=True, comments disabled), which snp must implement exactly:
- Whitespace separators: space, `\t`, `\r`, `\n`.
- Outside quotes, `\` escapes the next character, whatever it is (including newline and whitespace). A `\` at end of input raises `ValueError("No escaped character")`.
- `'…'`: everything is literal (a backslash is literal; no escaping of `'`).
- `"…"`: a backslash escapes **only `"` and `\`**. Before any other char the backslash is **kept**: `"a\$b"` → `a\$b`, `"a\b"` → `a\b`, `` "a\`b" `` → `` a\`b ``, `"a\<newline>b"` → `a\<newline>b`. This **differs from POSIX sh and from the `shlex` crate**.
- Quoted segments concatenate with adjacent text: `a"b c"d` → `ab cd`.
- Empty quotes create an empty token: `''` → `['']`, `echo "" x` → `['echo','','x']`.
- `#` is an ordinary character: `a#b` → `a#b`, `#x` → `#x`.
- An unterminated quote raises `ValueError("No closing quotation")`. Observed: the status line was already partly printed, then `An unexpected error has occurred: ValueError: No closing quotation`, exit 3 (after the stash restore).

Test table (Python results):

| input | tokens |
|---|---|
| `a\ b` | `[a b]` |
| `a\$b` | `[a$b]` |
| `"a\$b"` | `[a\$b]` |
| `"a\"b"` | `[a"b]` |
| `"a\\b"` | `[a\b]` |
| `'a\'b` | `[a\b]` |
| `"it's"` | `[it's]` |
| `'say "hi"'` | `[say "hi"]` |
| `a"b c"d` | `[ab cd]` |
| `a\<LF>b` | `[a<LF>b]` |
| `a<TAB>b<CR><LF>c` | `[a, b, c]` |
| `x \` | ValueError: No escaped character |
| `"abc` | ValueError: No closing quotation |

### 11.4 Per-language `run_hook` (only the run-relevant parts; see the languages spec)

| language | argv / behaviour |
|---|---|
| `unsupported` (= `system`, translated at config load in 4.4+), `python`, `node`, `golang`, … (most) | `basic_run_hook` → `run_xargs(hook_cmd(entry,args), files, require_serial, color)` |
| `unsupported_script` (= `script`) | `cmd = hook_cmd(entry,args)`, then `cmd[0] = prefix.path(cmd[0])` (`normpath(join(repo_dir, cmd[0]))`; absolute for local hooks: the root). Then `run_xargs`. Observed `$0` = `/abs/repo/scripts/s.sh`. |
| `fail` | No process. `retcode=1`, `out = f'{entry}\n\n'.encode() + b'\n'.join(files) + b'\n'`. Observed: `this is the fail message` / empty line / `a.txt`. |
| `pygrep` | `xargs((sys.executable,'-m','pre_commit.languages.pygrep',*args,entry), files, color=color)`. **No shuffle, target_concurrency=1** (one process per length partition). Output formats: `{file}:{lineno}:{line without trailing \r\n}`; multiline `{file}:{lineno}:{full first line}\n{rest of match}`; negate prints `{file}`. Exit 1 if any match. snp should implement this in-process (see the languages spec). |
| `docker` | Rebuilds the image on every run (`docker build --tag pre-commit-<md5> --label PRE_COMMIT .`), then `docker run --rm [--tty] [-u uid:gid] -v <cwd>:/src:rw,Z --workdir /src --entrypoint <exe> <tag> <rest…>` via `run_xargs`. |
| `docker_image` | `docker run … (same prefix) <entry tokens…>` via `run_xargs`. |

### 11.5 Executable resolution — `parse_shebang.normalize_cmd` (`parse_shebang.py:15-85`)

Applied once per `xargs()` call (not per partition) to `cmd`:
1. `exe = normexe(cmd[0])`:
   - If cmd[0] contains no `os.sep` (and, on Windows, no `/`): `find_executable`. This applies `normpath`. Then for each dir in `PATH.split(os.pathsep)` (an unset PATH gives `['']`; an empty component means the cwd = repo root), and for each candidate name (`exe+ext` for each `ext in PATHEXT.split(pathsep)` if PATHEXT is set, then plain `exe`), return the first `join(dir,name)` that `isfile` and `os.access(X_OK)`. If none: error ``Executable `{orig}` not found``.
   - Otherwise (it contains a separator): `isdir` → ``Executable `{orig}` is a directory``. Not `isfile` → ``… not found``. Not X_OK → ``… is not executable`` (never on Windows). Else use it as-is (relative to the cwd = root).
2. `cmd = parse_filename(exe) + (exe,) + cmd[1:]`. `parse_filename`: if the file exists, `identify.parse_shebang_from_file(exe)`, which returns `()` unless the file is X_OK and starts with `#!`. The first line must be UTF-8 and printable ASCII only, otherwise `()`. Then shlex-split, falling back to `.split()`. A leading `/usr/bin/env -S` or `/usr/bin/env` is dropped. `nix-shell` uses the `-i` arg from subsequent `#!` lines (`identify/identify.py:239-268`).
3. `exe = normexe(cmd[0])` again (the interpreter goes through PATH lookup, with the same error messages, e.g. ``Executable `python3` not found``).
4. Result: `(exe, *cmd[1:])`. Observed: `#!/usr/bin/env -S sh -e` → runs `/usr/bin/sh -e ./interp.sh files…`.

`ExecutableNotFoundError.to_output()` = `(1, msg.encode(), None)`, with **no trailing newline**. In `xargs()` it is returned **once** for the whole hook, before partitioning (`xargs.py:149-152`).

### 11.6 xargs partitioning (`xargs.py`)

`run_xargs(cmd, files, require_serial, color)` (`lang_base.py:156-171`):
- `require_serial` ⇒ `jobs = 1`, **no shuffle**.
- Otherwise `files = _shuffled(files)` and `jobs = target_concurrency()`: `1` if `PRE_COMMIT_NO_CONCURRENCY` is in the env; else `2` if `TRAVIS` is in the env; else `cpu_count()` = `len(os.sched_getaffinity(0))` (Linux), falling back to `multiprocessing.cpu_count()` (macOS/Windows), falling back to 1.

`xargs(cmd, varargs, color, target_concurrency, _max_length)`:
1. `cmd = normalize_cmd(cmd)`. On ExecutableNotFoundError return `(1, b'Executable … not found')`.
2. Windows and `cmd[0].lower().endswith(('.bat','.cmd'))`: `_max_length = 8192 - len(find_executable('cmd.exe')) - len(' /c ') - 1024`.
3. `partitions = partition(cmd, varargs, target_concurrency, _max_length)`.
4. `threads = min(len(partitions), target_concurrency)`. If threads == 1, run serially (`map`). Otherwise use a `ThreadPoolExecutor(threads).map`, which keeps results **in partition order**.
5. Each partition runs `cmd_output_b(*part, check=False, stderr=STDOUT)`, or `cmd_output_p` (pty) when color.
6. Aggregation: `retcode = 0; for rc, out in results: if abs(rc) > abs(retcode): retcode = rc; stdout += out`.

`partition(cmd, varargs, target_concurrency, _max_length)` — reproduce exactly:
```
max_args = max(4, ceil(len(varargs) / target_concurrency))
ret = []; ret_cmd = []
varargs = reversed copy (pop from end = original order)
total = cmdlen(' '.join(cmd)) + 1
while varargs:
    arg = pop
    alen = cmdlen(arg) + 1
    if total + alen <= max_len and len(ret_cmd) < max_args:
        ret_cmd.append(arg); total += alen
    elif not ret_cmd:
        raise ArgumentTooLongError(arg)          # uncaught → exit 3
    else:
        ret.append(cmd + ret_cmd); ret_cmd = []; total = cmdlen(' '.join(cmd)) + 1; push arg back
ret.append(cmd + ret_cmd)                         # always ≥1 partition; zero files ⇒ exactly (cmd,)
```
- `cmdlen`: POSIX `len(s.encode(sys.getfilesystemencoding()))` (UTF-8 bytes). win32 `len(s.encode('utf-16le')) // 2` (UTF-16 code units).
- `max_len` (`_get_platform_max_length`, computed **at import**):
  - POSIX: `clamp(sysconf(SC_ARG_MAX) - 2048 - environ_size, 4096, 131072)`, where `environ_size = 8*len(environ) + Σ(len(k)+len(v)+2)` over `os.environb` (bytes). Linux with an 8 MiB stack has ARG_MAX = 2097152, giving **131072**. macOS ARG_MAX = 1048576, giving 131072.
  - Windows: `2**15 - 2048 = 30720`.
  - Other: 4096.
- Test vectors (`tests/xargs_test.py`): `partition(('foo',), ('A',)*22, 4, _max_length=50)` → partitions of 6,6,6,4. `('A',)*10, 4` → 4,4,2. `('ninechars',), ('.'*5,'.'*4,'.'*10,'.'*5,'.'*6), 1, 21` → `[5,4]`, `[10]`, `[5]`, `[6]`. `partition(('a'*5,), ('a'*5,), 1, 10)` raises.
- Observed: 36 staged files, 22 CPUs → `max_args = max(4, ceil(36/22)=2) = 4` → 9 invocations of 4 files, printed in partition order.

### 11.7 Deterministic shuffle (`lang_base.py:22,146-153`) — MUST be bit-exact

`random.Random(); seed(1542676187, version=1); shuffle(list)`. For an int seed `version` is irrelevant. CPython seeds MT19937 with `init_by_array(key=[1542676187])` (the absolute value split into 32-bit little-endian words; here a single word).

```
MT19937 (N=624, M=397, MATRIX_A=0x9908B0DF, UPPER=0x80000000, LOWER=0x7FFFFFFF), all arithmetic mod 2^32
init_genrand(19650218): mt[0]=19650218; mt[i]=(1812433253*(mt[i-1]^(mt[i-1]>>30))+i)
init_by_array(key):
  init_genrand(19650218); i=1; j=0
  repeat max(N, len(key)) times:
     mt[i] = (mt[i] ^ ((mt[i-1]^(mt[i-1]>>30))*1664525)) + key[j] + j
     i+=1; j+=1; if i>=N { mt[0]=mt[N-1]; i=1 }; if j>=len(key) { j=0 }
  repeat N-1 times:
     mt[i] = (mt[i] ^ ((mt[i-1]^(mt[i-1]>>30))*1566083941)) - i
     i+=1; if i>=N { mt[0]=mt[N-1]; i=1 }
  mt[0] = 0x80000000;  index = N           # forces a twist on first draw
genrand_uint32: standard twist + tempering (y^=y>>11; y^=(y<<7)&0x9D2C5680; y^=(y<<15)&0xEFC60000; y^=y>>18)
getrandbits(k) for 1<=k<=32: genrand_uint32() >> (32-k)
randbelow(n): k = bit_length(n); r = getrandbits(k); while r >= n: r = getrandbits(k); return r
shuffle(x): for i in (len(x)-1) down to 1: j = randbelow(i+1); swap(x[i], x[j])
```
Test vectors (verified against CPython 3.12 with an independent reimplementation, for all n in range(0,2000,37)):
- first five `genrand_uint32()`: `2472326011, 2061721674, 2625518588, 881996946, 1343846257`
- n=2 → `[0,1]`. n=3 → `[0,1,2]`. n=4 → `[1,0,2,3]`. n=5 → `[1,0,2,3,4]`.
- n=10 → `[4,0,5,1,8,6,2,3,7,9]` (the same vector as `tests/lang_base_test.py:138-141`).
- n=20 → `[4,9,5,14,0,13,12,11,1,17,2,8,3,7,19,16,10,6,15,18]`.

The shuffle is applied to the post-filter list, after `pass_filenames` truncation (an empty list is a no-op). It is a fresh RNG per call, so every hook shuffles independently from the same seed.

### 11.8 Process I/O — non-color mode (`util.py:81-113`)

- `subprocess.Popen(cmd, stdin=PIPE, stdout=PIPE, stderr=STDOUT)` (one pipe for both streams, so the hook's own write order is preserved). `communicate()` sends nothing and closes stdin immediately, so the hook reads EOF (observed `stdin-bytes=0`). No `cwd`, so the cwd is the repo root. env = current `os.environ`. `close_fds=True` (Python default). No new session or process group.
- `returncode`: Python's `-N` for signal N (observed `- exit code: -9`, `-15`).
- `OSError` at spawn ⇒ `(1, str(e).encode().rstrip(b'\n') + b'\n')`, e.g. `[Errno 8] Exec format error: './noshebang.sh'` (format `[Errno {errno}] {strerror}: {repr(filename)}`; on Windows `[WinError N] …`). This is **per partition**, so it is repeated once per partition (observed twice).
- The output is collected fully in memory. Nothing streams to the terminal while the hook runs.

### 11.9 Process I/O — color mode, POSIX (`util.py:123-200`)

- `cmd_output_p`: stdin = `/dev/null`. stdout **and** stderr = the slave of a fresh `openpty()` (one per partition). Before spawn: `tcgetattr(slave)`, clear `OPOST` and `ONLCR` in `c_oflag`, then `tcsetattr(TCSANOW)`, so `\n` is not turned into `\r\n`. The window size is **not** set (0×0). There is **no setsid**, so the pty is not the controlling terminal.
- The parent closes the slave after spawn and reads the master in 4096-byte chunks until EOF, treating **EIO** as EOF. Then `proc.wait()`.
- Hooks see `isatty(1)` and `isatty(2)` as true (observed `stdout-tty`) and typically emit ANSI colors, which pass through verbatim.
- Windows: `cmd_output_p = cmd_output_b` (no pty).

---

## 12. File-modification detection (`run.py:274-279`)

- `_get_diff()` = stdout of `git diff --no-ext-diff --no-textconv --ignore-submodules` (unchecked; the exit code is ignored and stderr is captured and discarded). This compares the **worktree with the index**, for tracked files only.
- It is taken once before the loop and once after each **executed** hook. Skipped and no-files hooks reuse the previous value.
- `files_modified = before != after` (a byte-exact comparison of the whole output).
- What it detects: content changes and mode changes (if `core.fileMode`) to tracked files, and deletions of tracked files.
- What it does **not** detect:
  - changes to untracked or ignored files;
  - changes the hook also `git add`ed;
  - submodule changes;
  - in `--all-files`/`--files` mode (no stash), changes to a **binary** file that was already modified, because the text says only "Binary files differ";
  - a hook that rewrites a file with identical content.
- The diff honours user git config (`color.diff=always`, `diff.noprefix`, `diff.renames`, …). That is harmless because it is only compared for equality.
- In `--all-files` mode, pre-existing unstaged changes are part of the baseline, so they do not count as modifications by themselves.

---

## 13. Output format — byte-exact

All writes go through `output.write`/`write_line(_b)`, which **flush stdout after every write** (`output.py:9-33`). Strings are encoded as UTF-8 regardless of locale. Hook output bytes are passed through unmodified (arbitrary bytes allowed).

### 13.1 Width

```
_len_cjk(s) = Σ over chars c of {W:2, F:2, A:1, H:1, N:1, Na:1}[unicodedata.east_asian_width(c)]
cols = max(80, max(_len_cjk(h.name) for h in selected_hooks, default 0) + 3 + len('(no files to check)')=19 + 1 + len('Skipped')=7)
     = max(80, maxname + 30)
_start_msg(name)          = name + '.' * (cols - _len_cjk(name) - 6 - 1)
_full_msg(name, postfix, end) = name + '.' * (cols - _len_cjk(name) - len(postfix) - len(end) - 1) + postfix + color(end) + '\n'
```
- Python `'.' * negative` is `''`. In Rust use `saturating_sub`.
- Every status line is exactly `cols - 1` columns, i.e. **79** by default (observed `passing hook` + 61 dots + `Passed`).
- The widest hook in the no-files case gets exactly 3 dots: `name...(no files to check)Skipped`. There is **no space** before `Skipped`.
- CJK example: `检查中文名称の長いフック` (12 chars × 2 = 24) gets 49 dots before `Passed`. Observed: `emoji 🎉 é combining é` counts 🎉 as 2.
- Unicode version = the running Python's `unicodedata` (3.12 → Unicode 15.0). This is **not** wcwidth: combining marks and zero-width characters count 1.
- Test vectors (`tests/commands/run_test.py:39-99,576-591`): `_start_msg('start', end_len=5, cols=15)` = `start....`. `_full_msg('start','end',cols=15)` = `start......end\n`. With postfix `'post '` and cols=20: `start......post end\n`. `_compute_cols([])=80`, one name of 51 chars → 81, names of 51 and 52 → 82.

### 13.2 Line catalogue (no color / color)

| Situation | No color | Color |
|---|---|---|
| start (hook running) | `{name}{dots}` (no newline yet) | same (dots never colored) |
| passed | `Passed\n` | `\x1b[42mPassed\x1b[m\n` |
| failed | `Failed\n` | `\x1b[41mFailed\x1b[m\n` |
| skipped by SKIP | `{name}{dots}Skipped\n` | `{name}{dots}\x1b[43;30mSkipped\x1b[m\n` |
| no files | `{name}{dots}(no files to check)Skipped\n` | `{name}{dots}(no files to check)\x1b[46;30mSkipped\x1b[m\n` (postfix not colored) |
| detail | `- hook id: {id}\n` | `\x1b[2m- hook id: {id}\x1b[m\n` |
| detail | `- duration: {d}s\n` | `\x1b[2m- duration: {d}s\x1b[m\n` |
| detail | `- exit code: {rc}\n` | `\x1b[2m- exit code: {rc}\x1b[m\n` |
| detail | `- files were modified by this hook\n` | `\x1b[2m- files were modified by this hook\x1b[m\n` |
| output block | `\n{out.strip()}\n\n` | same (hook bytes verbatim) |
| log line | `[{LEVEL}] {msg}\n` | `{COLOR}[{LEVEL}]\x1b[m {msg}\n` with ERROR=`\x1b[41m`, WARNING=`\x1b[43;30m`, INFO/DEBUG=`` (empty, but the reset is still emitted) |

- `out.strip()` is `bytes.strip()`: it strips ASCII whitespace `b' \t\n\r\x0b\x0c'` at **both** ends of the **combined** output. Interior and trailing spaces on inner lines are kept. Observed `"  out-with-spaces  \n…"` printed as `out-with-spaces  `.
- If the output is empty after the strip, no blank lines are printed.
- Duration: `round(x, 2) or 0`, printed with Python `repr(float)`, so `0` (exactly zero after rounding), `0.01`, `0.1`, `0.77`, `1.0`, `1.24`, `12.5`. Rust recipe: `s = format!("{:.2}", x)` (verified to round identically to Python on ties such as 0.125→0.12, 2.675→2.67, 0.375→0.38). If `s == "0.00"` print `0`. Otherwise trim trailing `0`s but keep at least one digit after the `.`.
- `log_file`: only the **output block payload** (`out.strip() + b'\n'`) is appended (`'ab'`), and only when the details block is printed **and** the output is non-empty. Status lines are never logged. The path is relative to the repo root. A missing parent directory raises FileNotFoundError, giving exit 3. In color mode, pty ANSI codes are written to the file too. Observed: a passing non-verbose hook with output and `log_file: hook2.log` creates **no file**.
- Hook output is **discarded** if the hook passes and neither `--verbose` nor `verbose: true` is set.

### 13.3 Captured transcript (no color, `SKIP=skipme`, 4.1.0)

```
passing hook.............................................................Passed
failing hook.............................................................Failed
- hook id: fail
- exit code: 3

out
err

no files hook........................................(no files to check)Skipped
skipped hook............................................................Skipped
fail lang................................................................Failed
- hook id: f
- exit code: 1

this is the fail message

a.txt

```
Color, verbose (`cat -v`):
```
passing hook.............................................................^[[42mPassed^[[m
^[[2m- hook id: pass^[[m
^[[2m- duration: 0s^[[m

hello .pre-commit-config.yaml a.txt b.py

failing hook.............................................................^[[41mFailed^[[m
^[[2m- hook id: fail^[[m
^[[2m- duration: 0s^[[m
^[[2m- exit code: 3^[[m

out
err

no files hook........................................(no files to check)^[[46;30mSkipped^[[m
^[[2m- hook id: nofiles^[[m
skipped hook............................................................^[[43;30mSkipped^[[m
^[[2m- hook id: skipme^[[m
```
Exec-error transcript:
```
not found................................................................Failed
- hook id: notfound
- exit code: 1

Executable `does-not-exist-xyz` not found

is a dir.................................................................Failed
- hook id: dir
- exit code: 1

Executable `./sub` is a directory

not executable...........................................................Failed
- hook id: noexec
- exit code: 1

Executable `./noexec.sh` is not executable

no shebang...............................................................Failed
- hook id: noshebang
- exit code: 1

[Errno 8] Exec format error: './noshebang.sh'
[Errno 8] Exec format error: './noshebang.sh'

killed by signal.........................................................Failed
- hook id: sig
- exit code: -9
```

---

## 14. Message ordering within one `run`

1. (Windows) cygwin mismatch warning.
2. Preflight error (§5). The run ends here.
3. `[WARNING] Unstaged intent-to-add files detected.`
4. `[WARNING] Unstaged files detected.` then `[INFO] Stashing unstaged files to <p>.`
5. Config-load warnings (unexpected keys, mutable rev, sensible-regex, deprecated stages).
6. `[INFO] Initializing environment for <repo>.` (clone) and per-repo deprecated-stage warnings. Repository errors such as an unknown hook id call `exit(1)` here.
7. `No hook with id … in stage …` (then restore and return 1).
8. `[INFO] Installing environment for <src>.` / `[INFO] Once installed this environment will be reused.` / `[INFO] This may take a few minutes...` (and `[INFO] Locking pre-commit directory` if the lock is contended).
9. `[INFO] Checking merge-conflict files only.`
10. Hook lines.
11. show-diff block (§15).
12. `[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...` (only on conflict) and `[INFO] Restored changes from <p>.`
13. Crash message (if an exception escaped). Printed **after** the restore message, because the error handler sits outside the stash context.

---

## 15. `--show-diff-on-failure` (`run.py:304-320`)

The condition is `retval and args.show_diff_on_failure and prior_diff` (the diff after the last executed hook is non-empty). It runs **inside** the stash context, so in stash mode it shows only hook-made changes.
1. If `args.all_files`, `write_line` this 3-line text:
   ```
   pre-commit hook(s) made changes.
   If you are seeing this message in CI, reproduce locally with: `pre-commit run --all-files`.
   To run `pre-commit` as part of git workflow, use `pre-commit install`.
   ```
2. `All changes made by hooks:`
3. `subprocess.call(('git','--no-pager','diff','--no-ext-diff','--color=always'|'--color=never'))`: inherits stdout/stderr, so output goes straight to fd 1. The exit code is ignored. Note it has **no** `--no-textconv`/`--ignore-submodules`.

The return code is unchanged. Observed with `-a`: the diff also contained a **pre-existing** unstaged change (`+a2`), because `--all-files` does not stash. The text says "All changes made by hooks", but it really shows the whole worktree diff.

---

## 16. `hook-impl`: running from a git hook (`commands/hook_impl.py`)

### 16.1 The installed script (`resources/hook-tmpl`, `install_uninstall.py:88-108`)

```bash
#!/usr/bin/env bash                      # Windows: "#!/bin/sh\n" is written before it
# File generated by pre-commit: https://pre-commit.com
# ID: 138fd403232d2ddd5efb44317e38bf03

# start templated
INSTALL_PYTHON=/path/to/python
ARGS=(hook-impl --config=.pre-commit-config.yaml --hook-type=commit-msg)   # + --skip-on-missing-config
# end templated

HERE="$(cd "$(dirname "$0")" && pwd)"
ARGS+=(--hook-dir "$HERE" -- "$@")

if [ -x "$INSTALL_PYTHON" ]; then
    exec "$INSTALL_PYTHON" -mpre_commit "${ARGS[@]}"
elif command -v pre-commit > /dev/null; then
    exec pre-commit "${ARGS[@]}"
else
    echo '`pre-commit` not found.  Did you forget to activate your virtualenv?' 1>&2
    exit 1
fi
```
Git runs hooks with cwd = the worktree root, so the relative `--config` resolves. The CLI, arguments, env and stdin semantics of `hook-impl` must be accepted **verbatim** by snp, because existing scripts and templates (`init-templatedir`) call it.

### 16.2 Algorithm (`hook_impl.py:259-275`)

1. `retv, stdin = _run_legacy(hook_type, hook_dir, args)`:
   - If env `PRE_COMMIT_RUNNING_LEGACY` is non-empty: `SystemExit` with this text on stderr, exit 1:
     ```
     bug: pre-commit's script is installed in migration mode
     run `pre-commit install -f --hook-type <type>` to fix this

     Please report this bug at https://github.com/pre-commit/pre-commit/issues
     ```
   - `stdin = sys.stdin.buffer.read()` if `hook_type == 'pre-push'`, else `b''`. stdin is read **completely, before anything else**.
   - If `hook_dir is None`: return `(0, stdin)`.
   - `legacy = join(hook_dir, f'{hook_type}.legacy')`. If `os.access(legacy, X_OK)`: run `normalize_cmd((legacy, *args))` with env + `PRE_COMMIT_RUNNING_LEGACY=1` and `input=stdin`. stdout/stderr are inherited (not captured). Return its returncode. Otherwise return `(0, stdin)`.
2. `_validate_config(retv, config, skip_on_missing_config)`. If `not os.path.isfile(config)`:
   - With `--skip-on-missing-config` or a non-empty `PRE_COMMIT_ALLOW_NO_CONFIG`: print to **stdout** ``\`.pre-commit-config.yaml\` config file not found. Skipping \`pre-commit\`.``, then exit `retv`.
   - Otherwise print to stdout, then exit 1:
     ```
     No .pre-commit-config.yaml file was found
     - To temporarily silence this, run `PRE_COMMIT_ALLOW_NO_CONFIG=1 git ...`
     - To permanently silence this, install pre-commit with the --allow-missing-config option
     - To uninstall pre-commit run `pre-commit uninstall`
     ```
3. `_check_args_length` (errors are `SystemExit(str)`: stderr, exit 1; the list uses Python `repr`, e.g. `['a', 'b']`):

| hook_type | allowed argc | message |
|---|---|---|
| prepare-commit-msg | 1..3 | `hook-impl for prepare-commit-msg expected 1, 2, or 3 arguments but got {n}: {args}` |
| pre-rebase | 1..2 | `hook-impl for pre-rebase expected 1 or 2 arguments but got {n}: {args}` |
| commit-msg 1, post-checkout 3, post-commit 0, pre-commit 0, pre-merge-commit 0, post-merge 1, post-rewrite 1, pre-push 2 | exact | `hook-impl for {t} expected {k} argument(s) but got {n}: {args}` ("argument" when k == 1) |
| anything else | — | `AssertionError('unexpected hook type: X')` → exit 3 (raised after legacy and config handling) |

4. Namespace mapping (`_run_ns`, `hook_impl.py:214-256`). Every case sets `files=()`, `hook=None`, `verbose=False`, `show_diff_on_failure=False`, `fail_fast=False`, `all_files=False` unless noted, and `color` = `--color` of hook-impl.

| hook_type | run namespace |
|---|---|
| pre-commit, pre-merge-commit, post-commit | `hook_stage=type` |
| commit-msg | `commit_msg_filename=argv[0]` |
| prepare-commit-msg | `commit_msg_filename=a0`, `prepare_commit_message_source=a1?`, `commit_object_name=a2?` |
| post-checkout | `from_ref=a0` (previous HEAD), `to_ref=a1` (new HEAD), `checkout_type=a2` |
| post-merge | `is_squash_merge=a0` |
| post-rewrite | `rewrite_command=a0`. **stdin (the rewritten list) is not read.** |
| pre-rebase | `pre_rebase_upstream=a0`, `pre_rebase_branch=a1?` |
| pre-push | `_pre_push_ns` (16.3). `remote_name=a0`, `remote_url=a1` |

5. If the namespace is `None` (nothing to push): return `retv`. Otherwise return **`retv | run(config, store, ns)`**.

### 16.3 pre-push stdin algorithm (`hook_impl.py:120-174`)

For each line of `stdin.decode()` (strict UTF-8) `.splitlines()`:
- `local_ref, local_sha, remote_ref, remote_sha = line.rsplit(maxsplit=3)`. This is a right split, so a local ref with spaces is fine. Anything other than 4 fields raises ValueError → exit 3.
- If `local_sha == '0'*40` (a deletion), skip the line. **Only the 40-char SHA-1 zero is recognised.**
- Else if `remote_sha != Z40` and `git cat-file -e <remote_sha>^{commit}` exits 0 (4.6.1+; 4.1.0 used `git rev-list --quiet <sha>`): return the namespace with `from_ref=remote_sha`, `to_ref=local_sha`, `remote_branch=remote_ref`, `local_branch=local_ref`. The stderr of `cat-file` is **not** suppressed: `fatal: Not a valid object name <sha>^{commit}` leaks when the remote sha is unknown locally.
- Else: `git rev-list <local_sha> --topo-order --reverse --not --remotes=<remote_name>` (checked):
  - If the output is empty: skip the line (everything is already on the remote).
  - Otherwise `first = output.splitlines()[0]`. `roots = set(git rev-list --max-parents=0 <local_sha>)`:
    - If `first in roots`: **all_files=True** namespace (pushing a whole new history / orphan). This means no stash and `git ls-files` files.
    - Else `source = git rev-parse <first>^`: namespace with `from_ref=source`, `to_ref=local_sha`.
- **The function returns at the first line that produces a namespace.** Later refs in a multi-ref push are ignored. If no line produces one, return `None`, so the push is not checked at all.
- Observed: pushing a new branch whose commits already exist on the remote gives `None` (silent, exit 0). Pushing the root commit gives all files plus `PRE_COMMIT_REMOTE_*`/`LOCAL_BRANCH` env, with no FROM/TO.

---

## 17. Error handler and crash log (`error_handler.py`, `logging_handler.py`)

`error_handler()` catches `Exception` and `KeyboardInterrupt`. `SystemExit` passes through.

| Exception | Header | Exit |
|---|---|---|
| `FatalError` (and subclasses `InvalidConfigError`, `InvalidManifestError`) | `An error has occurred` | 1 |
| `KeyboardInterrupt` | `Interrupted (^C)` | 130 |
| anything else | `An unexpected error has occurred` | 3 |

`_log_and_exit(msg, code, exc, traceback)`:
1. `error_msg = f'{msg}: {type(exc).__name__}: '.encode() + force_bytes(exc)`. `force_bytes` returns `bytes(exc)` if supported (CalledProcessError defines `__bytes__`), otherwise `str(exc).encode()`, otherwise `<unprintable X object>`. The message is written to stdout with `\n`. KeyboardInterrupt gives `Interrupted (^C): KeyboardInterrupt: ` with a **trailing space**.
2. Run `git --version` (unchecked). Decode with backslashreplace and rstrip.
3. `storedir = Store().directory` (re-creates the store if needed). `log_path = join(storedir, 'pre-commit.log')`.
4. If the store dir is writable: print `Check the log at {log_path}` and **overwrite** (`'wb'`) the log. Otherwise print `Failed to write to log at {log_path}` and write the log content to stdout.
5. Log content, byte-exact:
````
### version information

```
pre-commit version: 4.1.0
git --version: git version 2.43.0
sys.version:
    3.12.3 (main, Aug 31 2026, 10:18:26) [GCC 13.3.0]
sys.executable: /home/…/bin/python
os.name: posix
sys.platform: linux
```

### error information

```
An unexpected error has occurred: ValueError: No closing quotation
```

```
Traceback (most recent call last):
  …
ValueError: No closing quotation
```
````
6. `raise SystemExit(code)`.

`logging_handler`: the `pre_commit` logger at level INFO. Each record becomes `write_line(f'{format_color(f"[{levelname}]", LEVEL_COLOR, use_color)} {message}')` on stdout. DEBUG is never shown.

`CalledProcessError.__bytes__` (`util.py:63-75`):
```
command: ('git', 'write-tree')\n
return code: 128\n
stdout:\n    <indented, rstripped>  | stdout: (none)\n
stderr:\n    <indented, rstripped>  | stderr: (none)
```
The command is the Python `repr` of the **normalised** argv tuple, e.g. `('/usr/bin/git', 'write-tree')`.

---

## 18. Platform differences

| Topic | Linux | macOS | Windows |
|---|---|---|---|
| auto color | `isatty(stderr)` and TERM≠dumb | same | additionally requires that enabling VT mode on the stderr console succeeds |
| pty for colored hooks | yes | yes | no (pipes) |
| path normalisation | none | none | `\` → `/` in all candidate filenames before filtering |
| cpu_count | `sched_getaffinity` | `multiprocessing.cpu_count` | same as macOS |
| max cmdline | `clamp(ARG_MAX-2048-env,4096,131072)` bytes | same (ARG_MAX 1 MiB) | 30720 UTF-16 units; `.bat/.cmd`: `8192-len(cmd.exe path)-4-1024` |
| executable lookup | PATH, X_OK | same | PATH × PATHEXT (extensions first, then the bare name); X_OK is always true |
| "is not executable" error | yes | yes | never |
| shebang emulation | redundant but changes argv0 and errors | same | essential (`#!/usr/bin/env python` scripts work) |
| hook script header | `#!/usr/bin/env bash` | same | `#!/bin/sh` line prepended |
| OSError text | `[Errno N] msg: 'path'` | same | `[WinError N] msg` |
| file lock | `flock` | `flock` | `msvcrt.locking` region 0xffff, retry loop |
| cygwin | — | — | warns on a cygwin/windows python/git mix |

---

## 19. Edge cases checklist (all derived from code or observed)

- No hooks selected (empty config or all filtered by stage): no output, return 0. Stash messages may still appear.
- No staged files: every hook shows `(no files to check)Skipped` unless `always_run`.
- `always_run: true` with no files: one invocation with no file args (`partition` returns `(cmd,)`).
- `pass_filenames: false` with many files: still exactly **one** invocation (no varargs).
- `--files` with directories: they pass `lexists` but get the tag `directory`, so they are filtered by `types: [file]`.
- `--files` with paths outside the repo: `../x` survives if it exists.
- `commit-msg` hooks get the message file through the normal filters. A top-level `exclude` matching `.git/COMMIT_EDITMSG`, or `types` that do not match `text`, would hide it.
- Hook names are not escaped. Newlines in a name break the layout.
- The same hook id listed twice: both run, and the `hook` argument selects both.
- Hooks are executed strictly sequentially. Parallelism exists only inside one hook's partitions.
- Hook output from concurrent partitions is concatenated by partition index, not by completion time.
- `hook-impl` for `post-checkout` during the stash `git checkout -- .`: `_PRE_COMMIT_SKIP_POST_CHECKOUT=1` makes the nested run return 0 immediately (prevents recursion, #1418).
- A crash between `_start_msg` and the status leaves a dangling partial line. The next output (restore info or crash message) continues on the same line.

---

## 20. Version deltas relevant to run (4.1.0 installed → 4.6.2 target)

| Change | Version | Spec follows |
|---|---|---|
| `run --fail-fast` flag | 4.4.0 | 4.6.2 |
| `language: unsupported`/`unsupported_script` aliases for system/script (translated at config load) | 4.4.0 | 4.6.2 |
| `pre-commit hazmat` + `hook_cmd` rewrite | 4.5.0 | 4.6.2 |
| `hook-impl --hook-type` required; `--hook-dir` optional (git 2.54+ config hooks) | 4.6.0 | 4.6.2 (4.1.0 crashes with TypeError when `--hook-dir` is missing, observed) |
| pre-push remote existence check `git cat-file -e <sha>^{commit}` (was `git rev-list --quiet`) | 4.6.1 | 4.6.2 |
| `git ls-files -z --deduplicate` | 4.6.1 | 4.6.2 (requires git ≥ 2.31) |
| `--` after `MERGE_HEAD` in the conflict diff | 4.2.0 | 4.6.2 |

---

## 21. Implementation notes for snp (Rust 1.98, edition 2024)

### 21.1 Crate and module layout for the run pipeline
- `#![forbid(unsafe_code)]` in all snp crates. Third-party crates may contain unsafe (std, rustix, nix, signal-hook), but no snp code.
- Suggested modules: `cli::run` (clap), `git` (typed wrappers, one function per git command in this spec), `stash` (RAII guard), `select` (file selection + Classifier), `xargs` (partition + shuffle + executor), `proc` (spawn: pipe/pty), `printer` (all formatting), `errors` (FatalError/Unexpected/Interrupted → exit codes + crash log), `hook_impl`.
- Model each stage's arguments as an enum (`HookStage`) with `FromStr` doing the legacy translation. Keep the `Option<String>` refs and treat `Some("")` as `None` everywhere, matching Python truthiness.

### 21.2 CLI (clap 4)
- `Command::new("pre-commit")`-compatible: `infer_long_args(true)` (argparse prefix matching), `args_override_self(true)`. `--files` is `num_args(0..)` with `ArgAction::Set`: **last occurrence wins** (observed: `--files a.txt --files b.txt` → only `b.txt`). `--all-files` conflicts with `--files` **whenever `--files` appears, even with zero values** (observed: `-a --files` → `argument --files: not allowed with argument --all-files/-a`; `--files -a` → `argument --all-files/-a: not allowed with argument --files`; the message names the later option first). `-s/--source` and `-o/--origin` are visible aliases.
- Positional `hook` is optional, and `--files` must stay greedy (users expect `--` to terminate it).
- With no argv, run `run`. Implement this before clap parsing so `snp` alone behaves like `pre-commit`.
- `--color` default = `env PRE_COMMIT_COLOR` or `auto`. Validate env values with the same error, exit 2.
- Usage errors: exit code 2 to stderr. Byte-identical argparse wording is optional (see Open questions). At minimum keep the exit code and the `error:` keyword.

### 21.3 Process spawning without unsafe
- **Merged stdout/stderr**: `let (reader, writer) = std::io::pipe()?;` (stable since 1.87). `cmd.stdout(writer.try_clone()?).stderr(writer)`, `stdin(Stdio::null())`. **Drop all writer handles in the parent right after `spawn()`**, `read_to_end` the reader, then `wait()`. This reproduces `stderr=STDOUT` ordering exactly and works on Windows.
- **Exit codes**: `status.code().unwrap_or_else(|| -status.signal().unwrap_or(0))` on Unix (`ExitStatusExt`).
- **Spawn errors**: format like Python: `format!("[Errno {e}] {msg}: {py_repr(path)}")`, where `msg` = `io::Error::from_raw_os_error(e).to_string()` with the ` (os error N)` suffix stripped. Implement a `py_repr(&str)` helper (single quotes unless the string contains `'` and no `"`; escape `\\`, the quote, `\n`, `\r`, `\t`, non-printables as `\xNN`/`\uNNNN`/`\UNNNNNNNN`). Windows: emulate `[WinError N] <msg>`.
- **pty mode (Unix, color=true)**: use `rustix::pty::{openpt, grantpt, unlockpt, ptsname}` + `rustix::termios::{tcgetattr, tcsetattr}` (safe APIs). Clear `OPOST|ONLCR`. Pass the slave `OwnedFd` as stdout and as stderr (`try_clone`), with stdin null. Do **not** setsid or make it the controlling tty. Drop the slave in the parent. Read the master until `Ok(0)` or `Err(EIO)`. One pty per partition. Optionally set the window size to the parent terminal's (an improvement; pre-commit leaves 0×0; see Open questions).
- Never pass a shell. Always exec argv directly, like pre-commit.
- Reset nothing extra: std already resets SIGPIPE to default and clears the signal mask in children. Use *caught* handlers (signal-hook/ctrlc), never `SIG_IGN`, for SIGINT/SIGTERM. Caught dispositions reset to default on exec, but ignored ones would be inherited by hooks.

### 21.4 Concurrency
- Per hook: `std::thread::scope` with `min(partitions, jobs)` workers pulling partition indices from an `AtomicUsize`. Store `(i32, Vec<u8>)` results in a `Vec<OnceLock<…>>` by index and concatenate in index order. No async runtime is needed for the run path.
- `jobs`: `PRE_COMMIT_NO_CONCURRENCY` present → 1. `TRAVIS` present → 2. Otherwise CPU count. For parity on Linux, prefer the sched affinity count (`rustix::thread::sched_getaffinity` + counting set bits; safe API, but check that the rustix version in use exposes a count or iterate `CpuSet::is_set`) over `available_parallelism` (that one also applies cgroup quotas and can be smaller). The count only changes partition sizes and the number of invocations, never the order of files in the concatenated output.
- Exit code aggregation: the max by `abs`, first wins ties.

### 21.5 Command-length budget
- POSIX ARG_MAX: `nix::unistd::sysconf(SysconfVar::ARG_MAX)` (safe wrapper), with a 131072 fallback. Compute `environ_size` from `std::env::vars_os()` once at startup using pre-commit's formula (8 per pointer + `k.len() + v.len() + 2`). Then `clamp(arg_max - 2048 - env, 4096, 131072)`.
- Lengths: Unix `OsStr::len()` (bytes); Windows `encode_wide().count()`.
- Reimplement `partition` exactly (§11.6), including `max_args`, the `+1`s and `ArgumentTooLongError` (map it to the exit-3 path with the text `An unexpected error has occurred: ArgumentTooLongError: <arg>`).

### 21.6 Shuffle
- Implement MT19937 + `init_by_array` + CPython `_randbelow`/`shuffle` exactly as in §11.7 (about 60 lines, no deps), or use `rand_mt` (`Mt::new_with_key`) plus a hand-written `randbelow`/`shuffle`. Unit-test it against all the vectors in §11.7. **Do not** use `fastrand`/`rand::seq::SliceRandom`. prek does that, and its file order (and therefore its output order and partition contents) diverges from pre-commit.
- Shuffle a `Vec<u32>` of file indices, not strings (cheap swaps).

### 21.7 shlex
- Write a Python-compatible `shlex_split(&str) -> Result<Vec<String>, ShlexError>` state machine (§11.3). Do not use the `shlex` crate: its double-quote escape set (`$`, `` ` ``, `"`, `\`, newline) and its line-continuation handling differ from Python. Map the errors to the exact Python messages `No closing quotation` / `No escaped character`, and to the crash path `An unexpected error has occurred: ValueError: <msg>` (exit 3). That path is entered **after** the start line was printed, so emit no newline before it.

### 21.8 Executable resolution
- Port `normalize_cmd` exactly (§11.5), including the second `normexe` on the shebang interpreter, the three error strings, the PATH empty-component semantics and PATHEXT order. Use `file-identify`'s `parse_shebang_from_file` (the user's crate) for parity with identify. Verify that it applies the `/usr/bin/env -S` stripping, the printable-ASCII rule and the nix-shell rule.
- Cache PATH lookups per (PATH value, name) within a run, since many hooks use the same tools.

### 21.9 Diff-based modification detection, O(1) memory
- Stream the stdout of `git diff --no-ext-diff --no-textconv --ignore-submodules` through a hasher (`xxhash-rust` xxh3-128 or `blake3`). Keep `(digest, non_empty: bool)`. Compare digests instead of whole byte strings. `--show-diff-on-failure` only needs `non_empty`.
- Do not "optimise" with `git diff --quiet`. Its exit code differs from "output non-empty" in the CRLF/stat-dirty quirks, and the byte-compare semantics are what users observe.
- Skip the diff for skipped and no-files hooks (as pre-commit does). Optional opt-in: per-hook `modifies_files: false`. Not compatible, so it must stay opt-in.

### 21.10 Stash safety (highest-risk code: it holds the only copy of the user's unstaged work)
- Run the same git commands with the same flags, in the same order (§7). Inherit the **full env** (GIT_INDEX_FILE matters during `git commit -a` / `git commit <paths>`, where git uses a temporary index).
- Write the patch with `File::create_new` (O_EXCL), `write_all`, `sync_all`, and fsync the directory **before** `git checkout -- .`. pre-commit does not fsync. Snp adds it for safety at negligible cost. Keep the file name format `patch{secs}-{pid}` and keep the file afterwards (compat, and a backup).
- RAII guard: restore in `Drop`, plus an explicit `restore()` on the normal path that returns errors. Build with `panic = "unwind"` and wrap `main` in `catch_unwind`, so a panic still restores and then follows the exit-3 crash path.
- Signals: register SIGINT, SIGTERM and SIGHUP with `signal-hook` (flag + self-pipe) or `ctrlc` (`termination` feature). On the first signal: stop starting new partitions and hooks, let running children finish (they received SIGINT from the tty's process group; for SIGTERM/SIGHUP forward the signal to the children, then wait with a timeout and kill), restore the stash, print `Interrupted (^C): KeyboardInterrupt: ` + the log-path line, write the crash log, exit 130. pre-commit dies without restoring on SIGTERM/SIGHUP. Restoring there is a strict safety improvement. Use 130 for SIGINT and 128+N for the others.
- Order on exit: show-diff (inside the stash) → patch restore → intent-to-add re-add. Messages are exactly as in §7.
- `_PRE_COMMIT_SKIP_POST_CHECKOUT` must keep this **exact name** (both directions): snp's own nested post-checkout and any still-installed pre-commit scripts must honour it.

### 21.11 File selection and classification performance
- Read git `-z` output into one `Vec<u8>`, validate it as UTF-8 once, and keep `&str` slices (zero-copy). Represent the file list as `Vec<u32>` indices into a path arena.
- Windows: normalise `\`→`/` once at ingestion.
- Regex: compile each distinct pattern once (`HashMap<String, Arc<Regex>>`). The defaults `''` and `'^$'` short-circuit (include-all / exclude-nothing) without running a regex.
- Tags: `Vec<OnceLock<TagBits>>` indexed by file, so they are lazy (pre-commit parity: no I/O for files no hook looks at) and thread-safe. Optionally prefetch in parallel (rayon or scoped threads) for files that pass any hook's include/exclude. Represent tag sets as a bitset over the interned identify tag universe (about 650 tags → `[u64; 11]`), so `types`/`types_or`/`exclude_types` checks are 3 bitwise operations. Pre-compute each hook's three masks once.
- Missing file during lazy tagging: pre-commit crashes (exit 3). Recommend snp treats a `PathNotFound` from `file-identify` as "no tags", so the file is excluded (see Open questions).
- Use `file-identify` 0.4.0 (`tags_from_path`). Ensure the Windows executable semantics match Python's `os.access(X_OK)` (true for any existing file on Windows).

### 21.12 Output
- One `Printer` owning `std::io::stdout().lock()` (or a `BufWriter` flushed at exactly the points pre-commit flushes: after every `write`/`write_line`). Bytes only (`&[u8]`). No `println!`.
- East Asian Width: generate a static sorted range table of the W and F code points from Unicode `EastAsianWidth.txt` (build-time codegen committed to the repo; about 120 ranges) and binary-search it. Do **not** use `unicode-width` (wcwidth semantics: combining marks count 0, controls behave differently). Pin the Unicode version and document it.
- Duration formatting per §13.2. Status words and colors are constants.
- `log_file`: open in append mode per write (`OpenOptions::new().append(true).create(true)`), relative to the root.

### 21.13 Git invocations per typical `git commit` run (pre-commit 4.6.2) and how to trim them
`rev-parse --show-cdup`, `rev-parse --is-inside-git-dir`, `ls-files --unmerged`, `diff --quiet --no-ext-diff <cfg>`, `diff … --diff-filter=A` (ITA), `write-tree`, `diff-index …`, [`checkout`, `apply`], `rev-parse --git-dir` (merge check), `diff --staged --name-only …`, `diff` (baseline), plus one `diff` per executed hook.
- Merge `--show-cdup --is-inside-git-dir --git-dir --git-common-dir` into **one** `git rev-parse` call (multiple flags give one output line each).
- Determine merge state by stat'ing `MERGE_MSG`/`MERGE_HEAD` in the already-known git dir.
- Spawn independent read-only queries concurrently (unmerged check, ITA list, staged list), but keep the order of user-visible messages and the error precedence of §5.
- Keep using the git CLI, not gitoxide, for every operation in this spec. Exact parity with sparse index, fsmonitor, `GIT_INDEX_FILE`, autocrlf and textconv semantics matters more than avoiding about 10 process spawns. gix may be considered later for pure reads behind tests.

### 21.14 Avoiding Python
- `pygrep`, the `meta` hooks (identity, check-hooks-apply, check-useless-excludes) and `pre-commit hazmat …` entries must run **in-process or as `snp` sub-invocations**. This is the largest latency win: each Python start costs tens of milliseconds. Output and partition semantics must match exactly: pygrep has no shuffle and serial partitions, and identity output follows the shuffled order.
- `fail` runs in-process (pre-commit already does this).

### 21.15 Lefthook-inspired, **opt-in only**, compatible extensions
These are candidates for later topics. None may change the defaults above.
- `SNP_SKIP` alias of `SKIP`, and `SNP=0` (like `LEFTHOOK=0`) to disable all hooks from a git hook invocation.
- `NO_COLOR`/`CLICOLOR_FORCE` support. pre-commit ignores both, so make this a documented deviation or opt-in.
- Streaming output mode (`--stream`) that prints hook output live. pre-commit buffers.
- An end-of-run summary with per-hook durations.
- Parallel hook groups (lefthook `parallel`, prek priorities) for hooks marked side-effect-free.
- `stage_fixed`: auto-`git add` files modified by fixers when there were no unstaged changes in them (lefthook).
- Partially-staged-only stashing (lefthook hides only partially staged files). This needs care. Keep pre-commit's full-patch algorithm as the default.

### 21.16 Pitfalls checklist
- Python truthiness: an empty string is "unset" for every ref/flag option and for most env vars. `PRE_COMMIT_NO_CONCURRENCY`/`TRAVIS` use presence instead.
- `--origin` (CLI) means to-ref, but env `PRE_COMMIT_ORIGIN` holds the from-ref.
- `PRE_COMMIT_PRE_REBASE_*` are exported only when both values are present.
- `hook-impl` drops `rest[0]` unconditionally.
- The exit code of hook-impl is a **bitwise OR**, not a max.
- The xargs exit code is **max-abs**, not OR. prek ORs it, which is a deviation.
- `out.strip()` strips **both** ends.
- `(no files to check)` is followed directly by `Skipped` (no space).
- `[INFO]` gets a trailing `\x1b[m` even though it has no color.
- Patch files are never deleted.
- Merge-conflict file set order is random in Python. snp should sort it, which is harmless because the shuffle reorders it anyway (deviation).
- A non-UTF-8 path from git crashes pre-commit (strict decode).

---

## 22. Open questions

1. **Branding of messages.** Many strings contain "pre-commit" (`Your pre-commit configuration is unstaged.`, `pre-commit hook(s) made changes.`, ``reproduce locally with: `pre-commit run --all-files` ``, `pre-commit version:` in the crash log, the `No .pre-commit-config.yaml file was found` text). Should snp keep them byte-identical (recommended default for drop-in parity: CI log greps) or rebrand? Possibly substitute the program name only where it names a command to run.
2. **`--version` output** when invoked as `pre-commit` (symlink/shim). Some tools parse `pre-commit --version`. Should argv[0]-based behaviour print `pre-commit X`?
3. **argparse-identical error text and usage** vs clap's format. Exit code 2 parity is easy; byte parity of usage text is costly.
4. **CPU count source**: sched_getaffinity (parity) or `available_parallelism` (cgroup-aware, better in containers)?
5. **Non-UTF-8 filenames** (Unix): crash like pre-commit, skip with a warning, or support via lossy matching and raw-bytes argv? Recommendation: support raw `OsString` argv but match regexes on lossy UTF-8.
6. **Vanished file during lazy tagging**: keep the crash (parity) or exclude silently (recommended)?
7. **Crash log location and format** for snp: the shared `$PRE_COMMIT_HOME/pre-commit.log` (overwrites pre-commit's) or a separate snp store? This depends on the store-compat decision in the store spec. The `sys.version`/`sys.executable` lines have no Rust equivalent; replace them with `snp version`, `rustc`, `target triple` and `exe path`?
8. **SHA-256 repositories**: pre-commit only knows the 40-zero null OID (a bug for 64-hex repos). snp should treat any all-zero OID as null. Confirm it is acceptable to deviate.
9. **pre-push multi-ref**: pre-commit checks only the first ref with work. Keep parity (recommended by default), or offer opt-in checking of all refs?
10. **pty window size**: keep 0×0 (parity) or propagate the parent terminal size (better UX for tools that wrap output)?
11. **Suppress `git cat-file -e` stderr noise** in pre-push (cosmetic deviation)?
12. **SIGTERM/SIGHUP restore and exit codes** (130 vs 143/129): pre-commit has no defined behaviour here.
13. **`hook-impl` with `--hook-dir` absent** (git 2.54 config-based hooks): need to confirm how git 2.54 invokes the command (does it pass `--`?), so that `rest[1:]` stays correct.
14. **Unicode version for `_len_cjk`**: pick the latest UCD, or the one matching the Python that users' pre-commit most likely runs (3.12 → 15.0)? Differences affect only rare code points.
15. **Python `re` vs Rust regex** for `files`/`exclude`: deferred to the config spec. Lookaround/backrefs need `fancy-regex`. `\Z`, `$`-before-final-newline and `(?x)` need translation.
16. **Install-on-demand**: pre-commit installs environments for every selected non-skipped hook before running any, even if a hook then has no files. Should snp keep that eager install (output parity) or install lazily (faster)? Recommendation: eager by default.


---

## Addendum: Git version floor, git feature matrix, git ≥2.54 config-based hook invocation, and how user git config affects pre-commit's own git calls

Gap-fill #7. This section is appended. Nothing above it was changed. Where it corrects an earlier section of this file or of another spec, §G.9 says so explicitly.

**Evidence legend.** **SRC** = read in source. **OBS** = observed by running. **REL** = git release notes (`Documentation/RelNotes/<ver>.adoc`) or git docs at a tag.

**Setups used.**
- Host: Linux, git 2.43.0, pre-commit **4.6.2** (`scratchpad/venv-pc462`, Python 3.12.3), throwaway repos under `scratchpad/tmp-gitfloor/` with an isolated `HOME`, `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_NOSYSTEM=1` and `PRE_COMMIT_HOME`.
- Throwaway containers (images removed afterwards): Alpine edge with packaged **git 2.56.0**, Python 3.14.8 and pre-commit 4.6.2 from PyPI. Inside the same image, **git 2.18.5, 2.30.9 and 2.31.8** were compiled from kernel.org tarballs (`NO_CURL=1 NO_PERL=1 NO_PYTHON=1 NO_TCLTK=1 NO_GETTEXT=1 NO_EXPAT=1 NO_OPENSSL=1 CFLAGS='-O1 -w -std=gnu99'`). Probe scripts are in `scratchpad/scripts-gitfloor/`.
- git sources: a sparse, blobless clone of github.com/git/git at v2.53.0, v2.54.0, v2.55.0 and v2.56.0 (`scratchpad/gitsrc-dl/git`). 2.54.0 copies of `hook.c`, `run-command.c`, `transport.c`, `builtin/hook.c` and `githooks.adoc` are in `scratchpad/gitsrc-dl/`. Citations of the form `git:<file>:LINE@vX` refer to those copies. Git for Windows `fsmonitor-settings.c` (branch `main`) is in `scratchpad/dl-gfw/`.
- pre-commit citations are `pre_commit/<file>.py:LINE` at 4.6.2, as in the rest of this document.

### G.0 TL;DR: facts and decisions

1. **pre-commit 4.6.2 has no git version check.** It has two hard version dependencies:
   - `git ls-files --deduplicate` (git ≥ **2.31.0**) for `run --all-files`, for the pre-push "push the whole history" path, and for the meta hooks.
   - Intent-to-add paths shown as new files by `git diff` (git ≥ **2.19.0**) for the stash.

   Below 2.31, `--all-files` crashes with exit 3 (OBS on 2.30.9 and 2.18.5). Below 2.19, a commit with an unstaged `git add -N` file crashes with exit 3 after the patch fails to re-apply (OBS on 2.18.5).
2. **snp decision on versions:**
   - The supported and tested floor is **git 2.31.0** (lefthook uses the same, `internal/git/repo.go:20`).
   - Below **2.19.0**, snp refuses to run (FatalError, exit 1).
   - From 2.19.0 to 2.30.x, snp prints one warning and continues, using fallbacks.

   The exact texts are in §G.4.
3. **Config hooks in git ≥ 2.54 never pass `--`.** git runs `hook.<name>.command` as `sh -c '<command> "$@"' '<command>' <hook args…>` when the command contains a space or a shell metacharacter. Otherwise it execs the command directly with the hook args appended (`git:hook.c:467,478@v2.54.0`, `git:run-command.c:288-308@v2.54.0`).
   - The `--` that `hook-impl` drops with `rest[1:]` comes from the **configured command string**. pre-commit's docs use `'pre-commit hook-impl --hook-type pre-commit --'`.
   - Without that trailing `--`, `rest[1:]` drops the first real git argument. OBS: `hook-impl for commit-msg expected 1 argument but got 0: []` and `hook-impl for pre-push expected 2 arguments but got 1: ['/tmp/remote.git']`.
   - This answers 02 Q13. snp keeps `rest[1:]` unchanged, for parity.
4. **Config hooks get the same inputs as hooks-dir scripts.**
   - cwd is the worktree root, or `$GIT_DIR` in a bare repo.
   - Arguments are the same.
   - The env is the same, e.g. `GIT_INDEX_FILE`, `GIT_PREFIX`, `GIT_EDITOR=:`, `GIT_AUTHOR_*` and `GIT_EXEC_PATH`.
   - stdin is `/dev/null`, except for pre-push, where each hook gets its own full copy of the ref lines.

   Config hooks run **before** the hooks-dir script, and both run. So a repo with both `pre-commit install` and a config hook runs pre-commit **twice** (OBS on 2.56.0). Exit codes are OR-ed together (`git:hook.c:495,508@v2.54.0`).
5. **Data-safety hazards in pre-commit's own git calls** (all OBS on 4.6.2):
   - (a) `GIT_DIFF_OPTS=--unified=0` in the environment makes the stash patch zero-context, and `git apply` refuses it. pre-commit exits 3, and the user's unstaged changes survive only in the patch file.
   - (b) An intent-to-add file that rename detection pairs with a deleted tracked file (the default, `diff.renames=true`) is not detected as ITA. pre-commit then exits 3, and the user's unstaged deletion is lost from the worktree.
   - (c) If `GIT_DIR` is exported, `pre-commit autoupdate` writes `extensions.partialclone=true`, `remote.origin.promisor=true` and `remote.origin.partialclonefilter=blob:none` into the **user's** repo config, then crashes.
   - (d) `init.defaultObjectFormat=sha256` (git ≥ 2.49), and Git 3.0's planned default, breaks every store clone of a SHA-1 repo: `fatal: mismatched algorithms: client sha256; server sha1`.
6. snp fixes (a)–(d) with **no output change on any path that works in pre-commit**:
   - (a) Remove `GIT_DIFF_OPTS` from the env of the stash `diff-index` call only.
   - (b) Add `--no-renames` to the ITA query only.
   - (c) Use a sanitized env for every snp-owned repo.
   - (d) Choose or retry `git init --object-format`.

   Details are in §G.7.
7. **`-c core.useBuiltinFSMonitor=false` is a deprecated key that only Git for Windows knows.** Current GfW reads it only when `core.fsmonitor` is unset or a non-boolean string. Upstream git ignores it, and it does not stop the builtin daemon that `core.fsmonitor=true` starts.

   snp keeps it in argv exactly where pre-commit has it, so error texts stay byte-identical. snp also injects `core.fsmonitor=` (an empty value means off on every git version) through the `GIT_CONFIG_COUNT` env, but only for calls on snp-owned repos.
8. **User config that changes the bytes of pre-commit's own git output but not its meaning:**
   - `diff.noprefix`, `diff.mnemonicPrefix`, `diff.srcPrefix`/`dstPrefix`, `diff.context`, `diff.algorithm`, `color.ui`/`color.diff=always` and `GIT_DIFF_OPTS` all change the modification-detection diff. That diff is only compared for equality, so this is harmless.
   - `diff.orderFile` **reorders** every `--name-only` file list (OBS). For parity, snp must never sort git's output.
   - Plumbing `diff-index` ignores every `diff.*` UI key (OBS) except `diff.suppressBlankEmpty`, which is harmless (OBS).
   - `core.quotePath` changes nothing in pre-commit itself, because every path list is read with `-z`.
9. **`diff.renames` changes `get_changed_files`** (pre-push `from...to`): with renames off, the old path of a rename is also listed. snp must **not** add `--no-renames` there.

   On the staged list (`--diff-filter=ACMRTUXB`), `--no-renames` leaves the output unchanged (OBS) and saves the CPU of rename detection. snp may add it there.
10. **Several different failures produce the same message.** A `safe.directory` refusal ("detected dubious ownership"), a missing git, and "not a repository" all produce pre-commit's `git failed. Is it installed, and are you in a Git repository directory?` (exit 1). snp keeps that stdout text and writes git's stderr into the crash log.
11. **Pre-push stdin format.** git writes `<local ref> SP <local oid> SP <remote ref> SP <remote oid> LF` (`git:transport.c:1350@v2.54.0`).
    - `<local ref>` can contain spaces (OBS `HEAD@{1 second ago}`).
    - A deletion uses `(delete)` as the local ref.

    snp implements Python `splitlines()` + `rsplit(maxsplit=3)` exactly, and a malformed line gives exit 3 (fail closed). 03 §20.4's "skip malformed lines" is rejected, because for a guard it fails open.
12. **Errata for hook stdin.** 02 §21.3's `stdin(Stdio::null())` is wrong for non-color mode. There the hook's stdin is an **empty pipe** (OBS: `fifo`, 0 bytes). It is `/dev/null` only in pty/color mode (OBS: `character special file`). 04a §14.3 is correct.

### G.1 Inventory of every git invocation in pre-commit 4.6.2

**Conventions.**
- **checked**: `cmd_output[_b]` with its default `check=True`. A non-zero exit raises `pre_commit.util.CalledProcessError`, which ends in exit 3 with `An unexpected error has occurred: CalledProcessError: command: (…)`, unless the table says the error is caught.
- **pipe**: the I/O of `cmd_output_b` (`util.py:81-113`). stdin is an empty pipe, closed at once by `communicate()`. stdout and stderr are captured separately.
- **inherit**: `subprocess.call`/`check_output` with the parent's file descriptors.
- **Argv in error texts**: every argv except G21–G25 goes through `parse_shebang.normalize_cmd` (PATH lookup + shebang), so error texts show the resolved path, e.g. `('/usr/bin/git', …)`. G21–G25 call `subprocess` directly with a bare `'git'`.
- **Env "full"**: the current `os.environ`, including git-exported `GIT_*` and the `PRE_COMMIT_*` variables that `run` sets.
- **Env "nge"**: `no_git_env()` (`git.py:27-48`). It removes every `GIT_*` variable except `GIT_EXEC_PATH`, `GIT_SSH`, `GIT_SSH_COMMAND`, `GIT_SSL_CAINFO`, `GIT_SSL_NO_VERIFY`, `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_*`, `GIT_CONFIG_VALUE_*`, `GIT_HTTP_PROXY_AUTHMETHOD`, `GIT_ALLOW_PROTOCOL` and `GIT_ASKPASS`.

**Run pipeline and startup**

| # | Site | Exact argv after `git` | Failure handling | I/O | Env / cwd |
|---|---|---|---|---|---|
| G01 | `git.py:58` (`get_root`, from `main.py:188`) | `rev-parse --show-cdup` | caught → `FatalError: git failed. Is it installed, and are you in a Git repository directory?` (exit 1) | pipe | full / invocation cwd |
| G02 | `git.py:60-62` | `rev-parse --is-inside-git-dir` | as G01. Output ≠ `false` → FatalError `git toplevel unexpectedly empty! make sure you are not inside the \`.git\` directory of your repository.` | pipe | full / invocation cwd |
| G03 | `run.py:326` | `ls-files --unmerged` | checked. Only "stdout non-blank" is used | pipe | full / root |
| G04 | `run.py:331-333` | `diff --quiet --no-ext-diff <config>` | unchecked. Only `rc == 1` matters | pipe | full / root |
| G05 | `staged_files_only.py:37` → `git.py:147-150` | `diff --no-ext-diff --ignore-submodules --diff-filter=A --name-only -z` | checked | pipe | full / root |
| G06 | `staged_files_only.py:41` (xargs) | `rm --cached -- <ita files…>` | exit code ignored. Partitioned by command length, serial | pipe | full / root |
| G07 | `staged_files_only.py:45` (xargs) | `add --intent-to-add -- <ita files…>` | exit code ignored | pipe | full / root |
| G08 | `staged_files_only.py:52` | `write-tree` | checked | pipe | full / root |
| G09 | `staged_files_only.py:53-57` | `diff-index --ignore-submodules --binary --exit-code --no-color --no-ext-diff <tree> --` | unchecked. rc 0 / 1 / other → FatalError (§7.2) | pipe | full / root |
| G10 | `staged_files_only.py:23,81,95` | `-c submodule.recurse=0 checkout -- .` | checked | pipe | full + `_PRE_COMMIT_SKIP_POST_CHECKOUT=1` / root |
| G11 | `staged_files_only.py:29` | `apply --whitespace=nowarn <patch>` | checked, caught → G12 | pipe | full / root |
| G12 | `staged_files_only.py:32` | `-c core.autocrlf=false apply --whitespace=nowarn <patch>` | checked. On the second (post-rollback) attempt it is not caught (exit 3) | pipe | full / root |
| G13 | `git.py:78` (from `git.py:97,118`) | `rev-parse --git-dir` | checked | pipe | full / `cwd='.'` |
| G14 | `git.py:125` | `write-tree` (merge mode) | checked | pipe | full / root |
| G15 | `git.py:127-130` | `diff --name-only --no-ext-diff -z -m <tree> HEAD MERGE_HEAD --` | checked | pipe | full / root |
| G16 | `git.py:137-142` | `diff --staged --name-only --no-ext-diff -z --diff-filter=ACMRTUXB` | checked | pipe | full / root |
| G17 | `git.py:155` (`run --all-files`, pre-push root push, meta hooks `meta_hooks/check_hooks_apply.py:17`, `meta_hooks/check_useless_excludes.py:33`) | `ls-files -z --deduplicate` | checked (in a meta hook: an uncaught traceback inside the hook subprocess, so the hook fails with exit code 1) | pipe | full / root |
| G18 | `git.py:159-161` | `diff --name-only --no-ext-diff -z <from>...<to>` | checked, caught → G19 | pipe | full / root |
| G19 | `git.py:165` | `diff --name-only --no-ext-diff -z <from>..<to>` | checked | pipe | full / root |
| G20 | `run.py:275-278` | `diff --no-ext-diff --no-textconv --ignore-submodules` | unchecked. stdout is compared byte for byte | pipe | full / root |
| G21 | `run.py:317-320` | `--no-pager diff --no-ext-diff --color=always\|never` | `subprocess.call`. rc ignored | inherit (writes straight to fd 1) | full / root |

**hook-impl (pre-push only)**

| # | Site | Exact argv after `git` | Failure handling | I/O | Env / cwd |
|---|---|---|---|---|---|
| G22 | `hook_impl.py:117` | `cat-file -e <remote_sha>^{commit}` | `subprocess.call`. rc 0 → the remote sha exists locally | inherit (stderr leaks `fatal: Not a valid object name <sha>^{commit}`, see 02 Q11) | full / root |
| G23 | `hook_impl.py:143-146` | `rev-list <local_sha> --topo-order --reverse --not --remotes=<remote_name>` | `check_output`. A failure raises the **stdlib** `subprocess.CalledProcessError` → exit 3 with `An unexpected error has occurred: CalledProcessError: Command '('git', 'rev-list', …)' returned non-zero exit status 128.` | stdout captured, stdin/stderr inherited | full / root |
| G24 | `hook_impl.py:151-152` | `rev-list --max-parents=0 <local_sha>` | as G23 | as G23 | full / root |
| G25 | `hook_impl.py:163-164` | `rev-parse <first_ancestor>^` | as G23 | as G23 | full / root |

**Crash log, install, init-templatedir**

| # | Site | Exact argv after `git` | Failure handling | I/O | Env / cwd |
|---|---|---|---|---|---|
| G26 | `error_handler.py:28` | `--version` | unchecked. stdout is decoded with `backslashreplace` and rstripped. A missing git logs ``Executable `git` not found`` | pipe | full |
| G27 | `install_uninstall.py:51` → `git.py:88` | `rev-parse --git-common-dir` | checked | pipe | full / root |
| G28 | `install_uninstall.py:123` → `git.py:181` | `config core.hooksPath` | unchecked. Non-blank stdout → `[ERROR] Cowardly refusing to install hooks with \`core.hooksPath\` set.` + `hint: \`git config --unset-all core.hooksPath\``, return 1 | pipe | full / root |
| G29 | `init_templatedir.py:30` | `config init.templateDir` | checked, caught | pipe | full |

**Store clone** (`store.clone`, also used by `install-hooks`, `run`, `try-repo` and `gc`)

| # | Site | Exact argv after `git` | Failure handling | I/O | Env / cwd |
|---|---|---|---|---|---|
| G30 | `store.py:199` → `git.py:192` | `-c core.useBuiltinFSMonitor=false init --template= <dir>` | checked | pipe | nge / process cwd |
| G31 | `git.py:193` | `-c core.useBuiltinFSMonitor=false remote add origin <repo>` (`repo` is made absolute if it is a local dir) | checked | pipe | nge / `<dir>` |
| G32 | `store.py:188` | `-c protocol.version=2 fetch origin <ref> --depth=1` | checked, caught → G35 | pipe | nge / `<dir>` |
| G33 | `store.py:189` | `checkout FETCH_HEAD` | checked, caught → G35 | pipe | nge / `<dir>` |
| G34 | `store.py:190-193` | `-c protocol.version=2 submodule update --init --recursive --depth=1` | checked, caught → G35 | pipe | nge / `<dir>` |
| G35 | `store.py:180-182` | `fetch origin --tags`; `checkout <ref>`; `submodule update --init --recursive` | checked (exit 3) | pipe | nge / `<dir>` |

**autoupdate** (prefix `_git` = `git -c core.useBuiltinFSMonitor=false -C <tmp>`, `autoupdate.py:40`)

| # | Site | Exact argv | Failure handling | I/O | Env / cwd |
|---|---|---|---|---|---|
| G36 | `autoupdate.py:48` | G30 + G31 (`init_repo`) | checked | pipe | nge |
| G37 | `autoupdate.py:49` | `_git config extensions.partialClone true` | checked | pipe | **full** (hazard c) |
| G38 | `autoupdate.py:50-53` | `_git fetch origin HEAD --quiet --filter=blob:none --tags` | checked | pipe | **full** |
| G39 | `autoupdate.py:46,56,58` | `_git describe FETCH_HEAD --tags --abbrev=0` (default) or `… --exact` (`--bleeding-edge`), caught → `_git rev-parse FETCH_HEAD` | checked; the first is caught | pipe | **full** |
| G40 | `autoupdate.py:61` → `git.py:239-241` | `git -c core.useBuiltinFSMonitor=false tag --points-at <rev>` (no `-C`; `cwd=tmp`) | checked | pipe | **full** / tmp |
| G41 | `autoupdate.py:65` | `_git rev-parse <rev>` (`--freeze`) | checked | pipe | **full** |
| G42 | `autoupdate.py:71-72` | `_git show <rev>:.pre-commit-hooks.yaml`; `_git checkout <rev> -- .pre-commit-hooks.yaml` | checked, caught | pipe | **full** |

**try-repo**

| # | Site | Exact argv after `git` | Failure handling | I/O | Env / cwd |
|---|---|---|---|---|---|
| G43 | `try_repo.py:26` → `git.py:171` | `ls-remote --exit-code <repo> HEAD` | checked | pipe | full |
| G44 | `try_repo.py:28` → `git.py:176` | `diff --quiet --no-ext-diff HEAD` | unchecked, `== 1` | pipe | full / `<repo>` |
| G45 | `try_repo.py:32-33` | `clone <repo> <shadow>`; `checkout <ref> -b _pc_tmp` | checked | pipe | full |
| G46 | `try_repo.py:35-36` → `git.py:206` | `rev-parse --git-path index` / `objects` | checked | pipe | full / shadow |
| G47 | `try_repo.py:39-44` | G16 (cwd=repo); xargs `add -- <staged…>`; `add -u`; `commit --no-edit --no-gpg-sign -n -minit` (`git.py:196-202`) | checked | pipe | `add`: full + `GIT_INDEX_FILE`/`GIT_OBJECT_DIRECTORY` of the shadow; `commit`: nge + `GIT_AUTHOR_*`/`GIT_COMMITTER_*` = `pre-commit` / `asottile+pre-commit@umich.edu` |

**Not git, but a git-adjacent env:** `languages/golang.py:145` uses `no_git_env` for `go install`.

### G.2 Feature matrix: minimum git per flag or feature

| Feature | Min git | Evidence | Used by | Behavior below the minimum |
|---|---|---|---|---|
| `-c k=v`, `rev-parse --show-cdup/--is-inside-git-dir/--git-dir`, `write-tree`, `diff-index --binary --exit-code`, `apply --whitespace=`, `checkout -- .`, `cat-file -e <x>^{commit}`, `ls-files --unmerged`, `diff -m <t> A B` | ≤ 1.7 | ancient | G01–G15, G22 | — |
| `rev-list --max-parents=0`, `--remotes=<pat>` | ≤ 1.7.x | ancient (not in RelNotes) | G23–G24 | — |
| `ls-remote --exit-code` | 1.7.6 | REL 1.7.6:81 | G43 | — |
| `tag --points-at` | 1.7.10 | REL 1.7.10:128 | G40 | — |
| `git -C <dir>` | 1.8.5 | git history | G37–G42 | unknown option → exit 129 |
| `rev-parse --git-common-dir`, `--git-path` | 2.5.0 (fixed when run from a subdir in 2.7.3/2.8.0) | REL 2.7.3:26 | G27, G46 | returns the literal option (pre-commit's `git.py:92` fallback) |
| `core.hooksPath` | 2.9.0 | REL 2.9.0:127 | G28 | key ignored |
| `diff.renames` defaults to true for `diff`/`log` porcelain | 2.9.0 | REL 2.9.0:7-9 | G05, G16, G18, G20 | — (enables hazard b) |
| `submodule.recurse` | 2.14.0 | REL 2.14.0:73 | G10 | unknown key ignored (harmless) |
| `fetch --filter`, `extensions.partialClone` | 2.17.0 (stabilised in 2.18/2.19) | REL 2.17.0:103-107, 2.19.0:288,363 | G37–G38 | unknown option → exit 129 → autoupdate crashes |
| protocol v2 (`-c protocol.version=2`) | 2.18.0 (default in 2.26, reverted in 2.27, default again in 2.29) | REL 2.18.0, 2.26.0:101, 2.27.0:27, 2.29.0:99 | G32, G34 | `fetch <sha>` usually fails → caught → full clone |
| **ITA paths shown as new (`A`) by `git diff`** | **2.19.0** | REL 2.19.0:9-12 | G05 | ITA never detected → **stash crash** (OBS 2.18.5, §G.3) |
| `--end-of-options` | 2.24.0 | REL 2.24.0:18 | (snp only) | — |
| `diff.relative` config / `--no-relative` | 2.28.0 | REL 2.28.0:15 | G05, G15–G19 (no effect from the root) | — |
| `git init --object-format=` | 2.29.0 | git-init docs | (snp only, §G.7) | unknown option |
| **`ls-files --deduplicate`** | **2.31.0** | REL 2.31.0:57-59 | G17 | `error: unknown option \`deduplicate'` + usage, **exit 129** → pre-commit exit 3 (OBS) |
| `rev-parse --path-format=absolute` | 2.31.0 | REL 2.31.0:38-39 | (snp, lefthook) | exit 129 |
| `GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_<n>` / `GIT_CONFIG_VALUE_<n>` env | 2.31.0 | REL 2.31.0:127-130 | (snp env injection) | silently ignored |
| `safe.directory` ownership check | 2.35.2 (+ backports to 2.30.3 … 2.34.2) | REL 2.30.4:7-12 | every user-repo call | — (exit 128 `detected dubious ownership …`) |
| builtin fsmonitor daemon (`core.fsmonitor=true`) | 2.37.0 Windows/macOS (GfW 2.33–2.35 experimental via `core.useBuiltinFSMonitor`); Linux backend in 2.55.0 | REL 2.37.0:101, 2.55.0:48 | store/autoupdate temp dirs | — |
| `protocol.file.allow=user` default | 2.38.1 (+ backports 2.30.6 … 2.37.4) | REL 2.37.4:23 | store clones with local `file://` submodules | — |
| `diff.srcPrefix` / `diff.dstPrefix` | 2.45.0 | REL 2.45.0:48 | G20/G21 (bytes only) | — |
| `git config set` subcommand (used by pre-commit's docs for config hooks) | 2.46.0 | REL 2.46.0:44 | docs, snp install | — |
| `init.defaultObjectFormat` | 2.49.0 (absent from the v2.48.0 docs, present at v2.49.0) | `Documentation/config/init.adoc@v2.49.0` | G30 (hazard d) | — |
| **config hooks** `hook.<name>.{command,event,enabled}`, `git hook list` | **2.54.0** | REL 2.54.0:67-69; not in `git-hook.adoc@v2.53.0` | hook-impl without `--hook-dir` | keys silently ignored: **the hook never runs** |
| `hook.<name>.parallel`, `hook.jobs`, `hook.<event>.jobs`, `hook.<event>.enabled`; a friendly name equal to an event name is **fatal** | 2.55.0 | REL 2.55.0:7-8; `git:hook.c:201-205@v2.55.0` | config hooks | — |
| Git 3.0 (unreleased as of 2.56): SHA-256 and reftable become the defaults for new repos; `safe.bareRepository` changes | future | `Documentation/BreakingChanges.adoc@v2.56.0:91,121,219` | G30 | — |

### G.3 Observed pre-commit 4.6.2 behavior on old and new git

All transcripts below are stdout+stderr of `pre-commit run …`.

**git 2.30.9 and 2.18.5, `pre-commit run show --all-files`** (identical apart from the path and the 2.18 usage wording `use <n> digits to display SHA-1s`). Exit **3**:
```
An unexpected error has occurred: CalledProcessError: command: ('/opt/git-2.30.9/bin/git', 'ls-files', '-z', '--deduplicate')
return code: 129
stdout: (none)
stderr:
    error: unknown option `deduplicate'
    usage: git ls-files [<options>] [<file>...]
    
        -z                    paths are separated with NUL character
        …(the full option list, 4-space indented; the blank line after "usage:" is the 4-space line "    ")…
        --debug               show debugging data
Check the log at /tmp/pch/pre-commit.log
```
- The default staged run and the stash both work on 2.30.9 and 2.18.5.
- `pre-commit install` + `git commit` work on both.
- `--files` works on both.

**git 2.18.5, an unstaged `git add -N n.txt` plus a staged `b.txt`:**
- `git diff --diff-filter=A --name-only -z` prints nothing.
- n.txt goes into the stash patch and is passed to hooks (`b.txt n.txt`).
- Then:
```
[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...
An unexpected error has occurred: CalledProcessError: command: ('/opt/git-2.18.5/bin/git', '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn', '/tmp/pch/patchN')
return code: 1
stdout: (none)
stderr:
    error: n.txt: already exists in working directory
```
Exit 3. On 2.30.9 and 2.31.8 the same scenario prints `[WARNING] Unstaged intent-to-add files detected.` and passes.

**git 2.31.8:** everything passes, including `--all-files`.

**git 2.43.0, hazard (a): `GIT_DIFF_OPTS=--unified=0 pre-commit run`** with an unstaged change in `a.txt` (staged `b.txt`, hook `true`):
```
[WARNING] Unstaged files detected.
[INFO] Stashing unstaged files to $T/pchome/patch1791407923-2020263.
show.....................................................................Passed
[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...
An unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn', '$T/pchome/patch1791407923-2020263')
return code: 1
stdout: (none)
stderr:
    error: patch failed: a.txt:10
    error: a.txt: patch does not apply
Check the log at $T/pchome/pre-commit.log
```
- Exit 3. Afterwards `git status` shows only `M  b.txt`: the unstaged edit is gone from the worktree.
- The patch has zero-context hunks (`@@ -10 +10 @@`). It applies only with `git apply --unidiff-zero`.
- `GIT_DIFF_OPTS` overrides even an explicit `-U3` on the command line (OBS). The cause is `diff.c:4045-4051@v2.56.0`, which reads the env after option parsing.

**git 2.43.0 and 2.56.0, hazard (b):** `mv old.txt new.txt; git add -N new.txt` (same content) plus a staged change.
- `git diff --no-ext-diff --ignore-submodules --diff-filter=A --name-only -z` → empty, because the pair shows as `R old.txt -> new.txt`.
- The same query with `--no-renames` → `new.txt`.
- pre-commit: `[WARNING] Unstaged files detected.` … `Passed` … `[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...` then `An unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn', …)` / `stderr:` / `    error: new.txt: already exists in working directory`. Exit 3. Afterwards `old.txt` is back in the worktree, so the user's deletion is lost.
- With `diff.renames=false` (through `GIT_CONFIG_COUNT`), the same state passes: `[WARNING] Unstaged intent-to-add files detected.` … `[INFO] Restored changes from …`.
- With `diff.renames=copies`, an ITA copy of a modified file shows as `C` and is missed in the same way (by construction, not run).

**git 2.43.0, hazard (c): `GIT_DIR=$repo/.git pre-commit autoupdate`** (hook repo is a local path):
```
An unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', '-c', 'core.useBuiltinFSMonitor=false', '-C', '/tmp/tmp6pe_43nk', 'fetch', 'origin', 'HEAD', '--quiet', '--filter=blob:none', '--tags')
return code: 128
stdout: (none)
stderr:
    fatal: 'origin' does not appear to be a git repository
    fatal: Could not read from remote repository.
    
    Please make sure you have the correct access rights
    and the repository exists.
```
After this, the user's `.git/config` contains `extensions.partialclone=true`, `remote.origin.promisor=true` and `remote.origin.partialclonefilter=blob:none`. `GIT_DIR` reaches pre-commit through shell exports (e.g. dotfile setups) or `git --git-dir=…` wrappers whose child processes run pre-commit.

**git 2.56.0, hazard (d):** with `init.defaultObjectFormat=sha256` in global config, pre-commit's store sequence (G30–G32, G35) against a SHA-1 remote fails:
- `git init` → `sha256`.
- `-c protocol.version=2 fetch origin <40-hex sha> --depth=1` → `fatal: couldn't find remote ref <sha>` (exit 128).
- `fetch origin --tags` → `fatal: mismatched algorithms: client sha256; server sha1` (exit 128).
- So pre-commit exits 3 for every hook repo.
- `git init --object-format=sha1` works, and `git clone` adapts on its own (OBS).
- `GIT_DEFAULT_HASH` cannot cause this, because `no_git_env` strips it.

**git 2.43.0, a stat-dirty file** (`touch a.txt` with no content change): `diff-index … --exit-code` prints nothing and exits 1. That is the "rc==1 and empty stdout" branch of §7.2, so nothing is stashed. The branch therefore covers stat-only dirtiness as well as the CRLF quirk. Do not "optimise" it away.

**git 2.43.0, other user config probes:**
- `diff.suppressBlankEmpty=true`: the patch contains empty context lines (`$` instead of ` $`), and `git apply` still accepts it (restore OK).
- `diff.autoRefreshIndex=false`: no effect on modification detection.
- `GIT_TEST_DISALLOW_ABBREVIATED_OPTIONS=1 git describe HEAD --tags --exact` → `fatal: disallowed abbreviated or ambiguous option 'exact'` (exit 128).
- `git rm --cached -- '[ab].txt'` with `a.txt`, `b.txt` and a literal `[ab].txt` all in the index removes only the literal file (git matches literal paths first). Without the literal file, `git rm --cached -n -- '[ab].txt'` lists `a.txt` and `b.txt`. `git add -N` is a no-op for already-tracked paths. So the glob magic in G06/G07 is harmless in practice.

### G.4 snp git version policy (decision)

**Bands**

| git version | Behavior | Exit |
|---|---|---|
| unparseable `git --version` output | continue silently. Record the raw text in the crash log | — |
| ≥ 2.31.0 | continue silently | — |
| 2.19.0 ≤ v < 2.31.0 | one warning per process to **stderr** (see below), then continue with capability fallbacks (§G.10). Silenced when `SNP_ALLOW_OLD_GIT` is non-empty | — |
| < 2.19.0 | refuse before any git call that writes (no stash, no hooks, no legacy hook) | 1 |

**Exact texts** (`<V>` = the parsed `major.minor.patch`; `<git>` = the resolved executable path):
- Refusal, printed through the standard FatalError path (stdout, then the usual `Check the log at …` trailer, exit 1):
  `An error has occurred: FatalError: git <V> is not supported: snp requires git 2.19.0 or newer (2.31.0 or newer recommended; found <git>).`
- Warning (stderr; `[WARNING]` is colored like the other log lines, §4):
  `[WARNING] git <V> is older than 2.31.0, the oldest git version snp is tested with. Set SNP_ALLOW_OLD_GIT=1 to silence this warning.`
- `snp install --config-hooks` (or any feature that needs config hooks) below 2.54.0: FatalError, exit 1:
  `An error has occurred: FatalError: config-based hooks require git 2.54.0 or newer (found git <V>).`

**When the check runs**
- For every command **not** in pre-commit's `COMMANDS_NO_GIT` (`main.py:44-47`: clean, gc, hazmat, init-templatedir, sample-config, validate-config, validate-manifest). That is: run, hook-impl, install, install-hooks, uninstall, autoupdate, migrate-config, try-repo.
- It runs **after** `_adjust_args_and_chdir`'s G01/G02 succeed. A missing git, "not a repository" and `safe.directory` errors therefore keep pre-commit's exact texts and their order.
- For hook-impl, it runs before the legacy hook. Failing closed blocks the commit. The user can bypass with `git commit --no-verify`.

**Parsing `git --version`**
- stdout must start with `git version `.
- Then match `^(\d+)\.(\d+)(?:\.(\d+))?` on the rest. A missing patch number counts as 0. Ignore any suffix.

| Raw output | Parsed |
|---|---|
| `git version 2.43.0` | 2.43.0 |
| `git version 2.39.5 (Apple Git-154)` | 2.39.5 |
| `git version 2.47.1.windows.2` | 2.47.1 |
| `git version 2.56.0.rc0` (git's `GIT-VERSION-GEN:55` turns `-` into `.`) | 2.56.0 |
| `git version 2.56.0.123.gabcdef0.dirty` | 2.56.0 |

**Cost** (snp is a hot-path tool, and spawning git costs about 1 ms on Linux and 10–30 ms on Windows):
- Resolve `git` once (PATH + PATHEXT).
- Look it up in a small cache file in the snp store, keyed by `(canonical path, size, mtime_ns, GIT_EXEC_PATH)`.
- **Trust only passing entries** (≥ 2.31). For a cached failing or below-floor entry, re-spawn `git --version` before warning or refusing. Shims (scoop, chocolatey, asdf) can update git without changing the shim's own mtime, and a stale below-floor entry must never block an upgraded user.
- On a cache miss, spawn `git --version` on a thread at the same time as G01. Write the cache atomically (temp file + rename) and ignore write errors (a read-only store is fine).
- **Reactive backstop:** if any git call exits **129** (a usage error, independent of locale), and the version is unknown or cached, re-check the version before reporting.

**CI matrix** (refines 10 §L6 / Q8):
- 2.18.5 (refusal test)
- 2.30.x (warning band smoke test; Debian 11 ships 2.30.2)
- 2.31.0 (floor)
- 2.34 (Ubuntu 22.04)
- 2.39 (Debian 12, Apple Git)
- 2.43 (Ubuntu 24.04)
- 2.54.0 (first with config hooks)
- 2.55.x (parallel keys and the fatal name collision)
- the latest release
- the latest Git for Windows

Old gits build in about a minute each from kernel.org tarballs with the flags in the setup notes above. `NO_CURL` means no https remotes, which is fine for local fixtures.

### G.5 git ≥ 2.54 config-based hooks: exact invocation semantics

**Configuration model** (`Documentation/config/hook.adoc@v2.54.0`, `git:hook.c:127-302@v2.54.0`)

| Key | Semantics |
|---|---|
| `hook.<friendly>.command` | A path or a shell one-liner. Last one wins. |
| `hook.<friendly>.event` | Multi-valued. An empty value removes `<friendly>` from every event seen so far. Re-adding moves the hook to the position of its **last** occurrence ("Re-insert if necessary to preserve last-seen order", `git:hook.c:170@v2.54.0`). Events git doesn't know are ignored (for wrappers). |
| `hook.<friendly>.enabled` | Boolean, default true. `false` keeps the hook listed but skips it (`git:hook.c:438@v2.54.0`). Unrecognised values are ignored. |
| (2.55+) `hook.<friendly>.parallel` | Default false. |
| (2.55+) `hook.<event>.enabled` | false disables **all** hooks for the event, including the hooks-dir script. |
| (2.55+) `hook.jobs`, `hook.<event>.jobs` | `-1` = CPU count. |
| `<friendly>` naming | `<friendly>` is the config subsection: everything between the first and the last dot. So `hook.pre-commit.pre-commit.event` has friendly name `pre-commit.pre-commit`. That is pre-commit's documented scheme `pre-commit.<hook-type>`, and 03 adopts it. |

**Errors when a hook event fires** (the whole git command aborts with exit 128):
- An event with no command: `fatal: 'hook.nocmd.command' must be configured or 'hook.nocmd.event' must be removed; aborting.` (OBS; `git:hook.c:279@v2.54.0`). A *disabled* hook without a command only warns: `warning: disabled hook '<n>' has no command configured`.
- (2.55+) A friendly name equal to a known event name: `fatal: hook friendly-name 'pre-commit' collides with a known event name; please choose a different friendly-name` (OBS on 2.56.0, the commit is aborted).
- (2.55+) A friendly name equal to its own event value (unknown events only): warning `hook friendly-name '<n>' is the same as its event; this may cause ambiguity with hook.<n>.enabled`.
- 2.54 accepts both silently.

**Execution** (`git:hook.c:425-486@v2.54.0`; OBS on 2.56.0 with an argv/env probe)

| Aspect | Exact behavior |
|---|---|
| Order | Configured hooks in config-parse order (system → global → local → worktree → `-c`), then the hooks-dir script **last** (`git:hook.c:399-402`). Every hook runs, even after one fails. |
| argv | `cp->use_shell = true; args = [command, hook_args…]` (`:467-478`). `prepare_shell_cmd` (`git:run-command.c:288-308`): if `command` contains any of `` \|&;<>()$`\"' \t\n*?[#~=% ``, run `sh -c '<command> "$@"' '<command>' <hook_args…>`, or `sh -c '<command>' '<command>'` when there are no args. Otherwise exec `command` directly with `hook_args`. **git inserts no `--`.** |
| Shell | POSIX: `SHELL_PATH` (normally `/bin/sh`). Git for Windows: its bundled `sh`. `PATH` is the env git was started with. GUI clients often have a minimal PATH, so a bare `snp` may not be found: `sh: snp: not found`, exit 127, and the commit is blocked. |
| Hook args | Identical to hooks-dir scripts. OBS: pre-commit none; prepare-commit-msg `.git/COMMIT_EDITMSG message`; commit-msg `.git/COMMIT_EDITMSG`; post-checkout `<old> <new> <flag>` (also fired by `git checkout -- .`, flag `0`); pre-push `<remote name> <url>`. |
| cwd | Worktree root (OBS: `/tmp/r` for a commit run from `sub/`). `$GIT_DIR` in bare repos and for push-side hooks (`githooks.adoc@v2.54.0`). |
| env | Inherited from git, plus what git exports. OBS for commit: `GIT_INDEX_FILE=.git/index`, or `/abs/.git/index.lock` for `commit -a`, or `/abs/.git/next-index-<pid>.lock` for `commit <paths>`; `GIT_EDITOR=:`; `GIT_AUTHOR_NAME/EMAIL/DATE`; `GIT_EXEC_PATH`; `GIT_PREFIX=<subdir>/` (empty at the root). For push: `GIT_EXEC_PATH`, `GIT_PREFIX`. `git -c k=v commit` adds `GIT_CONFIG_PARAMETERS`, which pre-commit's own git calls also see. |
| stdin | `/dev/null` (`cp->no_stdin = 1`, `:440`), except: pre-push gets a **pipe** that git fills separately for **each** hook with the full ref list (`feed_pipe` + per-hook `pre_push_hook_data_alloc`, `git:transport.c:1325-1368`), and hooks git feeds from a file (`path_to_stdin`, e.g. post-rewrite) get that file. |
| stdout | Redirected to git's **stderr** for every hook except pre-push (`RUN_HOOKS_OPT_INIT .stdout_to_stderr = 1`, `hook.h:155-160@v2.54.0`; pre-push sets 0, `git:transport.c:1397`). With 2.55+ parallel execution (jobs > 1), stdout is merged into stderr for pre-push too, and output is buffered per hook. |
| Exit status | `rc \|= result` per hook (`:508`); a spawn failure adds `\|= 1` (`:495`). The commit or push is rejected if the OR is non-zero. |
| Parallelism (2.55+) | Serial unless **every** configured hook for the event has `parallel = true`, or `-j`/`hook.jobs` is set together with that. pre-commit, prepare-commit-msg, commit-msg, post-commit, post-checkout, applypatch-msg and push-to-checkout are always serial. pre-push, pre-rebase, post-merge, post-rewrite and pre-merge-commit **can** run in parallel. |

**Interplay with `hook-impl`** (`main.py:356-365,408`; `hook_impl.py:30-37`)
- `rest` is `argparse.REMAINDER`. Behavior OBS identically on CPython 3.12.3 and 3.14.8:
  - `-- a` → `['--','a']`
  - `a b` → `['a','b']`
  - `--` → `['--']`
  - `-- -- a` → `['--','--','a']`
- `args = rest[1:]` therefore needs the configured command to end in ` --`. pre-commit's docs do this, and every command snp writes must too.
- Missing `--` with a 0-arg hook type (pre-commit, pre-merge-commit, post-commit) is harmless. With any other type, the arity check fails (OBS texts in §G.0 item 3; stderr, exit 1, and git rejects the commit or push).
- There is no `--hook-dir`, so `hook_dir is None`: no `<type>.legacy` handling. `--config` defaults to `.pre-commit-config.yaml`, relative to the worktree root.
- pre-push stdin is read in full and parsed as in §16.3 (see §G.9 item 2).
- OBS end to end on git 2.56.0 + pre-commit 4.6.2:
  - The recommended config produced the normal `show-args....Failed` blocks for pre-commit and pre-push. For pre-push the env had `PRE_COMMIT_FROM_REF`/`TO_REF`/`REMOTE_NAME`, and `GIT_INDEX_FILE` was unset.
  - With a global `--skip-on-missing-config` config hook, a repo without a config printed ``\`.pre-commit-config.yaml\` config file not found. Skipping \`pre-commit\`.`` and the commit succeeded.
  - Without `--skip-on-missing-config`, it printed the 4-line `No .pre-commit-config.yaml file was found` block and the commit failed.
- A traditional `pre-commit install` **plus** a config hook ran the hooks **twice** (two `Failed` blocks, config hook first).

**Requirements for snp**
1. `hook-impl`: keep `rest[1:]` (parity). Leniency is Open question G-2.
2. `snp install --config-hooks` (03 §6/§20.3), requires git ≥ 2.54 (§G.4):
   - Friendly name `pre-commit.<type>`. **Never** a bare event name (fatal on ≥ 2.55).
   - Never set `parallel` (concurrent stashes in one worktree would race).
   - The command is `<sh-quoted absolute path of snp> hook-impl --hook-type <type> [--config=<sh-quoted cfg>] [--skip-on-missing-config] --`, with no trailing comment. git appends ` "$@"`, so a `# …` would swallow the hook args.
   - Write the event key with `git config set --all hook.pre-commit.<type>.event <type>` or an equivalent, so stale duplicates are replaced.
3. Double-run detection:
   - `snp install` (traditional), when git ≥ 2.54 and `git config -z --get-regexp '^hook\.pre-commit\..*\.event$'` lists the type: print `[WARNING] git config also runs a pre-commit config hook (hook.pre-commit.<type>) for <type>; hooks will run twice.`
   - `snp install --config-hooks`, when a pre-commit/snp-generated script exists in the hooks dir: print the mirror-image warning, and offer `--overwrite` to remove the script.
4. `hook-impl` must work from **both** sources unchanged, because pre-commit's own scripts and configs may call `snp` through the `pre-commit` shim name.

### G.6 How user git config and env change pre-commit's own git calls

**General rules.**
- Every call in the run pipeline inherits the **full** env and all config scopes. That is intended: `GIT_INDEX_FILE`, `GIT_DIR` and `GIT_WORK_TREE` from git must be honoured.
- `GIT_CONFIG_PARAMETERS` (from `git -c …`) and `GIT_CONFIG_COUNT/KEY/VALUE` inject config into every call. git reads `GIT_CONFIG_COUNT` first and `GIT_CONFIG_PARAMETERS` last, and the later value wins (`config.c:734-790@v2.56.0`).
- Exit-code semantics are locale-independent: 0, 1 (diff `--exit-code`/`--quiet`), 128 (fatal), 129 (usage). Message texts are localised. **Never parse git stderr for logic.**

| Call(s) | Config / env that change output or exit code | Neutralised by pre-commit | Effect / verdict |
|---|---|---|---|
| G01, G02, G13, G27 (rev-parse) | `GIT_DIR`, `GIT_WORK_TREE`, `core.worktree`, `core.bare`, `GIT_CEILING_DIRECTORIES`, `GIT_DISCOVERY_ACROSS_FILESYSTEM`, `GIT_COMMON_DIR`, `safe.directory` (exit 128, `fatal: detected dubious ownership in repository at '<path>'` + `To add an exception for this directory, call:` + `\tgit config --global --add safe.directory <path>`, `setup.c:1476-1481@v2.56.0`), `safe.bareRepository` (2.38+) | no (intended) | Every failure becomes `git failed. Is it installed, and are you in a Git repository directory?`. Output is never quoted. pre-commit `.strip()`s it, so trailing whitespace in a path is lost (bug-compat). |
| G03 `ls-files --unmerged` | none relevant (`core.quotePath` quotes, but only emptiness is used) | – | immune |
| G04 `diff --quiet <config>` | `core.fileMode` (mode-only change → 1), `core.autocrlf`/`core.eol`/`.gitattributes` text/eol/filter (CRLF-only edits may give 0), textconv (not disabled), `GIT_ICASE_PATHSPECS`/`GIT_GLOB_PATHSPECS`/`GIT_LITERAL_PATHSPECS` (the config path is a pathspec; literal match first), `GIT_EXTERNAL_DIFF`/`diff.external` | `--no-ext-diff` | parity: same argv, same env |
| G05 ITA query | **`diff.renames` (default true), `diff.renames=copies`, `diff.renameLimit`** → an ITA path paired as `R`/`C` is not `A` → **hazard (b)**. `diff.orderFile` → order only. Pathspec env: n/a. `core.quotePath`/`color.*`: n/a (`-z`, name-only) | `--no-ext-diff --ignore-submodules` only | **snp adds `--no-renames`** (§G.7) |
| G06/G07 `rm --cached` / `add -N` | `GIT_*_PATHSPECS`, `core.ignoreCase` (pathspec matching; literal-first makes glob magic harmless, OBS). Index writes fire **post-index-change** hooks, hooks-dir or 2.54+ config | `--` | exit ignored; parity |
| G08/G14 `write-tree` | none (an unmerged index → 128, prevented by G03) | – | – |
| G09 stash `diff-index` | **`GIT_DIFF_OPTS`** (`-u<N>`/`--unified=<N>`, overrides `-U`) → **hazard (a)**. `diff.suppressBlankEmpty` (harmless, OBS). `diff.<driver>.binary` / `-diff` attribute and `core.bigFileThreshold` (binary hunks, fine with `--binary`). `core.autocrlf`/`eol`/clean filters, e.g. LFS (pointer diffs; CRLF quirk → rc 1 with empty output). stat-dirty entries (rc 1, empty output, OBS). `core.fileMode`. **Ignored** because it is plumbing (OBS): `diff.noprefix`, `diff.mnemonicPrefix`, `diff.srcPrefix`/`dstPrefix`, `diff.context`, `diff.interHunkContext`, `diff.algorithm`, `diff.renames`, `diff.relative`, `color.ui=always` | `--no-color --no-ext-diff --binary --ignore-submodules` | **snp removes `GIT_DIFF_OPTS` from this call's env** (§G.7) |
| G10 `checkout -- .` | `submodule.recurse` (neutralised), `core.autocrlf`/`eol`, smudge filters (`filter.lfs.*`: can download LFS objects; `filter.<d>.required` errors), `core.sparseCheckout`, `checkout.workers`, `core.symlinks`, `core.fileMode`. Fires **post-checkout** hooks with flag `0`, hooks-dir and 2.54+ config: pre-commit/snp's own hook-impl returns early because of `_PRE_COMMIT_SKIP_POST_CHECKOUT=1`, but other tools' post-checkout hooks do run. Also fires post-index-change | `-c submodule.recurse=0` | parity |
| G11/G12 `apply` | `core.autocrlf` (handled by the retry), `apply.whitespace` (overridden), `apply.ignoreWhitespace` (looser context matching), `core.whitespace` (warnings only), smudge/eol conversion of written files | `--whitespace=nowarn`; retry with `core.autocrlf=false` | parity |
| G15 merge-conflict diff | `diff.orderFile`, `diff.ignoreSubmodules`/`submodule.<n>.ignore`, `diff.external` (neutralised) | `--no-ext-diff -z` | parity; result is a set anyway |
| G16 staged list | `diff.orderFile` (**order**, OBS). `diff.ignoreSubmodules`/`submodule.<n>.ignore=all` (staged gitlink changes may vanish). `diff.renames`/`renameLimit` (**set and order unchanged**, OBS; CPU only). `diff.relative` (no effect from the root). `color.ui=always` (no effect on `--name-only`, OBS). `core.quotePath` (no effect, `-z`, OBS). `GIT_INDEX_FILE` (critical, intended) | `--no-ext-diff -z`, `--diff-filter` | snp may add `--no-renames` (perf, output-neutral) |
| G17 `ls-files -z --deduplicate` | `core.sparseCheckout`/`index.sparse` (skip-worktree entries are listed, the sparse index is expanded; then dropped by `lexists`), `GIT_INDEX_FILE` | `-z` | parity |
| G18/G19 changed files | **`diff.renames`**: off → the old path of a rename is also listed (usually dropped by `lexists`, but kept if a file exists at that path). `diff.orderFile` (order). Merge base: none → `fatal: <a>...<b>: no merge base` exit 128 (OBS) → `..` retry. Several → stderr `warning: <a>...<b>: multiple merge bases, using <oid>` (`builtin/diff.c:640@v2.56.0`), captured and dropped | `--no-ext-diff -z` | **do not** add `--no-renames` (it would change output) |
| G20 modification-detection diff | Every UI diff key and `GIT_DIFF_OPTS` change the bytes **consistently** (OBS: `diff.noprefix`, `diff.context=0`, `color.ui=always` escape codes). `diff.autoRefreshIndex=false`: no effect (OBS). `core.fileMode=false` hides chmod-only fixes. Side effect: when stat-dirty entries were seen, `git diff` rewrites the index (`refresh_index_quietly`, `builtin/diff.c:237-250,649-650@v2.56.0`; **not** controlled by `GIT_OPTIONAL_LOCKS`), which fires post-index-change hooks | `--no-ext-diff --no-textconv --ignore-submodules` | harmless (compared for equality only) |
| G21 show-diff | Display only: `GIT_DIFF_OPTS`, prefixes, `diff.context`, **textconv (not disabled)**, `color.diff.*` slots, `diff.colorMoved` | `--no-pager --no-ext-diff --color=` | parity |
| G22–G25 pre-push | `GIT_DIR` etc. With `--remotes=<remote_name>` where the remote name is a URL (push to a URL), the pattern matches nothing, so all ancestors count as new. In a shallow clone, boundary commits act as roots (`--max-parents=0`) → the `all_files` path | – | parity |
| G26 `--version` | none | – | – |
| G28/G29 config reads | all config scopes, `include.path`/`includeIf`, `GIT_CONFIG_*` | – | parity |
| G30–G35 store | User global/system config still applies (only `GIT_*` **env** is stripped): `url.<base>.insteadOf`, `http.*`, `credential.*`, `protocol.<name>.allow`, `transfer/fetch.fsckObjects`, `core.fsmonitor` (a daemon in a long-lived store dir: Windows/macOS ≥ 2.37, Linux ≥ 2.55), **`init.defaultObjectFormat` (≥ 2.49) → hazard (d)**, `init.defaultRefFormat` (reftable, harmless), `core.autocrlf` (hook scripts checked out with CRLF on Windows), `core.symlinks`. `no_git_env` keeps `GIT_CONFIG_COUNT/KEY/VALUE` but **drops** `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, `GIT_CONFIG_NOSYSTEM`, `GIT_DEFAULT_HASH`, `GIT_TERMINAL_PROMPT`, `GIT_LFS_SKIP_SMUDGE`, `GIT_SSH_VARIANT`, `GIT_PROXY_COMMAND` and `GIT_TRACE*` (asymmetric; see G-6) | `-c core.useBuiltinFSMonitor=false` on G30/G31 only; `-c protocol.version=2` | snp: §G.7 rows 4–6 |
| G36–G42 autoupdate | **Full env** (hazard c: `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` …). `GIT_TEST_DISALLOW_ABBREVIATED_OPTIONS` → `describe … --exact` dies, so `--bleeding-edge` silently picks the sha even when HEAD is exactly tagged. Server without filter support → `warning: filtering not recognized by server, ignoring` (stderr captured), then a full fetch | `-c core.useBuiltinFSMonitor=false` | snp: sanitized env (03 §20.2); spell `--exact-match` (same semantics, and the failure is caught, so the argv is never shown) |
| G43–G47 try-repo | Full env for `clone`/`checkout` (the same class as hazard c); intended env for the shadow `add` | – | snp: sanitized env except the deliberate `GIT_INDEX_FILE`/`GIT_OBJECT_DIRECTORY` |

**Related: pre-commit-hooks** (non-`-z` callers, 09 §3; re-verified here on 2.43.0):
- `git diff --staged --name-only --diff-filter=A` prints `"caf\303\251.txt"` with the default `core.quotePath` and `café.txt` with `quotePath=false`.
- A staged `git mv b.txt b2.txt` is missing with the default `diff.renames` and listed (`b2.txt`) with `diff.renames=false`.
- `git status --porcelain=v2 -z` (destroyed-symlinks) shows a staged rename as a `2 R.` record by default, and as `1 D.` + `1 A.` records when `status.renames=false` **or** `diff.renames=false`. `status.showUntrackedFiles=no` hides `?` records.
- 09's rule stands: native ports must spawn the **identical** argv and must not add `-z`, `core.quotePath` or `--no-renames`.

### G.7 Extra neutralisation: what snp adds and what it doesn't

**How each change is applied.** "argv" changes the command line. pre-commit prints the argv in `CalledProcessError` texts, so use argv only where the call practically never fails, or where the failure is caught. "env" uses `Command::env_remove` or `GIT_CONFIG_COUNT` injection, which is invisible in error texts.

| # | Change | Where | How | Observable difference vs pre-commit |
|---|---|---|---|---|
| 1 | `--no-renames` | G05 (ITA query) only | argv: `diff --no-renames --no-ext-diff --ignore-submodules --diff-filter=A --name-only -z` | Only in the hazard-(b) case: snp prints `[WARNING] Unstaged intent-to-add files detected.` and succeeds, where pre-commit crashes. Exists in every supported git. |
| 2 | remove `GIT_DIFF_OPTS` | G09 (stash `diff-index`) only. **Not** G20/G21 | env | Only when `GIT_DIFF_OPTS` would have broken the restore (hazard a). A non-zero-context `GIT_DIFF_OPTS` (e.g. `-u1`) also changes the patch file's bytes; that is not user-visible except in the kept `patch*` file. |
| 3 | `--no-renames` | G16 (staged list) | argv | None: same set and order (OBS). Saves rename-detection CPU on large `git mv`s. Optional; prek does the same (06 §3, `git diff --raw --no-renames --no-relative -z … --cached`). |
| 4 | inject `core.fsmonitor=` (empty) **and** `core.useBuiltinFSMonitor=false` | every call on an **snp-owned** repo (store G30–G35, autoupdate tmp G36–G42, try-repo tmp/shadow); never the user's repo | env: append to `GIT_CONFIG_COUNT` (git ≥ 2.31; skip on older git, which has no upstream daemon) | None. It prevents fsmonitor daemons being spawned in store and temp dirs (#2046-class `PermissionError: [WinError 32]` on temp-dir cleanup, and leaked daemons on macOS, and on Linux from 2.55). An empty value means "off" on every version: on ≥ 2.36, `maybe_bool("")` = false (OBS `git -c core.fsmonitor= config --type=bool core.fsmonitor` → `false`); on < 2.36 an empty hook path means disabled (OBS rc 0 on 2.18.5/2.30.9/2.31.8). Do **not** use `core.fsmonitor=false` before 2.36: it is read as a hook **path** named `false`. |
| 5 | sanitized env (`no_git_env` semantics) | autoupdate G37–G42, try-repo G45 (clone/checkout), all store calls | env | Only where pre-commit corrupts or misreads (hazard c). Matches 03 §20.2. |
| 6 | object-format selection | G30 (store `init`) | argv on G30 only: if `rev` is 64-hex, `--object-format=sha256`; else `--object-format=sha1`. If fetch fails and git ≥ 2.29, re-init once with the other format and retry the whole sequence | None on SHA-1 remotes with default config. It fixes hazard (d) and is required for Git 3.0. The extra G30 flag shows only in an `init` failure text (essentially never). Use `init.defaultObjectFormat` detection instead if strict argv parity of G30 is demanded (Open question G-8). |
| 7 | `--exact-match` instead of `--exact` | G39 | argv (failure is caught, never printed) | None, except it survives `GIT_TEST_DISALLOW_ABBREVIATED_OPTIONS`. |
| — | **Not added**: `-c core.quotePath=false` | anywhere | – | Pointless: every pre-commit path list is `-z`, and nothing else prints paths. Harmful in pre-commit-hooks ports (09). |
| — | **Not added**: `--no-renames` on G18/G19 (changed files) | – | – | Would change the file list (old paths). |
| — | **Not added**: `--no-relative` | – | – | Pointless (snp runs git from the root, as pre-commit does), and it is an unknown option below 2.28. |
| — | **Not added**: `--literal-pathspecs` / `GIT_LITERAL_PATHSPECS` | G04, G06, G07 | – | Literal-first matching already protects these calls (OBS), and it would turn G07's no-op into exit 128. |
| — | **Not added**: `-c core.fsmonitor=…` in the user repo | – | – | pre-commit keeps the user's fsmonitor, and disabling it would only slow large repos. |
| — | **Not added**: `GIT_OPTIONAL_LOCKS=0` | – | – | `git diff` ignores it, and pre-commit never runs its git calls while a hook is running. Revisit only for an opt-in parallel-hooks mode. |
| — | **Not added**: `-c core.useBuiltinFSMonitor=false` on user-repo calls (06 §8 proposed it for every call) | – | – | It would disable legacy GfW fsmonitor in the user's repo and change error-text argv. |

### G.8 Platform differences relevant to this gap

| Topic | Linux | macOS | Windows |
|---|---|---|---|
| `git --version` form | `2.43.0` | `2.39.5 (Apple Git-154)` | `2.47.1.windows.2` |
| Typical shipped git (2026) | Ubuntu 22.04: 2.34.1, 24.04: 2.43.0; Debian 11: 2.30.2 (**warning band**), 12: 2.39.x; Ubuntu 20.04: 2.25.1 (warning band) | Xcode CLT Apple Git 2.39+ on current macOS; older macOS/Xcode may ship 2.30–2.32 | GfW tracks upstream closely |
| Builtin fsmonitor daemon | 2.55+ (REL), but missing in the Alpine 2.56.0 build (`fatal: fsmonitor--daemon not supported on this platform`) | 2.37+ (FSEvents) | 2.37+ upstream; GfW 2.33–2.35 via `core.useBuiltinFSMonitor`. The daemon holds a handle on the worktree, so `TemporaryDirectory` cleanup fails (pre-commit #2046, `PermissionError: [WinError 32]`) |
| `core.useBuiltinFSMonitor` | ignored | ignored | GfW: honoured only when `core.fsmonitor` is unset or a non-bool string. A true value prints advice `core.useBuiltinFSMonitor=true is deprecated;please set core.fsmonitor=true instead` (verbatim; note the missing space; `fsmonitor-settings.c:104-127`, GfW main) |
| Config-hook shell | `/bin/sh` | `/bin/sh` | GfW's bundled `sh` (MSYS2). `snp` must be on that PATH, or written as an absolute path in POSIX form, quoted |
| Path normalisation in git output | – | `core.precomposeUnicode=true` (set by `git init` on macOS) → NFC names in all `-z` lists. Pass them through unchanged | `core.ignoreCase=true`, `core.symlinks=false`; `core.autocrlf=true` is common (installer default), so the §7.2 CRLF branch and the G12 retry are routine |
| `safe.directory` triggers | other-uid checkouts (Docker bind mounts, CI) | same | FAT/exFAT and network shares (no ownership info) |

### G.9 Resolution of cross-document contradictions related to this gap

1. **fsmonitor flag** (06 §8 vs 03 §20.2 vs the task text).
   - Facts (SRC): `NO_FS_MONITOR` is defined at `git.py:16`. It is used **only** in `init_repo` (`git.py:189-193`: `init`, `remote add`; reached from `store.py:199`, `autoupdate.py:48` and the try-repo store clone), in `get_best_candidate_tag` (`git.py:239-241`), and in autoupdate's `_git` prefix (`autoupdate.py:40`).
   - It is **not** on the store's fetch/checkout/submodule calls (`store.py:202-203`) and not on any run-pipeline or hook-impl call. The task's citation `git.py:129,302,353` is wrong (git.py has 245 lines).
   - Resolution: 06 §8 ("always adds") is rejected (§G.7, last row). 03 §20.2 ("store and autoupdate commands") is too broad for argv parity: copy pre-commit's argv exactly, and get the protective effect from the env injection in §G.7 row 4.
2. **Pre-push stdin parsing** (03 §20.4 vs 02 §16.3).
   - git's format is fixed (`git:transport.c:1350@v2.54.0`). The local field may contain spaces (OBS). Config hooks receive identical bytes.
   - Resolution: **02 §16.3 is normative.** Decode strict UTF-8 (failure → exit 3, `UnicodeDecodeError`). Split lines with Python `str.splitlines()` boundaries: `\n`, `\r\n`, `\r`, `\x0b`, `\x0c`, `\x1c`, `\x1d`, `\x1e`, `\x85`, ` `, ` `. Split fields with `rsplit(None, 3)`: split on runs of Python-whitespace from the right, at most 3 times; trailing whitespace is dropped; leading whitespace of the remaining left part is **kept** (OBS `'  x  y\tz w  '.rsplit(maxsplit=3)` → `['  x','y','z','w']`).
   - Python-whitespace = Rust `char::is_whitespace` **plus** U+001C..U+001F (OBS: CPython `str.isspace()` set).
   - Fewer than 4 fields → exit 3 with `An unexpected error has occurred: ValueError: not enough values to unpack (expected 4, got <n>)` (OBS, `got 2`). Only lines up to the first one that yields a namespace are parsed.
   - 03 §20.4's `\n` + strip-`\r` + ASCII `rsplitn(4,' ')` + "skip malformed lines" is rejected. It differs on Unicode whitespace and other line separators, and skipping is fail-open for a push guard.
   - pre-commit misparses a remote branch name containing non-ASCII whitespace such as U+00A0 or U+2028. git allows these in ref names. Keeping or fixing that is Open question G-4.
3. **Hook stdin** (02 §21.3 and 06 §8 vs 04a §14.3).
   - OBS with pre-commit 4.6.2 on 2.43.0, hook `stat -L -c %F /dev/stdin; cat | wc -c`: `--color never` → `fifo` / `0`; `--color always` → `character special file` / `0`.
   - **Erratum for 02 §21.3:** replace "`stdin(Stdio::null())`" with "`stdin(Stdio::piped())`, and drop the `ChildStdin` right after `spawn()` (an empty FIFO, immediate EOF) in non-color mode; `Stdio::null()` only in pty/color mode". 02 TL;DR #9 and 04a §14.3 are right. 06 §8 is wrong in the same way.
   - For snp's **own git calls**, stdin does not matter. pre-commit gives git an empty pipe (`cmd_output_b`) or the inherited stdin (G21–G25; already drained in pre-push), and none of these git invocations read stdin. snp may use `Stdio::null()` for all of them.
4. **02 Q13 answered** (§G.0 item 3, §G.5). git passes no `--`. The `--` belongs to the configured command. Keep `rest[1:]`.
5. **10 Q8** ("≥ 2.31 (warn below)") is refined by §G.4: refuse below 2.19.0, warn from 2.19.0 to 2.30.x. **02 §20** ("`ls-files --deduplicate` … requires git ≥ 2.31") is correct about the requirement. Note that 4.6.2 does not enforce it: it only crashes when G17 runs.
6. **02 §21.13** (merge the rev-parse calls into one). `--show-cdup --is-inside-git-dir --git-dir --git-common-dir` in one call gives one line per flag. `--path-format=absolute` needs git 2.31 (fall back to separate calls in the warning band). Paths containing `\n` make line parsing ambiguous. Detect a wrong line count and fall back to separate calls. In a bare repo, `--show-cdup` prints nothing (03 §19).
7. **Stash restore algorithm** (02 §21.10 vs 06 §7/§8): not decided here. Whatever is chosen, any `git diff-index` used to produce **or verify** a patch must run without `GIT_DIFF_OPTS` (hazard a). A `read-tree`/`reset`-based restore is additionally exposed to `core.sparseCheckout`, `submodule.recurse` and post-index-change/post-checkout hooks, which pre-commit's algorithm handles only through `_PRE_COMMIT_SKIP_POST_CHECKOUT`.

### G.10 Implementation notes for snp (Rust)

- **One `Git` handle per process.**
  - Resolve the executable once (PATH; PATHEXT on Windows). Reproduce `normalize_cmd`'s result (including shebang expansion when `git` is a script wrapper), so `CalledProcessError` texts show the same `command: (…)` tuple.
  - Spawn by absolute path, with no shell.
  - Keep `GitVersion`/`GitCaps` in a `OnceLock`:
    ```rust
    pub struct GitVersion { pub major: u16, pub minor: u16, pub patch: u16 }   // Ord derives give version order
    pub struct GitCaps {
        pub version: Option<GitVersion>,   // None = unparseable → treat as newest
        pub ita_as_new: bool,              // >= 2.19.0  (else refuse)
        pub init_object_format: bool,      // >= 2.29.0
        pub ls_files_dedup: bool,          // >= 2.31.0  (else ls-files -z + adjacent dedup in Rust)
        pub path_format: bool,             // >= 2.31.0  (else separate rev-parse calls)
        pub config_env: bool,              // >= 2.31.0  (GIT_CONFIG_COUNT injection; else skip)
        pub config_hooks: bool,            // >= 2.54.0
        pub hook_parallel_keys: bool,      // >= 2.55.0  (friendly-name == event name is fatal)
    }
    ```
- **Fallback for `--deduplicate`.** `ls-files` (cached mode) prints entries sorted by path and then stage, and `--deduplicate` skips an entry whose name equals the previously printed one (`builtin/ls-files.c:448-449,483-491@v2.56.0`). So `ls-files -z` + "drop a record equal to the previous record" gives byte-identical results on any git. Use it only when `!ls_files_dedup`. Keep the real flag otherwise, for argv parity in error texts.
- **Two builders, never mixed:**
  - `GitCmd::user_repo(args)`: cwd = the root that G01 found. Full inherited env. Exact pre-commit argv, plus only the §G.7 additions (rows 1–3). Per-call `env_remove("GIT_DIFF_OPTS")` for G09.
  - `GitCmd::owned_repo(dir, args)`: env = `no_git_env(environ)` + the §G.7 row 4 injection.

  Build `GIT_CONFIG_COUNT` injection like this:
  - Parse the existing `GIT_CONFIG_COUNT` as a `u32`.
  - If it is absent, start at 0.
  - If it is present but bogus, **do not inject**. Leave the env as it is, so git still reports `bogus count in GIT_CONFIG_COUNT`.
  - Set `GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>` for the new entries and raise the count. Later entries win, so the injected values beat the user's `GIT_CONFIG_COUNT` entries. `no_git_env` already removes `GIT_CONFIG_PARAMETERS`, which would otherwise win.
- **Never chdir** away from the root between git calls. A relative `GIT_INDEX_FILE`/`GIT_DIR` (git exports `.git/index`) and `diff.relative` would then resolve differently from pre-commit.
- **Keep git's output order** in every list (`diff.orderFile` parity). Store paths as raw bytes, `-z`-split, with no Unicode normalisation (macOS NFC comes from git, so pass it through).
- **Exit-code driven logic only.** 129 = unknown option or usage (version backstop), 128 = fatal, 1 = differences. Do not match on stderr text (it is localised). Copy stderr verbatim into error texts and the crash log.
- **G23–G25 error texts** come from Python's stdlib exception, not pre-commit's: `Command '<repr of the argv tuple>' returned non-zero exit status <n>.`, or `Command '<…>' died with <Signals.SIGKILL: 9>.` for a negative return code. The tuple starts with the bare `'git'` (no `normalize_cmd`). stderr is not captured, so git's own message has already gone to the terminal.
- **Pre-push parser:** implement `py_splitlines(&str) -> impl Iterator<Item=&str>` and `py_rsplit_ws(&str, 3) -> Vec<&str>` with `fn py_isspace(c: char) -> bool { c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c) }`. Unit-test them against CPython vectors: the boundary set in §G.9 item 2, `'  x  y\tz w  '`, the `(delete)` line, the `HEAD@{1 second ago}` line, CRLF input, and an empty line.
- **Version check:** as in §G.4. Compare `GitVersion` with derived `Ord` (field order major, minor, patch). Unit-test the parser on the five forms in §G.4.
- **Config-hook writer** (`snp install --config-hooks`):
  - sh-quote with single quotes, writing `'` as `'\''`. On Windows, convert the snp path to forward slashes before quoting.
  - Never write a comment.
  - Always end with ` --`.
  - Never write `parallel`.
  - Before writing, refuse any friendly name equal to a known hook event, using git's `hook-list.txt` set.
- **Optional (safety):** take a per-worktree advisory lock `<git-dir>/snp-stash.lock` (`fs4`/`rustix::fs::flock`; `LockFileEx` on Windows) around stash → restore.
  - Check `_PRE_COMMIT_SKIP_POST_CHECKOUT` **before** taking the lock, so the nested post-checkout run from G10 cannot deadlock.
  - Use try-lock with a timeout and a clear message. Nested `snp run` from inside a hook must fail fast instead of hanging.
  - This guards against concurrent snp runs and git 2.55+ parallel hooks. pre-commit has no such lock.
- **Tests to add** (differential where pre-commit is the oracle):
  - each hazard (a)–(d) as a regression: snp must succeed with an unchanged worktree;
  - config hooks on 2.54 and 2.56 for every hook type pre-commit supports, with and without the trailing `--`;
  - traditional + config double run;
  - the `--all-files` warning band on 2.30;
  - refusal on 2.18;
  - `diff.orderFile` ordering parity;
  - `color.ui=always` / `diff.noprefix` / `diff.context=0` invariance of modification detection;
  - `GIT_CONFIG_PARAMETERS` propagation (`git -c diff.renames=false commit`).

### G.11 Open questions

- **G-1. Channel for snp-only diagnostics.** The version warning goes to stderr here, to keep stdout byte-identical to pre-commit. pre-commit itself logs warnings to stdout. Is stderr the project-wide rule for snp-only messages?
- **G-2. Leniency in `hook-impl` `rest`.** Drop `rest[0]` only when it equals `--`, and keep it otherwise? That would make config hooks without the trailing `--` work, where pre-commit errors. It changes only hand-written invocations (`hook-impl --hook-type pre-commit x`, where pre-commit silently drops `x`). The default here is parity.
- **G-3. Config-hook command program.** Absolute snp path (robust against GUI PATHs, breaks when snp moves) or bare `snp`/`pre-commit` (pre-commit's docs)? Sharing the friendly name `pre-commit.<type>` with pre-commit means the last installer wins. Is that desired (it prevents double runs) or surprising?
- **G-4. Pre-push misparse.** pre-commit misparses ref names that contain Unicode whitespace or Unicode line separators. Keep the bug (strict parity) or split only on the ASCII SP that git emits?
- **G-5. Masked git errors.** Should snp ever print git's stderr after `git failed. Is it installed…` (e.g. `safe.directory`)? The default here is crash-log only.
- **G-6. Asymmetric `no_git_env` allowlist.** It keeps `GIT_CONFIG_COUNT/KEY/VALUE` but drops `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, `GIT_CONFIG_NOSYSTEM` and `GIT_LFS_SKIP_SMUDGE`. Should snp honour those for store clones (CI auth via `GIT_CONFIG_GLOBAL`), at the cost of parity on which config applies? This affects 03 §7.6 too.
- **G-7. Stash lock.** Adopt the per-worktree stash lock (§G.10) by default?
- **G-8. G30 argv.** Is `--object-format=` acceptable in G30's argv, or must snp keep that argv exact and instead read `init.defaultObjectFormat`/`GIT_DEFAULT_HASH` (the latter is already stripped) to decide whether to re-init?
- **G-9. Partial-clone user repos.** Unverified: whether `git cat-file -e <unknown sha>^{commit}` (G22) triggers a lazy fetch from the promisor remote. If it does, pre-push would hit the network once per push. Test on a `--filter=blob:none` and a `--filter=tree:0` clone.
- **G-10. Git 3.0.** When it ships, re-run §G.3 (SHA-256 and reftable defaults, `safe.bareRepository`), and check that `MERGE_HEAD`/`MERGE_MSG` remain plain files under reftable. Since 2.45 they are "special refs" stored as files. pre-commit's merge detection (`git.py:96-101`) depends on that.
