# snp spec 03: hook installation, the store/cache, and the remaining commands

Scope: `install`, `uninstall`, `install-hooks`, `init-templatedir`, `hook-impl` (the git-hook entry point), the
store/cache (`store.py`, `file_lock.py`), hook instantiation and environment state (`repository.py`), `gc`, `clean`,
`autoupdate` (and the `migrate-config` pass it runs), `try-repo`, `validate-config`, `validate-manifest`,
`sample-config`, `hazmat`. `run` internals, languages, and config schema are covered in other specs. They are only
referenced here.

---

## 0. Conventions and provenance

- **REF** means pre-commit **4.6.2** source in `refs/pre-commit` (commit `c429238`). Citations look like
  `pre_commit/x.py:LINE` and are relative to `refs/pre-commit/`. Other sources are cited as `lefthook/...` and
  `prek/...`, relative to `refs/`.
- **OBS** means behavior observed by running the installed **pre-commit 4.1.0** (git 2.43.0, CPython 3.12, Linux) in
  throwaway repos with an isolated `HOME`, `PRE_COMMIT_HOME` and `GIT_CONFIG_GLOBAL`. When OBS and REF differ, REF is
  authoritative. The relevant differences are listed in §0.1.
- "stdout" is literal. **Every pre-commit log line (`[INFO]`, `[WARNING]`, `[ERROR]`) and every
  `output.write_line` goes to stdout, not stderr** (`pre_commit/output.py:9-33`, `pre_commit/logging_handler.py:25-31`).
  Only `SystemExit("msg")` messages, argparse usage errors and the hook script's "not found" message go to stderr.
- A message written as `[WARNING] text` comes from the logger. The prefix is added once, to the first line only.
- Verbatim strings keep their exact spacing. pre-commit uses **two spaces after a period** in many messages.

### 0.1 Deltas between 4.1.0 (OBS) and 4.6.2 (REF) that affect this topic

| Change | Since | Where |
|---|---|---|
| `pre-commit hazmat {cd,ignore-exit-code,n1}` added | 4.5.0 | `pre_commit/commands/hazmat.py` |
| `hook-impl --hook-dir` may be omitted (git ≥ 2.54 config hooks); legacy hook is then not run | 4.6.0 | `pre_commit/commands/hook_impl.py:36-37` |
| `hook-impl --hook-type` became `required=True` (previously a missing value crashed with `TypeError`, exit 3) | 4.6.0 | `pre_commit/main.py:360` |
| `autoupdate --repo X` with an unknown X now prints an error and exits 1 (4.1.0 silently exits 0) | 4.6.1 | `pre_commit/commands/autoupdate.py:178-184` |
| pre-push `_rev_exists` uses `git cat-file -e <sha>^{commit}` (4.1.0: `git rev-list --quiet <sha>`) | 4.6.1 | `pre_commit/commands/hook_impl.py:116-117` |
| `local` repo template no longer writes `package.json` and `.npmignore` (`LOCAL_REPO_VERSION` is still `'1'`) | 4.6.1 | `pre_commit/store.py:39-45` |
| `gc` runs as one DB transaction under the store lock | ≥4.2 | `pre_commit/commands/gc.py:61-93` |
| A manifest whose top level is a mapping gets a forward-compat error ("pre-commit version 5 is required…") | 4.x | `pre_commit/clientlib.py:273-279` |
| `language: system`/`script` are aliases of `unsupported`/`unsupported_script`; meta hooks use `unsupported` | 4.4.0 | `pre_commit/clientlib.py:194-228, 417-420` |

---

## 1. Shared CLI frame (`pre_commit/main.py`)

### 1.1 Subcommands and options

| Command | No git needed (`COMMANDS_NO_GIT`, `main.py:44-47`) | `-c/--config` | Command-specific options |
|---|---|---|---|
| `autoupdate` | no | yes | `--bleeding-edge`, `--freeze`, `--repo REPO` (append, `dest=repos`, default `[]`), `-j/--jobs N` (int, default 1) |
| `clean` | **yes** | no | – |
| `gc` | **yes** | no | – |
| `hazmat` | **yes** | no | subcommand `cd SUBDIR CMD...`, `ignore-exit-code CMD...`, `n1 CMD...` |
| `init-templatedir` | **yes** | yes | positional `directory`; `--no-allow-missing-config` (`store_false`, `dest=allow_missing_config`, default True); `-t` |
| `install` | no | yes | `-f/--overwrite`, `--install-hooks`, `-t/--hook-type`, `--allow-missing-config` |
| `install-hooks` | no | yes | – |
| `migrate-config` | no | yes | – |
| `run` | no | yes | (run spec) |
| `sample-config` | **yes** | no | – |
| `try-repo` | no | yes | positional `repo`; `--ref/--rev`; all `run` options (`_add_run_options`) |
| `uninstall` | no | yes | `-t/--hook-type` |
| `validate-config` | **yes** | no | `filenames*` |
| `validate-manifest` | **yes** | no | `filenames*` |
| `help` | – | no | `help_cmd?`. No `--color`. |
| `hook-impl` (not intended for users) | no | yes | `--hook-type` (required, free string), `--hook-dir` (optional), `--skip-on-missing-config`, `rest` (`argparse.REMAINDER`) |

Every command except `help` has `--color {auto,always,never}` (`pre_commit/color.py:103-109`):
- The default is `os.environ['PRE_COMMIT_COLOR']`, or `auto` if unset. An invalid value (even from the env var) gives the
  argparse error `argument --color: invalid use_color value: 'x'` and exits 2.
- `always` is true. `auto` is true only when `sys.stderr.isatty()`, the terminal supports color (on Windows,
  `SetConsoleMode(ENABLE_VIRTUAL_TERMINAL_PROCESSING)` succeeded on stderr), and `TERM != 'dumb'` (`color.py:84-100`).

The `-c/--config` default is `.pre-commit-config.yaml` (`constants.py:5`). Help text: `Path to alternate config file`.

`-t/--hook-type` (`main.py:57-61`):
- It is `action='append'` with `dest=hook_types` and default `None`.
- `choices` is `HOOK_TYPES` (`clientlib.py:26-37`), in this order: `commit-msg, post-checkout, post-commit, post-merge,
  post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg`.
- `manual` is not accepted. Duplicates are kept and processed twice.

Argparse behaviors snp must accept, so existing scripts keep working:
- `--opt=value` and `--opt value`; `-tpre-push` and `-t pre-push`.
- **Unique-prefix abbreviation of long options**, for example `--over` for `--overwrite` (argparse `allow_abbrev=True`).
  In clap, use `infer_long_args(true)`.
- Options after positionals.
- No argv at all means `run` (`main.py:368-369`).
- `pre-commit help` prints the top-level help. `pre-commit help <cmd>` is the same as `<cmd> --help`. Both exit 0
  (`main.py:372-375`).
- `-V/--version` prints `pre-commit 4.6.2` and exits 0.
- Usage errors go to stderr with exit 2. Example (OBS):
  `pre-commit install: error: argument -t/--hook-type: invalid choice: 'bogus' (choose from 'commit-msg', 'post-checkout', 'post-commit', 'post-merge', 'post-rewrite', 'pre-commit', 'pre-merge-commit', 'pre-push', 'pre-rebase', 'prepare-commit-msg')`.

Help strings, for clap `about` parity (`main.py`):
- `install`: `-f, --overwrite` "Overwrite existing hooks / remove migration mode."; `--install-hooks` "Whether to install
  hook environments for all environments in the config file."; `--allow-missing-config` "Whether to allow a missing
  `pre-commit` configuration file or exit with a failure code."
- `init-templatedir`: `directory` "The directory in which to write the hook script."; `--no-allow-missing-config`
  "Assume cloned repos should have a `pre-commit` config."
- `autoupdate`: `--bleeding-edge` "Update to the bleeding edge of `HEAD` instead of the latest tagged version (the
  default behavior)."; `--freeze` "Store "frozen" hashes in `rev` instead of tag names"; `--repo` "Only update this
  repository -- may be specified multiple times."; `-j` "Number of threads to use.  (default %(default)s)."
- `try-repo`: `repo` "Repository to source hooks from."; `--ref/--rev` "Manually select a rev to run against,
  otherwise the `HEAD` revision will be used."
- Command help: autoupdate "Auto-update pre-commit config to the latest repos' versions."; clean "Clean out pre-commit
  files."; gc "Clean unused cached repos."; hazmat "Composable tools for rare use in hook `entry`.";
  init-templatedir "Install hook script in a directory intended for use with `git config init.templateDir`.";
  install "Install the pre-commit script."; install-hooks "Install hook environments for all environments in the
  config file.  You may find `pre-commit install --install-hooks` more useful."; migrate-config "Migrate list
  configuration to new map configuration."; sample-config "Produce a sample .pre-commit-config.yaml file"; try-repo
  "Try the hooks in a repository, useful for developing new hooks."; uninstall "Uninstall the pre-commit script.";
  validate-config "Validate .pre-commit-config.yaml files"; validate-manifest "Validate .pre-commit-hooks.yaml files";
  help "Show help for a specific command."; hazmat tools: cd "cd to a subdir and run the command", ignore-exit-code
  "run the command but ignore the exit code", n1 "run the command once per filename".

### 1.2 Startup sequence (`main.py:202-450`)

1. `os.environ` drops `__PYVENV_LAUNCHER__` and `PYTHONEXECUTABLE` at import time (`main.py:39-42`). This is
   Python-specific, but snp should also clear `PYTHONEXECUTABLE` for children (see the open questions).
2. Parse argv. `--version`, `--help` and usage errors exit here.
3. Enter `error_handler()` and `logging_handler(color)`.
4. `git.check_for_cygwin_mismatch()` runs on win32/cygwin only (`git.py:210-230`). It prints a multi-line warning that
   begins `pre-commit has detected a mix of cygwin python / git`.
5. **`store = Store()` runs for every command, before dispatch** (`main.py:380`). It creates the store dir, README and
   DB on first use, even for `sample-config` and `validate-config`.
6. If the command is not in `COMMANDS_NO_GIT`: run `_adjust_args_and_chdir(args)`, then
   `store.mark_config_used(args.config)` (`main.py:382-384`).
7. Dispatch. The return value is the exit code.

`_adjust_args_and_chdir` (`main.py:175-199`):
1. If `os.path.exists(args.config)`, make it absolute.
2. For `run`/`try-repo`: make `files` and `commit_msg_filename` absolute. For `try-repo`: make `repo` absolute if it
   exists on disk.
3. `os.chdir(git.get_root())`.
4. Convert `config` back to a path relative to the new cwd (the toplevel), with `os.path.relpath`. A config outside
   the repo becomes `../x.yaml`.
   **A non-existent config path is never made absolute, so it is reinterpreted relative to the toplevel**, not the
   invocation cwd. (OBS: `install -c "my cfg's.yaml"` from `sub/dir` embedded `--config=my cfg's.yaml`.)
5. Apply relpath to files, commit_msg_filename and the try-repo repo in the same way.

`git.get_root()` (`git.py:51-73`):
- It runs `git rev-parse --show-cdup`, then `abspath()` of the stripped output, then `git rev-parse --is-inside-git-dir`.
- It does not use `--show-toplevel`, because git ≥ 2.25 exposes the underlying volume for drives mapped with Windows
  `SUBST`.
- A git failure raises FatalError
  `git failed. Is it installed, and are you in a Git repository directory?`.
- If `--is-inside-git-dir` is not `false` (inside `.git`, or a bare repo), it raises FatalError
  ``git toplevel unexpectedly empty! make sure you are not inside the `.git` directory of your repository.``
  (OBS for both `cd .git` and a bare repo; exit 1.)

### 1.3 Output and colors

| Level | Prefix (color off) | Prefix bytes (color on) |
|---|---|---|
| INFO | `[INFO]` | `[INFO]\033[m` (empty color string, but the reset is still appended; OBS) |
| WARNING | `[WARNING]` | `\033[43;30m[WARNING]\033[m` |
| ERROR | `[ERROR]` | `\033[41m[ERROR]\033[m` |

The line format is `{prefix} {message}` (`logging_handler.py:25-31`). Each write is flushed.

Python `print()` (used by `hook_impl._validate_config`, `validate_*`, `sample_config` and `migrate_config`) is
block-buffered when stdout is not a TTY. OBS: `Configuration has been migrated.` appeared *after* the autoupdate
lines when piped. snp should emit in program order.

### 1.4 Error handler and crash log (`pre_commit/error_handler.py:19-81`)

| Exception | Message prefix | Exit |
|---|---|---|
| `FatalError` and subclasses (`InvalidConfigError`, `InvalidManifestError`) | `An error has occurred` | 1 |
| `KeyboardInterrupt` | `Interrupted (^C)` | 130 |
| any other `Exception` | `An unexpected error has occurred` | 3 |

Output (stdout):
1. `{prefix}: {ExceptionClassName}: {str(exc)}`.
   For cfgv errors `str(exc)` begins with `\n`, so the first line ends in `InvalidConfigError: ` with a trailing space
   (OBS).
2. `Check the log at {store_dir}/pre-commit.log` when the store dir is writable. Otherwise
   `Failed to write to log at {log_path}`, and the log body goes to stdout.

The log file is opened with `'wb'` and overwritten each time. Its format (OBS):

~~~
### version information

```
pre-commit version: 4.1.0
git --version: git version 2.43.0
sys.version:
    3.12.3 (main, Aug 31 2026, 10:18:26) [GCC 13.3.0]
sys.executable: /home/u/.local/share/pipx/venvs/pre-commit/bin/python
os.name: posix
sys.platform: linux
```

### error information

```
An error has occurred: InvalidConfigError: 
==> File .pre-commit-config.yaml
=====> mapping values are not allowed in this context
  in "<unicode string>", line 6, column 66
```

```
Traceback (most recent call last):
  ...
```
~~~

`CalledProcessError` text (`pre_commit/util.py:49-78`). The command tuple is the Python repr. argv[0] is the
**resolved absolute executable path**, because `normalize_cmd` resolves it. Example (OBS):

```
An unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', 'checkout', 'v9.9.9')
return code: 1
stdout: (none)
stderr:
    error: pathspec 'v9.9.9' did not match any file(s) known to git
```

Rules: each non-empty part goes on a new line indented 4 spaces with trailing whitespace stripped. An empty part
prints ` (none)`.

OBS pitfall: a `BrokenPipeError` (for example `pre-commit init-templatedir X | head -3`) crashes with two Python
tracebacks. snp must treat EPIPE on stdout as a quiet exit.

---

## 2. `install`

### 2.1 Dispatch

`install(config, store, hook_types=args.hook_types, overwrite=args.overwrite, hooks=args.install_hooks,
skip_on_missing_config=args.allow_missing_config)` with `git_dir=None` (`main.py:410-417`). Because `install` is not in
`COMMANDS_NO_GIT`, it chdirs to the toplevel and marks the config as used first.

### 2.2 Algorithm (`pre_commit/commands/install_uninstall.py:114-141`)

1. **`core.hooksPath` refusal.** This applies only when `git_dir is None`, so `init-templatedir` skips it.
   - Run `git config core.hooksPath` with `check=False`. If the stripped stdout is non-empty, treat it as set
     (`git.py:180-182`).
   - The effective value from any scope counts: system, global, local, worktree, `include`, `includeIf`, and
     `GIT_CONFIG_COUNT/KEY/VALUE` env.
   - For a multi-valued key, git prints the last value and exits 0. That counts as set (OBS).
   - An empty value (`core.hooksPath=""`) counts as unset (OBS).
   - When set, log at ERROR and **return 1**. Nothing is written. Exact output (OBS):
     ```
     [ERROR] Cowardly refusing to install hooks with `core.hooksPath` set.
     hint: `git config --unset-all core.hooksPath`
     ```
2. For each `hook_type` in `_hook_types(config, hook_types)`, call `_install_hook_script(...)` in order.
3. If `--install-hooks`, call `install_hooks(config, store)` (§4). With a missing config this raises
   InvalidConfigError (exit 1) *after* the scripts have been written (OBS).
4. Return 0.

`_hook_types` (`install_uninstall.py:35-44`):
- An explicit `-t` list is returned as is: order kept, duplicates kept.
- Otherwise it calls `load_config(config)` and returns `default_install_hook_types`. The schema default is
  `['pre-commit']` (`clientlib.py:507-511`).
- **Any `InvalidConfigError` (missing file, bad YAML, schema error) silently falls back to `['pre-commit']`.**
  Warnings emitted during the load, such as unexpected keys or a mutable rev, are still printed.

### 2.3 Hook location (`install_uninstall.py:47-53`, `git.py:86-93`)

- `git_dir` is `git rev-parse --git-common-dir`, then `os.path.normpath(os.path.join('.', out))`. cwd is the toplevel
  after the chdir.
- `hook_path = {git_dir}/hooks/{hook_type}` and `legacy_path = {hook_path}.legacy`.
- Results (OBS). The printed path is relative in the main worktree and absolute in the other layouts:

| Layout | `--git-common-dir` | Printed hook path |
|---|---|---|
| normal repo, toplevel or subdir (after chdir) | `.git` | `.git/hooks/pre-commit` |
| linked worktree (`git worktree add`) | `/abs/main/.git` | `/abs/main/.git/hooks/pre-commit` (**shared by all worktrees**) |
| submodule | `/abs/super/.git/modules/sm` | `/abs/super/.git/modules/sm/hooks/pre-commit` |
| bare repo / inside `.git` | – | never reached: `get_root` FatalError, exit 1 |

- pre-commit **ignores `core.hooksPath` entirely** for path resolution. It refuses instead (§2.2).
  `git rev-parse --git-path hooks` would honor it.
- `.git/hooks` is created with `os.makedirs(exist_ok=True)` if missing, for example after `git init --template=`.

### 2.4 Writing the script (`install_uninstall.py:64-111`)

1. `os.makedirs(dirname(hook_path), exist_ok=True)`.
2. If `os.path.lexists(hook_path) and not is_our_script(hook_path)`, run `shutil.move(hook_path, legacy_path)`.
   - **This silently overwrites an existing `.legacy`** (POSIX rename; on Windows `shutil.move` falls back to
     copy+unlink).
   - A dangling symlink is not "ours", because `exists()` is False, so it is moved to `.legacy` (OBS). It is never run
     later, because `os.access(X_OK)` on a dangling link is False.
3. If `overwrite and exists(legacy_path)`, run `os.remove(legacy_path)`.
   Otherwise, if `exists(legacy_path)`, print exactly (OBS):
   ```
   Running in migration mode with existing hooks at {legacy_path}
   Use -f to use only pre-commit.
   ```
   This is printed on every `install` while the `.legacy` file exists, including re-installs.
4. `args = ['hook-impl', f'--config={config_file}', f'--hook-type={hook_type}']`, plus `'--skip-on-missing-config'` if
   `--allow-missing-config` was given.
5. Open `hook_path` with `'w'`. This truncates in place, is **not atomic**, follows symlinks, and uses text mode, so
   newlines are CRLF on Windows (derived from Python semantics, not verified on Windows).
   - On `win32`, write `#!/bin/sh\n` first, so the file then has two shebang-like lines.
   - Write the template text before `# start templated\n`, then that marker line.
   - Write `INSTALL_PYTHON={shlex.quote(sys.executable)}\n`.
   - Write `ARGS=({shlex.join(args)})\n`.
   - Write `# end templated\n` and the rest of the template.
6. `make_executable` sets `mode |= 0o111` (u+x, g+x, o+x on top of the umask-derived mode, `util.py:43-46`). With
   umask 002 the result is `rwxrwxr-x` (OBS).
7. Print `pre-commit installed at {hook_path}`.

Template resource `pre_commit/resources/hook-tmpl`, verbatim (20 lines, trailing newline):

```bash
#!/usr/bin/env bash
# File generated by pre-commit: https://pre-commit.com
# ID: 138fd403232d2ddd5efb44317e38bf03

# start templated
INSTALL_PYTHON=''
ARGS=(hook-impl)
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

Installed result (OBS, Linux, default options):

```bash
# start templated
INSTALL_PYTHON=/home/u/.local/share/pipx/venvs/pre-commit/bin/python
ARGS=(hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit)
# end templated
```

Quoting examples (OBS, `shlex.join`/`shlex.quote`):
- `--config=my cfg's.yaml` becomes `'--config=my cfg'"'"'s.yaml'`.
- `--allow-missing-config` adds `--skip-on-missing-config`.

`shlex.quote` rule: the empty string becomes `''`. A string that contains only `[A-Za-z0-9_@%+=:,./-]` is left as is.
Anything else becomes `'` + `s.replace("'", "'\"'\"'")` + `'`.

Runtime semantics of the script:
- `HERE` is the logical absolute path of the hooks dir (`cd` + `pwd`; symlinks not resolved).
- argv becomes `hook-impl --config=… --hook-type=… [--skip-on-missing-config] --hook-dir "$HERE" -- <git hook args…>`.
- The interpreter is the one that ran `install`, if it is still executable. Otherwise `pre-commit` from PATH.
  Otherwise the message above goes to **stderr** and the script exits 1.
- The script does not read stdin. `exec` passes stdin through, which is how pre-push gets its ref lines.

### 2.5 Recognizing "our" scripts (`install_uninstall.py:22-61`)

`is_our_script(path)`:
1. If `os.path.exists(path)` is false, return False. Symlinks are followed.
2. Read the **whole file as bytes**.
3. Return True if any of these 32-character ASCII markers appears anywhere in it:

| Role | Hash |
|---|---|
| `CURRENT_HASH` | `138fd403232d2ddd5efb44317e38bf03` |
| `PRIOR_HASHES` | `4d9958c90bc262f47553e2c073f14cfe`, `d8ee923c46731b42cd95cc869add4062`, `49fd668cb42069aa1b6048464be5d395`, `79f09a650522a87b0da915d0d983b2de`, `e358c9dae00eac5d06b38dfdb1e33a8c` |

A directory at `hook_path` makes `open()` raise `IsADirectoryError`, which is an unexpected error (exit 3).

### 2.6 Idempotency and observed sequences (OBS)

| Situation | Output | Files afterwards |
|---|---|---|
| fresh repo, `install` | `pre-commit installed at .git/hooks/pre-commit` | `pre-commit` (0775) |
| re-run `install` | same line again | rewritten (no `.legacy`) |
| foreign `pre-push` exists, `install -t pre-push` | migration lines, then `pre-commit installed at .git/hooks/pre-push` | `pre-push`, `pre-push.legacy` |
| re-run the same | migration lines again, then installed | unchanged pair |
| `install -f -t pre-push` | only the installed line | `.legacy` deleted |
| `install -t pre-commit -t commit-msg -t post-checkout --allow-missing-config` | 3 installed lines, in order | each `ARGS=(… --skip-on-missing-config)` |
| config has `default_install_hook_types: [10 types]`, `install` | 10 installed lines, in config order | 10 scripts |
| `core.hooksPath=foo` | `[ERROR] Cowardly refusing…` | exit 1, nothing written |
| from a subdirectory with `-c ../../.pre-commit-config.yaml` | `--config=.pre-commit-config.yaml` (normalized) | – |
| `-c /abs/outside.yaml` | `--config=../outside.yaml` | – |
| not a git repo | `An error has occurred: FatalError: git failed. Is it installed, and are you in a Git repository directory?` | exit 1 |

---

## 3. `uninstall` (`install_uninstall.py:149-167`, `main.py:334-338, 434-438`)

- Options: `-c`, `-t`, `--color`. **No `core.hooksPath` check** and no `--git-dir`.
- Hook types come from `_hook_types(config, args.hook_types)`, the same logic as install. Without `-t`, it removes the
  config's `default_install_hook_types` (fallback `['pre-commit']`).
- For each type:
  1. Compute `hook_path`/`legacy_path` from `--git-common-dir`.
  2. If `not os.path.exists(hook_path) or not is_our_script(hook_path)`, skip it **silently**.
  3. `os.remove(hook_path)`, then print `{hook_type} uninstalled`.
  4. If `os.path.exists(legacy_path)`, run `os.replace(legacy_path, hook_path)` and print
     `Restored previous hooks to {hook_path}`.
- Always returns 0.
- Quirk: if the hook script is missing but `.legacy` exists, the legacy file is **not** restored.
- OBS: `pre-commit uninstall` printed `pre-commit uninstalled`. With a legacy file present it also printed
  `Restored previous hooks to .git/hooks/pre-commit`, and the original content was restored.

---

## 4. `install-hooks` (`install_uninstall.py:144-146`)

- `install_hook_envs(all_hooks(load_config(config), store), store)`, then return 0.
- It installs environments for **all** hooks in the config, whatever their stage or `SKIP`.
- It prints nothing when everything is already installed.
- Missing config (OBS):
  ```
  An error has occurred: InvalidConfigError: 
  =====> .pre-commit-config.yaml is not a file
  Check the log at …/pre-commit.log
  ```
  Exit 1. With `repos: []` it prints nothing and exits 0.

---

## 5. `init-templatedir` (`pre_commit/commands/init_templatedir.py:14-39`, `main.py:254-271, 418-423`)

1. It is in `COMMANDS_NO_GIT`: **no chdir to the toplevel and no `mark_config_used`**. `-c` (default
   `.pre-commit-config.yaml`) is read relative to the **invocation cwd**, only to resolve `default_install_hook_types`.
   OBS: run inside a repo whose config lists 10 types, it installed all 10 into the template dir.
2. It calls `install(config, store, hook_types, overwrite=True, skip_on_missing_config=allow_missing_config,
   git_dir=directory)`.
   - Hooks go to `{directory}/hooks/{type}`. `directory` is used as typed: a relative input gives a relative printed
     path.
   - `overwrite=True` means any `.legacy` is deleted and no migration message is shown.
   - `skip_on_missing_config` **defaults to True**. Passing `--no-allow-missing-config` turns it off.
   - **The `core.hooksPath` check is skipped** (`git_dir` is not None). OBS: a global hooksPath was ignored.
   - The return value of `install` is ignored.
   - The embedded `--config=` value is the raw `-c` string, which at hook time is resolved relative to the cloned
     repo's toplevel.
3. Run `git config init.templateDir` (effective value, all scopes, cwd = invocation cwd).
   - Nonzero exit (unset) means `configured_path = None`.
   - Otherwise `configured_path = realpath(expanduser(stdout.strip()))`. Only `~` is expanded. git's `%(prefix)/`
     syntax is not.
4. `dest = realpath(directory)`. If `configured_path != dest`, emit two warnings (OBS):
   ```
   [WARNING] `init.templateDir` not set to the target directory
   [WARNING] maybe `git config --global init.templateDir {dest}`?
   ```
   `dest` is absolute and realpath'd.
5. **Always exits 0.** A tilde form such as `~/../tmpl` that resolves to the directory produced no warning (OBS).

---

## 6. `hook-impl`: the git hook entry point

### 6.1 Invocation

- From the installed script:
  `hook-impl --config=<cfg> --hook-type=<type> [--skip-on-missing-config] --hook-dir <abs hooks dir> -- <git args…>`.
- From git ≥ 2.54 config-based hooks (pre-commit 4.6 docs, pre-commit.com "Usage with git 2.54+ hook configuration"):
  ```
  git config set hook.pre-commit.pre-commit.event pre-commit
  git config set hook.pre-commit.pre-commit.command 'pre-commit hook-impl --hook-type pre-commit --'
  git config set hook.pre-commit.pre-push.event pre-push
  git config set hook.pre-commit.pre-push.command 'pre-commit hook-impl --hook-type pre-push --'
  # global variant (discouraged; must use --skip-on-missing-config):
  git config set --global hook.pre-commit.pre-commit.command 'pre-commit hook-impl --hook-type pre-commit --skip-on-missing-config --'
  ```
  The docs ask for the friendly name `pre-commit.<hook-type>` and say "a later version will change `pre-commit install`
  to use this approach". There is no `--hook-dir`, so legacy hooks are not run.
- Git config hook facts (git-scm `git-hook` docs, 2.54):
  - `hook.<friendly-name>.command` is a path or a shell one-liner.
  - `hook.<friendly-name>.event` is multi-valued. An empty value resets it.
  - `hook.<friendly-name>.enabled` defaults to true.
  - Config hooks run in config-parse order. The traditional hookdir script runs **last**. If both a `.git/hooks`
    script and a config hook call pre-commit, the hooks run **twice**.
  - The friendly name must not be a known event name.
- `rest` is `argparse.REMAINDER`, and **the first element is always dropped** (`args=args.rest[1:]`, `main.py:408`).
  It is meant to be `--`.
  OBS: `hook-impl --hook-type=post-commit --hook-dir d x` drops `x` and runs with 0 args. With `-- a b`, `args` is
  `['a','b']`.
- `--hook-type` is not validated by argparse choices. An unknown value fails in `_check_args_length` with
  `AssertionError: unexpected hook type: <t>` (unexpected error, exit 3, OBS).
- `--color` is omitted by the script, so `PRE_COMMIT_COLOR`/auto applies. During `git commit`, stderr is usually the
  terminal, so color is on.
- Like other git commands, it chdirs to the toplevel (git already runs hooks from the worktree root) and runs
  `mark_config_used(config)`, a **sqlite write on every hook invocation**. `GIT_*` env vars set by git (for example
  `GIT_INDEX_FILE` during `git commit -a`/partial commits, or `GIT_DIR`) are **inherited** by repo-introspection
  commands. That is required for correct behavior.

### 6.2 Flow (`pre_commit/commands/hook_impl.py:259-275`)

```
retv, stdin = _run_legacy(hook_type, hook_dir, args)      # 1. legacy first (even before arg/config checks)
_validate_config(retv, config, skip_on_missing_config)    # 2. may exit
ns = _run_ns(hook_type, color, args, stdin)                # 3. arg-count check (may exit), build run namespace
return retv if ns is None else (retv | run(config, store, ns))   # 4. bitwise OR of exit codes
```

The final code is `legacy_rc | run_rc`. OBS: a legacy pre-push hook that exited 3 made the push fail with 3 even
though the pre-commit hooks passed.

### 6.3 Legacy hook (`hook_impl.py:17-46`)

1. If env `PRE_COMMIT_RUNNING_LEGACY` is non-empty, raise `SystemExit` (stderr, exit 1) with this exact text:
   ```
   bug: pre-commit's script is installed in migration mode
   run `pre-commit install -f --hook-type {hook_type}` to fix this

   Please report this bug at https://github.com/pre-commit/pre-commit/issues
   ```
2. If `hook_type == 'pre-push'`, `stdin = sys.stdin.buffer.read()` (all of it, as bytes). Otherwise `b''`. **Stdin is
   never read for other hook types, including `post-rewrite`, which git also feeds on stdin.**
3. If `hook_dir is None` (git 2.54 config hooks), return `(0, stdin)`.
4. `legacy = {hook_dir}/{hook_type}.legacy`. If `not os.access(legacy, os.X_OK)`, return `(0, stdin)`. On Windows any
   existing file passes `X_OK`.
5. Run `subprocess.run(normalize_cmd((legacy, *args)), input=stdin)` with env `PRE_COMMIT_RUNNING_LEGACY=1` added.
   - stdout/stderr are inherited.
   - The legacy hook gets the same positional args.
   - Its stdin is the captured bytes for pre-push and **empty** for everything else.
   - `normalize_cmd` parses the shebang, so it works on Windows (`pre_commit/parse_shebang.py:65-82`).
   - Return `(returncode, stdin)`.

### 6.4 Missing config (`hook_impl.py:49-67`)

The check is `os.path.isfile(config)`, relative to the toplevel. If the config is missing:

- If `--skip-on-missing-config` was given, or **env `PRE_COMMIT_ALLOW_NO_CONFIG` is any non-empty string, including
  `"0"`**: print `` `{config}` config file not found. Skipping `pre-commit`. `` (stdout) and exit with **`retv`**, the
  legacy hook's code.
- Otherwise print (stdout) and exit **1**, ignoring the legacy code:
  ```
  No {config} file was found
  - To temporarily silence this, run `PRE_COMMIT_ALLOW_NO_CONFIG=1 git ...`
  - To permanently silence this, install pre-commit with the --allow-missing-config option
  - To uninstall pre-commit run `pre-commit uninstall`
  ```

OBS:
- `No .pre-commit-config.yaml file was found` followed by the three lines; the commit was aborted with 1.
- With `PRE_COMMIT_ALLOW_NO_CONFIG=1`: `` `.pre-commit-config.yaml` config file not found. Skipping `pre-commit`. ``
  and exit 0.

### 6.5 Argument-count validation (`hook_impl.py:177-211`)

This runs **after** the legacy hook and the config check. Errors use `SystemExit` (stderr, exit 1). `{args}` is the
Python list repr, for example `['a', 'b']`.

| Hook type | Allowed argc | Error text |
|---|---|---|
| `prepare-commit-msg` | 1..3 | `hook-impl for prepare-commit-msg expected 1, 2, or 3 arguments but got {n}: {args}` |
| `pre-rebase` | 1..2 | `hook-impl for pre-rebase expected 1 or 2 arguments but got {n}: {args}` |
| `commit-msg`, `post-merge`, `post-rewrite` | 1 | `hook-impl for {t} expected 1 argument but got {n}: {args}` |
| `post-checkout` | 3 | `hook-impl for {t} expected 3 arguments but got {n}: {args}` |
| `pre-push` | 2 | `hook-impl for {t} expected 2 arguments but got {n}: {args}` |
| `pre-commit`, `pre-merge-commit`, `post-commit` | 0 | `hook-impl for {t} expected 0 arguments but got {n}: {args}` |
| anything else | – | `AssertionError('unexpected hook type: {t}')`, exit 3 |

OBS: `hook-impl for commit-msg expected 1 argument but got 2: ['a', 'b']`.

### 6.6 Per-hook-type mapping to the `run` namespace (`hook_impl.py:70-113, 214-256`) and exported env (`pre_commit/commands/run.py:376-417`)

The namespace always has `hook_stage=<hook_type>`, `color`, `files=()`, `hook=None`, `verbose=False`,
`show_diff_on_failure=False`, `fail_fast=False` and `all_files=False` unless set. `run` always exports `PRE_COMMIT=1`.

| Hook type | Positional args → fields | Extra env vars exported by `run` (only when the condition holds) |
|---|---|---|
| `pre-commit`, `pre-merge-commit`, `post-commit` | – | – |
| `commit-msg` | `$1` → `commit_msg_filename` | – |
| `prepare-commit-msg` | `$1` → `commit_msg_filename`; `$2` → `prepare_commit_message_source`; `$3` → `commit_object_name` | `PRE_COMMIT_COMMIT_MSG_SOURCE=$2`, `PRE_COMMIT_COMMIT_OBJECT_NAME=$3` (each if non-empty) |
| `post-checkout` | `$1` → `from_ref`; `$2` → `to_ref`; `$3` → `checkout_type` | `PRE_COMMIT_FROM_REF`, `PRE_COMMIT_TO_REF`, legacy `PRE_COMMIT_ORIGIN`(=from), `PRE_COMMIT_SOURCE`(=to); `PRE_COMMIT_CHECKOUT_TYPE` |
| `post-merge` | `$1` → `is_squash_merge` | `PRE_COMMIT_IS_SQUASH_MERGE` (any non-empty value, `"0"` included) |
| `post-rewrite` | `$1` → `rewrite_command` | `PRE_COMMIT_REWRITE_COMMAND` |
| `pre-rebase` | `$1` → `pre_rebase_upstream`; `$2` → `pre_rebase_branch` | `PRE_COMMIT_PRE_REBASE_UPSTREAM` and `PRE_COMMIT_PRE_REBASE_BRANCH`, **only when both are present**; with one arg, nothing is exported |
| `pre-push` | `$1` → `remote_name`; `$2` → `remote_url`; stdin → see §6.7 | `PRE_COMMIT_LOCAL_BRANCH`, `PRE_COMMIT_REMOTE_BRANCH`, `PRE_COMMIT_REMOTE_NAME`, `PRE_COMMIT_REMOTE_URL` (when all four are set); from/to vars as for post-checkout when from/to are set |

Related: `run` returns 0 immediately for `post-checkout` when env `_PRE_COMMIT_SKIP_POST_CHECKOUT` is set. The stash
logic sets it while running `git checkout` (`pre_commit/staged_files_only.py:78`, `run.py:369-372`). snp must set and
honor this exact name.

Quirk: in REF, `_run_ns` uses `elif hook_type in 'commit-msg'` (substring test). It is harmless because types are
validated first.

### 6.7 pre-push stdin algorithm (`hook_impl.py:116-174`)

Stdin lines have the form `<local ref> SP <local sha> SP <remote ref> SP <remote sha> LF`. Z40 is `'0' * 40`.

```
for line in stdin.decode('utf-8').splitlines():          # strict UTF-8; failure -> exit 3
    local_branch, local_sha, remote_branch, remote_sha = line.rsplit(maxsplit=3)   # <4 fields -> ValueError -> exit 3
    if local_sha == Z40: continue                          # branch deletion
    if remote_sha != Z40 and git cat-file -e <remote_sha>^{commit} succeeds:     # (4.1.0: git rev-list --quiet <sha>)
        return ns(from_ref=remote_sha, to_ref=local_sha, remote_branch, local_branch, remote_name, remote_url)
    # new branch, or remote tip unknown locally:
    ancestors = git rev-list <local_sha> --topo-order --reverse --not --remotes=<remote_name>   (stdout stripped)
    if ancestors == '': continue                           # everything already on the remote
    first = ancestors.splitlines()[0]
    roots = set(git rev-list --max-parents=0 <local_sha>)
    if first in roots:
        return ns(all_files=True, remote_name, remote_url, remote_branch, local_branch)   # no from/to refs
    source = git rev-parse <first>^
    return ns(from_ref=source, to_ref=local_sha, remote_name, remote_url, remote_branch, local_branch)
return None   # nothing to push -> hook_impl returns the legacy rc without running hooks
```

Semantics and edge cases:
- **Only the first line that yields a namespace is used.** With multiple refs, as in `git push --all`, later refs are
  never checked.
- `remote_name` is `$1`. When pushing to a URL rather than a named remote, `$1` is the URL, `--remotes=<url>` matches
  nothing, and every ancestor counts. That usually ends in the root-commit case, which is `all_files=True`.
- Force-push after a rebase: if the old remote tip still exists locally, `from=remote_sha` is used and the diff is
  `from...to`, which run computes from the merge base.
- **SHA-256 repositories:** null object ids are 64 zeros, so `== Z40` is false. A deletion line would then reach
  `cat-file`/`rev-list` with a null id. snp must treat "all zeros, any length" as null (prek does this).
- Lines with spaces in the local ref still parse, because the split is from the right.
- OBS:
  - First push of a branch that contains the root commit: no from/to refs (all files), and `PRE_COMMIT_LOCAL_BRANCH`,
    `PRE_COMMIT_REMOTE_BRANCH`, `PRE_COMMIT_REMOTE_NAME` and `PRE_COMMIT_REMOTE_URL` were exported.
  - Second push: from = old remote sha, to = new local sha.
  - New branch `feat` off pushed `main`: `from = parent of the first unpushed commit = main tip`, `to = feat tip`.
  - Deleting a remote branch: hooks did not run, exit 0.
  - "Everything up-to-date": git does not invoke the hook.

---

## 7. The store (`pre_commit/store.py`, `pre_commit/file_lock.py`)

### 7.1 Directory resolution (`store.py:25-36`)

```
dir = $PRE_COMMIT_HOME                 if non-empty
   or $XDG_CACHE_HOME/pre-commit       if XDG_CACHE_HOME non-empty
   or expanduser('~/.cache')/pre-commit
dir = os.path.realpath(dir)            # symlinks resolved; a relative value resolves against the invocation cwd
```

- `expanduser` uses `$HOME` on POSIX (falling back to the passwd entry) and `%USERPROFILE%` on Windows. So the default
  is `%USERPROFILE%\.cache\pre-commit` on Windows and `~/.cache/pre-commit` on macOS (**not** `~/Library/Caches`).
- `Store()` is constructed before the chdir, which is why a relative `PRE_COMMIT_HOME` resolves against the
  invocation cwd.
- `try-repo` uses `Store(tempdir)`, which is neither realpath'd nor given a README.

### 7.2 Layout (OBS + REF)

```
<store>/                                  (realpath'd)
├── README                                written only when pre-commit itself created <store>
├── .lock                                 empty file used for flock/msvcrt locking; never deleted
├── db.db                                 sqlite3, created 0600 (mkstemp) and atomically renamed
├── pre-commit.log                        last crash log (overwritten)
├── patch<unix-epoch>-<pid>               unstaged-changes patches from `run`'s stash (staged_files_only.py:68);
│                                         NEVER cleaned up by pre-commit (gc ignores them)
├── tmpXXXXXXXX                           possible leftover from an interrupted db creation
├── repo<8 random [a-z0-9_]>/             mode 0700 (tempfile.mkdtemp(prefix='repo')); one per (db key, ref)
│   ├── .git/  + working tree @ rev       cloned hook repo (shallow when possible)
│   └── <ENV_DIR>-<language_version>/     hook environments live INSIDE the repo dir (§8.4)
│       ├── .install_state_v1             JSON {"additional_dependencies": [...]}
│       └── .install_state_v2             empty marker
└── repo<…>/                              a `local` pseudo-repo: template files (§7.6) + env dirs
```

README content (`store.py:72-76`), exactly 2 lines:

```
This directory is maintained by the pre-commit project.
Learn more: https://github.com/pre-commit/pre-commit
```

### 7.3 Initialization (`store.py:62-101`)

1. `readonly = exists(dir) and not os.access(dir, W_OK)`. This is computed before creating anything.
2. If `dir` does not exist: `makedirs(dir, exist_ok=True)` and write the README.
3. If `db.db` exists, return. **This is the fast path. There is no lock or sqlite open here.**
4. Otherwise, under the exclusive lock: re-check `db.db`. Then `mkstemp(dir=dir)` creates `tmpXXXX` (0600), the
   schema is created in that temp file, and `os.replace(tmp, db.db)` makes it atomic. This protects against a ^C
   between creating the file and the `CREATE TABLE`.

### 7.4 SQLite schema and queries (exact SQL)

Creation (`store.py:90-98`, `store.py:217-223`). Note the exact whitespace in the source strings:

```sql
CREATE TABLE repos (    repo TEXT NOT NULL,    ref TEXT NOT NULL,    path TEXT NOT NULL,    PRIMARY KEY (repo, ref));
CREATE TABLE IF NOT EXISTS configs (   path TEXT NOT NULL,   PRIMARY KEY (path));
```

| Operation | SQL | Where |
|---|---|---|
| lookup clone | `SELECT path FROM repos WHERE repo = ? AND ref = ?` | `store.py:145-148` |
| register clone | `INSERT INTO repos (repo, ref, path) VALUES (?, ?, ?)` | `store.py:168-171` |
| track config | `CREATE TABLE IF NOT EXISTS configs (...)`, then `INSERT OR IGNORE INTO configs VALUES (?)` | `store.py:225-235` |
| gc: list | `SELECT repo, ref, path FROM repos`; `SELECT path FROM configs` | `gc.py:65-70` |
| gc: delete | `DELETE FROM configs WHERE path = ?` (executemany); `DELETE FROM repos WHERE repo = ? and ref = ?` (executemany, sorted keys) | `gc.py:82-88` |

- Values:
  - `repo` is the db key (§7.7): the config `repo:` string **verbatim** (no URL normalization, and a relative local
    path stays relative), or `local`, or `<repo>:<dep1>,<dep2>`.
  - `ref` is the config `rev:` string verbatim (tag, branch or sha; never resolved), or `'1'` for local.
  - `path` is the absolute store subdirectory.
- Connections: `contextlib.closing(sqlite3.connect(path))` plus `with db:`, which gives one transaction per `connect()`
  block. The Python default busy timeout is 5 s. Concurrent writers beyond that fail with `database is locked`
  (exit 3).
- **There is no schema version table.** The only "migration" is the lazy `CREATE TABLE IF NOT EXISTS configs` (stores
  created before config tracking existed). It runs in `mark_config_used` and `gc`. Other versioning is implicit:
  `LOCAL_REPO_VERSION='1'` in the `ref` column for local repos ("Bump when modifying `empty_template`",
  `constants.py:8-9`), and the `.install_state_v1`/`v2` file names.

### 7.5 Locking (`pre_commit/file_lock.py`, `store.py:103-109`)

- The lock file is `<store>/.lock`, opened with `'a+'`, which creates it if missing.
- POSIX: `flock(fd, LOCK_EX|LOCK_NB)`. On failure, call `blocked_cb()`, which logs `[INFO] Locking pre-commit directory`
  (`store.py:105-106`), and then block in `flock(fd, LOCK_EX)`. Release with `LOCK_UN`.
- Windows: `msvcrt.locking(fd, LK_NBLCK, 0xffff)`. On failure, call `blocked_cb()`, then loop on `LK_LOCK` (which
  internally retries 10 times at 1 s), retrying on `EDEADLOCK`. Unlock with `LK_UNLCK`. This is byte-range locking of
  65535 bytes at offset 0.
- The lock is held:
  - in `Store.__init__` while creating the DB;
  - in `_new_repo` around clone plus DB insert (double-checked: query, lock, query again);
  - in `install_hook_envs` around **all** environment installs of a run (double-checked);
  - in `gc` for the whole operation.
  It is **not** held by `clean` or `mark_config_used`.
- The lock is advisory, process-wide and not reentrant. Nested acquisition from the same process would deadlock, and
  pre-commit avoids it. The `clone()` calls inside `install_hook_envs` happen before the lock.

### 7.6 Cloning (`store.py:132-210`, `git.py:16, 27-48, 185-193`)

`Store.clone(repo, ref, deps=())` delegates to `_new_repo(repo, ref, deps, clone_strategy)`:

1. `key = db_repo_name(repo, deps)` (§7.7). Look up `(key, ref)`. If a row exists, **return the stored path without
   checking that it still exists**. OBS: after manually deleting a cached repo dir, `run` failed with
   `InvalidManifestError: =====> …/repoXXXX/.pre-commit-hooks.yaml is not a file`.
2. Take the lock and look up again.
3. Log `[INFO] Initializing environment for {key}.`. `key` includes `:deps`, for example
   `Initializing environment for https://github.com/pre-commit/mirrors-mypy:types-requests.`
4. `directory = mkdtemp(prefix='repo', dir=store)` gives mode 0700.
5. Under `clean_path_on_failure(directory)` (`util.py:27-35`), run the strategy. Any `BaseException`, including ^C,
   removes `directory` and re-raises. A hard kill (SIGKILL) leaves an orphan `repoXXXX` that is in no DB row and that
   gc never removes.
6. `INSERT` the row.
7. `warn_for_stages_on_repo_init(original_repo, directory)` (`clientlib.py:84-109`):
   - Load the manifest with a minimal schema. A failure here is silently ignored.
   - If any hook's `stages` uses `commit`, `push` or `merge-commit`, log (deduplicated, in first-seen order):
     ``[WARNING] repo `{repo}` uses deprecated stage names ({a, b}) which will be removed in a future version.  Hint: often `pre-commit autoupdate --repo {shlex.quote(repo)}` will fix this.  if it does not -- consider reporting an issue to that repo.``
     It is printed once per new clone (OBS).
8. Return `directory`.

`clone_strategy(directory)`, with the exact commands. "git" is the bare `git` resolved via PATH; every command
captures stdout/stderr with stdin=PIPE.

| # | Command | cwd | env |
|---|---|---|---|
| 1 | `git -c core.useBuiltinFSMonitor=false init --template= <directory>` | process cwd | `no_git_env()` |
| 2 | `git -c core.useBuiltinFSMonitor=false remote add origin <repo>`. `<repo>` is made `abspath` first **if `os.path.isdir(repo)`**. | directory | `no_git_env()` |
| 3 | `git -c protocol.version=2 fetch origin <ref> --depth=1` | directory | `no_git_env()` |
| 4 | `git checkout FETCH_HEAD` | directory | `no_git_env()` |
| 5 | `git -c protocol.version=2 submodule update --init --recursive --depth=1` | directory | `no_git_env()` |
| | If **any** of 3–5 raises `CalledProcessError`, fall back in the **same** directory to: | | |
| 6 | `git fetch origin --tags` | directory | `no_git_env()` |
| 7 | `git checkout <ref>` | directory | `no_git_env()` |
| 8 | `git submodule update --init --recursive` | directory | `no_git_env()` |

- `--template=` avoids the user's `init.templateDir`, so pre-commit's own hooks are not installed into the clone.
- The result is a detached HEAD at the rev. Fetching an arbitrary sha shallowly needs server support
  (`uploadpack.allowReachableSHA1InWant`/`allowAnySHA1InWant`, or protocol v2 on GitHub). The fallback covers other
  servers. `git fetch --tags` also fetches the configured branch refspec (git ≥ 1.9), and `git checkout <branch>`
  DWIMs from `origin/<branch>`.
- A failure of both strategies is an **unexpected error (exit 3)**, with the `CalledProcessError` of the *fallback*
  command (OBS for `rev: v9.9.9`: `command: ('/usr/bin/git', 'checkout', 'v9.9.9')`), and the directory is removed.
- **Credential prompts are not disabled.** git may prompt on `/dev/tty`.

`no_git_env()` (`git.py:27-48`) removes every env var that starts with `GIT_` **except**:
- names starting with `GIT_CONFIG_KEY_` or `GIT_CONFIG_VALUE_`;
- `GIT_EXEC_PATH`, `GIT_SSH`, `GIT_SSH_COMMAND`, `GIT_SSL_CAINFO`, `GIT_SSL_NO_VERIFY`, `GIT_CONFIG_COUNT`,
  `GIT_HTTP_PROXY_AUTHMETHOD`, `GIT_ALLOW_PROTOCOL`, `GIT_ASKPASS`.

Rationale: git exports `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE` while running hooks, which breaks clones.

### 7.7 Repo keys, `local` repos and additional_dependencies

- `db_repo_name(repo, deps)` returns `f'{repo}:{",".join(deps)}'` if deps is non-empty, otherwise `repo`
  (`store.py:125-130`). It is **order-sensitive**: `[a,b]` and `[b,a]` are different keys. It is also ambiguous in
  theory with commas inside deps, such as `pkg[x,y]` extras.
- **Every distinct `additional_dependencies` list of a cloned repo gets its own full clone directory.** In
  `_cloned_repository_hooks`, `store.clone(repo, rev, hook['additional_dependencies'])` is called per hook
  (`pre_commit/repository.py:193-200`). The manifest itself is read from the no-deps clone `store.clone(repo, rev)`
  (`repository.py:177`).
- `make_local(deps)` is `_new_repo('local', '1', deps, _make_local_repo)` (`store.py:212-215`). It is used only for
  `repo: local` hooks whose language has an environment (`repository.py:152-159`). Hooks whose language has no
  environment, and all `meta` hooks, use `Prefix(os.getcwd())`, the repo toplevel.
- `_make_local_repo(dir)` (`store.py:48-56`) writes these files from `pre_commit/resources/empty_template_<basename>`,
  in this order:

| Target file | Content (exact) |
|---|---|
| `Cargo.toml` | `[package]\nname = "__fake_crate"\nversion = "0.0.0"\n\n[[bin]]\nname = "__fake_cmd"\npath = "main.rs"\n` |
| `main.go` | `package main\n\nfunc main() {}\n` |
| `go.mod` | `module pre-commit-placeholder-empty-module\n` |
| `main.rs` | `fn main() {}\n` |
| `pre-commit-package-dev-1.rockspec` | lua rockspec: `package = "pre-commit-package"`, `version = "dev-1"`, `source = { url = "git+ssh://git@github.com/pre-commit/pre-commit.git" }`, empty description/dependencies, `build = { type = "builtin", modules = {}, }` (212 bytes) |
| `pre_commit_placeholder_package.gemspec` | `Gem::Specification.new do \|s\|` with name `pre_commit_placeholder_package`, version `0.0.0`, summary `placeholder gem for pre-commit hooks`, authors `['Anthony Sottile']` |
| `setup.py` | `from setuptools import setup\n\n\nsetup(name='pre-commit-placeholder-package', version='0.0.0', py_modules=[])\n` |
| `environment.yml` | conda: channels `conda-forge`, `defaults`; dependencies `- openssl` (with a comment explaining it cannot be empty) |
| `Makefile.PL` | `use ExtUtils::MakeMaker;` / `WriteMakefile(NAME => "PreCommitPlaceholder", VERSION => "0.0.1",);` |
| `pubspec.yaml` | `name: pre_commit_empty_pubspec` / `environment:` / `  sdk: '>=2.12.0'` / `executables: {}` |
| `renv.lock` | R 4.0.3, CRAN `https://cran.rstudio.com`, package renv 0.12.5 (hash `5c0cdb37f063c58cdab3c7e9fbb8bd2c`) |
| `renv/activate.R` | renv 0.12.5 bootstrap script (12037 bytes) |
| `renv/LICENSE.renv` | RStudio MIT license text (1052 bytes) |

snp should embed these byte-for-byte from `refs/pre-commit/pre_commit/resources/` (via `include_bytes!`), because the
language installers depend on them. OBS 4.1.0 also wrote `package.json` and `.npmignore`; REF 4.6.x does not.

### 7.8 Config tracking (`store.py:225-235`)

`mark_config_used(path)`:
- No-op if `store.readonly`.
- `path = realpath(path)`. Return if the path does not exist.
- Otherwise `CREATE TABLE IF NOT EXISTS configs …` and `INSERT OR IGNORE INTO configs VALUES (?)`.

It is called for every command not in `COMMANDS_NO_GIT` (`run`, `hook-impl`, `install`, `uninstall`,
`install-hooks`, `autoupdate`, `migrate-config`, `try-repo`), using the relpath'd `args.config`. OBS: worktree
configs appear as separate rows.

### 7.9 Read-only stores

`readonly` only disables config tracking. Anything that needs a clone or install fails (mkdtemp `PermissionError`,
exit 3). Cached runs succeed (changelog: "Allow pre-commit to succeed on a readonly store"). The crash log falls back
to stdout (`Failed to write to log at …`).

### 7.10 Observed robustness gaps (snp should not replicate)

- Stale DB rows whose directory was deleted: `run` fails as shown in §7.6, and **`gc` crashes** with
  `An unexpected error has occurred: FileNotFoundError: [Errno 2] No such file or directory: '…/repoXXXX'` (exit 3,
  OBS), because `rmtree` of a missing path raises.
- Relative local-path repos (`repo: ../hooks`) are keyed by the relative string. Two projects with the same relative
  string but different targets **share one cached clone**.
- Patch files accumulate forever. Orphan `repo*` dirs left by kills are never collected.

---

## 8. Hook instantiation and environments (`pre_commit/repository.py`, `pre_commit/hook.py`)

### 8.1 `all_hooks(root_config, store)` (`repository.py:232-237`)

This walks `config['repos']` in order and, within each repo, the hooks in order. It returns a flat tuple of `Hook`.
It runs **for all hooks** regardless of stage, so every remote repo gets cloned, even by `run` for a single stage.

- `repo: local` or `repo: meta` → `_non_cloned_repository_hooks` (`repository.py:147-168`):
  - `Hook.create(src=repo, prefix, _hook(hook, root_config))`.
  - The prefix is cwd for languages without an environment, otherwise `store.make_local(deps)`.
- Any other repo → `_cloned_repository_hooks` (`repository.py:171-200`):
  1. `manifest = load_manifest(store.clone(repo, rev)/.pre-commit-hooks.yaml)`. An `InvalidManifestError` is fatal
     (exit 1).
  2. Build `by_id` from the manifest. **Last wins for duplicate ids.**
  3. For each config hook whose id is not in the manifest, log ERROR and `exit(1)` (OBS):
     ``[ERROR] `{id}` is not present in repository {repo}.  Typo? Perhaps it is introduced in a newer version?  Often `pre-commit autoupdate` fixes this.``
  4. `dict = _hook(by_id[id], config_hook, root_config=root_config)`.
  5. `Hook.create(repo, Prefix(store.clone(repo, rev, dict['additional_dependencies'])), dict)`.

### 8.2 Merge and defaulting (`_hook`, `repository.py:109-144`)

1. `ret = dict(manifest_hook)`, then `ret.update(config_hook)`. The config keys win, as a shallow per-key override.
   Lists are **replaced**, not merged. Manifest defaults come from cfgv; config hooks have no defaults
   (`OptionalNoDefault`).
2. `language_version`:
   - if it is `'default'`, use `root_config['default_language_version'][language]`, whose schema default per language
     is `'default'`;
   - if it is still `'default'`, use `languages[lang].get_default_version()`.
3. `stages`: if empty or falsy, use `root_config['default_stages']`, whose schema default is all of `STAGES`
   (`clientlib.py:515`).
4. If the language has `ENVIRONMENT_DIR is None` (`docker_image`, `fail`, `pygrep`, `unsupported`,
   `unsupported_script`):
   - a `language_version` other than `'default'` logs ERROR and exits 1:
     ``The hook `{id}` specifies `language_version` but is using language `{lang}` which does not install an environment.  Perhaps you meant to use a specific language?``
   - a non-empty `additional_dependencies` logs ERROR and exits 1:
     ``The hook `{id}` specifies `additional_dependencies` but is using language `{lang}` which does not install an environment.  Perhaps you meant to use a specific language?``
5. `Hook.create` (`hook.py:49-57`): keys outside the 22 `Hook` fields log
   `[WARNING] Unexpected key(s) present on {src} => {id}: {sorted keys joined by ", "}`.
   This happens, for example, with a manifest that has newer keys.

`install_key = (prefix, language, language_version, tuple(additional_dependencies))` (`hook.py:39-46`). It
deduplicates environment installs.

### 8.3 Environment directory naming (`pre_commit/lang_base.py:90-91`)

`env_dir = os.path.normpath(os.path.join(prefix_dir, f'{ENVIRONMENT_DIR}-{language_version}'))`. Examples (OBS):
`py_env-python3.12`, `node_env-system`, `rustenv-default`.

`language_version` is used **raw**, so a path-like version such as `/usr/bin/python3.11` produces nested directories
(`py_env-/usr/bin/python3.11` → `…/py_env-/usr/bin/python3.11`). Because of `normpath`, `..` segments could even
escape the prefix. snp must sanitize this (see §20).

| language | ENVIRONMENT_DIR | `get_default_version()` (when both config levels say `default`) | health_check |
|---|---|---|---|
| conda | `conda` | `default` | none |
| coursier | `coursier` | `default` | none |
| dart | `dartenv` | `default` | none |
| docker | `docker` | `default` | none |
| docker_image | *None* | `default` | – |
| dotnet | `dotnetenv` | `default` | none |
| fail | *None* | `default` | – |
| golang | `golangenv` | `system` if `exe_exists('go')`, else `default` | none |
| haskell | `hs_env` | `default` | none |
| julia | `juliaenv` | `default` | none |
| lua | `lua_env` | `default` | none |
| node | `node_env` | win32: `default`; else `system` if `exe_exists('node')` and `exe_exists('npm')`; else `default` | `node --version` inside the env must return 0 |
| perl | `perl_env` | `default` | none |
| pygrep | *None* | `default` | – |
| python | `py_env` | the first of `python{X.Y}`, `python{X}` found on PATH (X.Y = pre-commit's own interpreter); else from `sys.executable`; else (win) py launcher; else `default` | `pyvenv.cfg` present with `version_info`, and the env's python version matching; `base-executable` checks |
| r | `renv` | `default` | the R version recorded at install must equal the current R |
| ruby | `rbenv` | `system` if `ruby` and `gem` are found via `exe_exists`, else `default` | none |
| rust | `rustenv` | `system` if `cargo --version` (cwd `/`) exits 0, else `default` | none |
| swift | `swift_env` | `default` | none |
| unsupported | *None* | `default` | – |
| unsupported_script | *None* | `default` | – |

`exe_exists(exe)` (`lang_base.py:63-83`) is true when the exe is found on PATH, its path does not match
`[/\\]shims[/\\]`, **and** either `$HOME` is `/` or the exe is not inside the home directory.

For snp the python default is not "pre-commit's interpreter". The language spec must define it, for example the
newest `python3.X` on PATH.

### 8.4 Install state and "installed" check (`repository.py:25-106`)

`_hook_installed(hook)`:
- Languages with no ENVIRONMENT_DIR always count as installed.
- Otherwise `venv = env_dir`, and the hook is installed iff
  `(exists(venv/.install_state_v2) or json.load(venv/.install_state_v1) == {'additional_dependencies': deps}) and
  health_check(prefix, version) is None`.
- A malformed v1 JSON raises an unexpected error (exit 3).

`_hook_install(hook)`:
1. Log three INFO lines (OBS, `{src}` is the config `repo` string, or `local`/`meta`):
   ```
   [INFO] Installing environment for {hook.src}.
   [INFO] Once installed this environment will be reused.
   [INFO] This may take a few minutes...
   ```
2. If `venv` exists (a partial previous install), `rmtree` it.
3. Under `clean_path_on_failure(venv)`, call `lang.install_environment(prefix, version, deps)` and then `health_check`.
   If the check returns an error, raise
   `AssertionError(f'BUG: expected environment for {language} to be healthy immediately after install, please open an issue describing your environment\n\nmore info:\n\n{health_error}')`
   (exit 3).
4. Write `.install_state_v1staging` with `json.dumps({'additional_dependencies': deps})`. The bytes are, for example,
   `{"additional_dependencies": ["six"]}` with the separators `", "` and `": "`. Then
   `os.replace(staging, .install_state_v1)`. The rename is the "installed" commit point.
5. Touch an empty `.install_state_v2`.

`install_hook_envs(hooks, store)` (`repository.py:214-229`):
1. `_need_installed()` walks the hooks in order and keeps the first hook per `install_key` that is not installed.
2. If none, return **without locking**.
3. Otherwise take `store.exclusive_lock()`, recompute, and install **sequentially** in config order.

---

## 9. `gc` (`pre_commit/commands/gc.py`)

In `COMMANDS_NO_GIT`: no chdir and no config marking. Algorithm (`gc.py:61-93`), all inside one
`exclusive_lock()` and one `connect()` transaction:

1. `CREATE TABLE IF NOT EXISTS configs`.
2. `all_repos = {(repo, ref): path}` from `repos`. `unused = set(all_repos)`.
3. For each tracked config path, try `load_config(path)`. Warnings are printed: mutable rev, unexpected keys,
   deprecated stages, and so on.
   - On `InvalidConfigError` (file deleted, invalid now, wrong version), append the path to `dead_configs`.
     **A config that is temporarily invalid is forgotten and its repos collected.**
   - Otherwise call `_mark_used_repos` for each repo in that config (`gc.py:18-58`):
     - `meta` is skipped.
     - `local`: for each hook, discard `(db_repo_name('local', hook.get('additional_dependencies')), '1')`. After
       schema defaults `deps` is `[]`, so the key is `local`.
     - Other repos:
       1. Look up `key = (repo, rev)`. If it is not cloned, return.
       2. Load the manifest from `path/.pre-commit-hooks.yaml`. On `InvalidManifestError`, return. **The repo stays
          unused and is deleted.**
       3. Otherwise discard `key` and, for each config hook whose id is in the manifest:
          `deps = hook.get('additional_dependencies', manifest_hook['additional_dependencies'])`, then discard
          `(db_repo_name(repo, deps), rev)`.
4. `DELETE FROM configs WHERE path = ?` for the dead configs.
5. `DELETE FROM repos WHERE repo = ? and ref = ?` for `sorted(unused)`.
6. `rmtree(path)` for each unused repo. Environments go with it, because they live inside.
7. Print `{n} repo(s) removed.` (always, including `0 repo(s) removed.`) and exit 0.

`rmtree` (`util.py:205-235`) retries on `EACCES`/`EPERM` by chmod-ing `+w` on the path and its parent. This is for
read-only git objects on Windows.

OBS:
- `gc` printed `0 repo(s) removed.`.
- After a rev bump with both revs cached, it printed `1 repo(s) removed.`.
- After deleting a worktree its config row was dropped.
- Configs not checked out (other branches) do not protect their repos.

---

## 10. `clean` (`pre_commit/commands/clean.py:10-16`)

- For `dir` in `(store.directory, expanduser('~/.pre-commit'))`, in that order: if it exists, `rmtree(dir)` and print
  `Cleaned {dir}.`. Exit 0.
- `~/.pre-commit` is the pre-1.0 legacy store.
- **No lock is taken.** It can race with running hooks.
- Because `Store()` re-creates the store before dispatch, `clean` **always** prints `Cleaned {store}.`, even right after
  a previous clean (OBS).

---

## 11. `autoupdate` (`pre_commit/commands/autoupdate.py`)

### 11.1 Flow (`autoupdate.py:162-222`)

1. `migrate_config(config_file, quiet=True)` (§16).
   - It may print `Configuration has been migrated.` and rewrite the file.
   - A missing config here is `FileNotFoundError`, an **unexpected error with exit 3** (OBS:
     `An unexpected error has occurred: FileNotFoundError: [Errno 2] No such file or directory: '.pre-commit-config.yaml'`).
2. `config_repos` are the repos of `load_config(config_file)` whose `repo` is not `local` or `meta`, in order.
3. REF ≥ 4.6.1: `missing = set(--repo values) - {r['repo']}`. If it is non-empty, print
   `repos ({", ".join(sorted(missing))}) were not found in {config_file}` and **return 1**. (4.1.0 silently returns 0;
   OBS.)
4. `rev_infos = [None] * len(config_repos)`.
5. `jobs`:
   - `jobs = jobs or cpu_count()` (0 means CPUs, from `sched_getaffinity`/`cpu_count`, `pre_commit/xargs.py:26-38`);
   - `jobs = min(jobs, len(repos) or len(config_repos))`;
   - `jobs = max(jobs, 1)`. A negative value ends up as 1.
6. Submit `_update_one(i, repo)` for each config repo with `not --repo or repo['repo'] in --repo` (exact string
   match) to a `ThreadPoolExecutor(jobs)`. Process results **in completion order** (`as_completed`). With `-j1` that is
   config order.
   - `RepositoryCannotBeUpdatedError`: print `str(e)` and set `retv = 1`.
   - Any other exception, for example `CalledProcessError` from `git fetch` on an unreachable or nonexistent repo,
     **aborts the whole command as an unexpected error (exit 3) and nothing is written**, even for repos already
     updated (OBS).
   - On success: if `new.rev != old.rev`, then `changed = True`, the message is `updating {old.rev} -> {new_s}`
     (`new_s = f'{new.frozen} (frozen)'` if frozen else `new.rev`), and `rev_infos[i] = new`. Otherwise the message
     is `already up to date!`.
   - Print `[{old.repo}] {msg}`.
7. If changed, `_write_new_config(config_file, rev_infos)`. Return `retv`.

### 11.2 `RevInfo.update(tags_only, freeze)` (`autoupdate.py:38-82`)

This works in a fresh `tempfile.TemporaryDirectory()` under the system temp dir; **the store is not used**.
`_git = ('git', '-c', 'core.useBuiltinFSMonitor=false', '-C', tmp)`.

| # | Command | Notes |
|---|---|---|
| 1 | `git -c core.useBuiltinFSMonitor=false init --template= <tmp>` and `git … remote add origin <repo>` | via `git.init_repo` (abspath if a local dir; `no_git_env`) |
| 2 | `git … -C tmp config extensions.partialClone true` | inherits the full env (not sanitized) |
| 3 | `git … -C tmp fetch origin HEAD --quiet --filter=blob:none --tags` | blobless fetch of the remote HEAD (default branch) plus all tags |
| 4a | tags_only (default): `git … describe FETCH_HEAD --tags --abbrev=0` | nearest tag reachable from HEAD, lightweight included |
| 4b | `--bleeding-edge`: `git … describe FETCH_HEAD --tags --exact` | tag name only if HEAD is exactly tagged |
| 5 | if 4 fails: `rev = git … rev-parse FETCH_HEAD` | full 40-hex sha (no tags / not exact) |
| 6 | if tags_only and 4a succeeded: `rev = get_best_candidate_tag(rev, tmp)` | runs `git … tag --points-at <rev>` and returns the **first** listed tag (refname order) that contains `.`, else `rev` unchanged (`git.py:233-245`) |
| 7 | if freeze: `exact = git … rev-parse <rev>`; if `exact != rev` then `rev, frozen = exact, rev` | **for an annotated tag this is the tag OBJECT sha, not the commit** (OBS: `rev: e5943278…  # frozen: v2.0.0` where the commit was `f9559395…`) |
| 8 | `git … show <rev>:.pre-commit-hooks.yaml` then `git … checkout <rev> -- .pre-commit-hooks.yaml` | errors ignored ("workaround for windows #2865"); the blob is fetched lazily |
| 9 | `load_manifest(tmp/.pre-commit-hooks.yaml)` | `InvalidManifestError` becomes `RepositoryCannotBeUpdatedError(f'[{repo}] {e}')` |

The result is `RevInfo(repo, rev, frozen, hook_ids=frozenset(manifest ids))`.

`_check_hooks_still_exist_at_rev` (`autoupdate.py:89-100`): if config hook ids minus manifest ids is non-empty, the
error is `[{repo}] Cannot update because the update target is missing these hooks: {", ".join(sorted(missing))}`.

Observed outputs (OBS), one line each, prefixed `[<repo string as in config>]`:

```
[/path/hooks1] updating v1.0.0 -> v1.1.0
[/path/hooks1] already up to date!
[/path/hooks1] updating v1.1.0 -> v1.1.0 (frozen)
[/path/hooks1] updating d2b71d1a9423792ecbbe84e174b95bcc6ab72427 -> v1.1.0
[/path/hooks1] updating v1.1.0 -> aa658a0cb064bf927c7cbd4c3034e4d876de3c9c          (--bleeding-edge, HEAD untagged)
[/path/notags] updating abc -> 0f64dbc2f4dc0e22e6365e8d135664dca6b0c62a             (repo has no tags)
[/path/hooks2] Cannot update because the update target is missing these hooks: h2
[/path/badman] 
==> File /tmp/tmp4nkfzbwp/.pre-commit-hooks.yaml
==> At Hook(id=1)
==> At key: id
=====> Expected string got int
[/path/noman] 
=====> /tmp/tmph3vbog77/.pre-commit-hooks.yaml is not a file
```

The manifest errors show the random temp path and have a trailing space after `]`, because the cfgv string starts
with a newline.

Semantics worth preserving:
- The comparison is `new.rev != old.rev` on strings:
  - Freezing an already-current tag counts as a change (`v1.1.0 -> v1.1.0 (frozen)`).
  - Unfreezing (a sha with a `# frozen:` comment, run without `--freeze`) gives `updating <sha> -> v1.1.0`.
  - `--bleeding-edge --freeze` on an untagged HEAD sha that is already in the config gives `already up to date!`.
- When a repo is "already up to date", its line is untouched. Its frozen comment is not refreshed.
- The same repo URL listed twice is updated independently. Each prints its own line.
- A mutable `rev` (branch name) is updated like any other value. `load_config` prints the mutable-rev warning first
  (OBS).

### 11.3 Rewriting the YAML (`autoupdate.py:116-159`)

`REV_LINE_RE = re.compile(r'^(\s+)rev:(\s*)([\'"]?)([^\s#]+)(.*)(\r?\n)$')`. Groups: 1 indent (**at least one
whitespace character**), 2 space after the colon, 3 the opening quote if any, 4 the value **including any closing
quote**, 5 the rest of the line (spaces + comment), 6 the newline.

`_original_lines(path, rev_infos)`:
1. Read the file with `newline=''`, so CRLF is preserved. `lines = splitlines(keepends=True)`.
2. `idxs` are the indices of the lines that match.
3. If `len(idxs) == len(rev_infos)` (the number of non-local/meta repos), return.
4. Otherwise **reformat the whole file** as `yaml_dump(yaml_load(original))`, written in text mode, and retry once. If
   it still mismatches, `AssertionError('could not find rev lines')` (exit 3).
   - The reformat **drops every comment** and normalizes style (PyYAML, `indent=4`, block style, key order kept).
   - Triggers: flow-style configs, `rev:` at column 0, **a last line without a trailing newline** (OBS: the
     `# keep me` header and the `# c` comment were lost), and a `rev:` line count mismatch from other `rev:` keys.

Lines are matched to repos **positionally**: the k-th matching line belongs to the k-th non-local repo.

`_write_new_config`. For each `(idx, info)` with `info` not None:
1. `new_rev = yaml_dump({'rev': info.rev}, default_style=group3).split(':', 1)[1].strip()`.
   - A quoted value keeps its quote style.
   - An unquoted value uses PyYAML's plain/quoted choice. Values that would re-type, such as `1.0`, `yes` or `null`,
     get single quotes.
2. Comment:
   - if frozen: `'  # frozen: {tag}'` (two spaces). This **replaces** any existing comment, which is then lost
     forever (OBS: `rev: v1.1.0  # my comment` became `rev: <sha>  # frozen: v1.1.0`, then `rev: v1.1.0` after
     unfreezing);
   - else if `group5.strip().startswith('# frozen:')`: `''`, the comment is removed;
   - else: group5 unchanged (OBS: `rev: '0f64…'   # trailing` kept its 3 spaces).
3. The line becomes `{indent}rev:{group2}{new_rev}{comment}{newline}`.
4. Write with `newline=''`. CRLF is preserved (OBS). The write truncates in place and is **not atomic**.

PyYAML dump settings (`pre_commit/yaml.py:14-19`): `Dumper=CSafeDumper` (or `SafeDumper`), `default_flow_style=False`,
`indent=4`, `sort_keys=False`, default `width=80`. Long plain or single-quoted scalars are **folded at spaces past
column 80**. Example reformat output (OBS):

```yaml
repos:
-   repo: /path/hooks1
    rev: v1.1.0
    hooks:
    -   id: h1
-   repo: local
    hooks:
    -   id: x
        name: x
        entry: x
        language: system
```

Sequences inside mappings are not indented. Each item is `-` plus 3 spaces. An empty list is `[]` and an empty map
is `{}`.

---

## 12. `try-repo` (`pre_commit/commands/try_repo.py`)

`try-repo REPO [HOOK] [--ref/--rev REF] [run options]`. The `run` options are the same as `pre-commit run`, including
`-a`, `--files`, `--hook-stage`, `-v`, `--show-diff-on-failure` and `--fail-fast`. It is a git command: chdir, and the
user's config is marked used. `REPO` becomes a toplevel-relative path if it exists on disk.

1. Create `tempdir = TemporaryDirectory()` (system temp).
2. `_repo_ref(tempdir, repo, ref)` (`try_repo.py:21-48`):
   - If `--ref` was given, use `(repo, ref)` and **do not check for a dirty working tree**.
   - Otherwise `ref = git ls-remote --exit-code <repo> HEAD`, first whitespace token (`git.py:170-172`). A failure is
     exit 3.
   - If `os.path.exists(repo)` and `git diff --quiet --no-ext-diff HEAD` in `repo` returns 1:
     1. Log `[WARNING] Creating temporary repo with uncommitted changes...`.
     2. `git clone <repo> <tempdir>/shadow-repo`.
     3. `git checkout <ref> -b _pc_tmp` (cwd shadow).
     4. Build env `GIT_INDEX_FILE=<shadow git-path index>`, `GIT_OBJECT_DIRECTORY=<shadow git-path objects>`.
        (`git rev-parse --git-path X`, joined to shadow.)
     5. `staged = git diff --staged --name-only --no-ext-diff -z --diff-filter=ACMRTUXB` (cwd repo). If non-empty,
        run `git add -- <staged…>` (xargs-partitioned, cwd repo, the env above).
     6. `git add -u` (cwd repo, same env).
     7. `git.commit(shadow)` runs `git commit --no-edit --no-gpg-sign -n -minit` with `no_git_env()` plus
        `GIT_AUTHOR_NAME=GIT_COMMITTER_NAME=pre-commit` and
        `GIT_AUTHOR_EMAIL=GIT_COMMITTER_EMAIL=asottile+pre-commit@umich.edu` (`git.py:196-202`).
     8. Return `(shadow, ls-remote HEAD of shadow)`.
     Untracked files are **not** included.
   - Otherwise use `(repo, ref)`.
3. `store = Store(tempdir)`. This is a throwaway store, so every try-repo clones and installs from scratch.
4. Hooks:
   - With a positional `HOOK`: `[{'id': HOOK}]`.
   - Otherwise clone (prints `[INFO] Initializing environment for …`), load the manifest, **sort by id**, and use
     `[{'id': id} …]`. Duplicates are kept.
5. `config = {'repos': [{'repo': repo, 'rev': ref, 'hooks': hooks}]}`. Dump it with `yaml_dump` to
   `<tempdir>/.pre-commit-config.yaml`.
6. Print the following, then `return run(config_filename, store, args)`:
   ```
   ===============================================================================
   Using config:
   ===============================================================================
   repos:
   -   repo: ../hooks1
       rev: aa658a0cb064bf927c7cbd4c3034e4d876de3c9c
       hooks:
       -   id: h1
       -   id: h2
   ===============================================================================
   ```
   That is 79 `=` characters, `Using config:`, 79 `=`, the YAML exactly as dumped, and 79 `=`. Then `run` output
   follows. The clone or init messages come before this block when no HOOK is given and after it when HOOK is given
   (OBS).

---

## 13. `validate-config` and `validate-manifest` (`pre_commit/commands/validate_config.py:8-18`, `validate_manifest.py:8-18`)

- These are in `COMMANDS_NO_GIT` and run anywhere. They do **not** mark configs as used.
- For each filename in argv order: `load_config` or `load_manifest`. On the respective Invalid*Error, `print(e)` and
  set `ret = 1`.
- No filenames means exit 0 with no output.
- Warnings (unexpected keys, mutable rev, deprecated stages, regex hints) are logged as `[WARNING] …` during the load.
- Exit is 1 if any file failed, otherwise 0.
- `print(e)` puts a blank first line (the cfgv message starts with `\n`), then the trace, then a newline. OBS:

```
[WARNING] Unexpected key(s) present at root: foo

==> File bad1.yaml
==> At Config()
==> At key: repos
==> At Repository(repo='x')
=====> Missing required key: hooks

==> File bad2.yaml
=====> while parsing a flow sequence
  in "<unicode string>", line 1, column 6
did not find expected ',' or ']'
  in "<unicode string>", line 2, column 1

=====> missing.yaml is not a file
```

- A manifest that is a mapping:
  - 4.1.0 (OBS): `\n==> File warn.yaml\n=====> Expected array but got 'dict'`.
  - REF (`clientlib.py:273-279`): the load strategy calls `check_min_version('5')`. The resulting message is (derived,
    not run) `\n==> File X\n=====> \n=====> pre-commit version 5 is required but version 4.6.2 is installed.  Perhaps run \`pip install --upgrade pre-commit\`.`.
    The doubled marker appears because cfgv wraps `str(ValidationError)` into a new ValidationError.
- The cfgv message format is implemented by the user's crate `cfgv` 0.2.0 (`cfgv-rs/src/error.rs:62-71`:
  `\n`, then `==> {ctx}\n` repeated, then `=====> {leaf}`; `load.rs:32` gives `"{display} is not a file"`). YAML parse
  error texts are libyaml/PyYAML-specific and cannot be byte-identical in Rust. Golden tests should compare the
  structure only.

---

## 14. `sample-config` (`pre_commit/commands/sample_config.py`)

It prints exactly this (the `print(..., end='')` text has one trailing newline) and exits 0:

```yaml
# See https://pre-commit.com for more information
# See https://pre-commit.com/hooks.html for more hooks
repos:
-   repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v3.2.0
    hooks:
    -   id: trailing-whitespace
    -   id: end-of-file-fixer
    -   id: check-yaml
    -   id: check-added-large-files
```

---

## 15. `hazmat` (4.5.0+, `pre_commit/commands/hazmat.py`)

These are helpers for use inside a hook `entry`. They are in `COMMANDS_NO_GIT` (no chdir; cwd is whatever the hook
runner set, which is the toplevel). The official usage puts the hook `args` into `entry` and sets `args: []`, so that
`--` is the last token before the filenames:

```yaml
-   id: example
    alias: example-repo1
    files: ^repo1/
    entry: pre-commit hazmat cd repo1 example-bin --arg1 --
    args: []
```

`_cmd_filenames(cmd)` splits on the **last** `--` in `cmd`: `cmd[:i]` is the command and `cmd[i+1:]` are the
filenames. If there is no `--`, it raises `SystemExit` with ``hazmat entry must end with `--` ``, which goes to stderr
with exit 1.

| Tool | argv | Behavior | Exit |
|---|---|---|---|
| `cd SUBDIR CMD… -- FILES…` | `subdir`, `cmd` (REMAINDER) | Every filename must start with `f'{subdir}/'`. Otherwise `SystemExit(f'unexpected file without {prefix=}: {filename}')`, which renders as `unexpected file without prefix='repo1/': other/x.py` (Python repr quoting). The prefix is stripped, then `subprocess.call((*normalize_cmd(cmd), *stripped), cwd=subdir)`. | the child's code |
| `ignore-exit-code CMD…` | `cmd` | `subprocess.call(normalize_cmd(cmd))` with the **whole** cmd, including `--` and the filenames. **`--` is not stripped.** | always 0 |
| `n1 CMD… -- FILES…` | `cmd` | For each filename, sequentially: `subprocess.call((*normalize_cmd(cmd), filename))`. The result is the bitwise OR of the codes. | OR of codes |

- The children inherit stdio.
- `normalize_cmd` resolves the executable against the **current** cwd and PATH before `cwd=subdir` applies. A relative
  `./tool` is validated at the toplevel but executed relative to `subdir`.
- Docs: these do not work with the languages `docker`, `docker_image`, `fail`, `julia`, `pygrep`, `r` and
  `unsupported_script`, which rewrite `entry`. `ignore-exit-code` needs `verbose: true` to show output.
- `normalize_cmd` errors (`pre_commit/parse_shebang.py:46-62`): `Executable \`{orig}\` not found`,
  `… is a directory`, `… is not executable`. These are `ExecutableNotFoundError(OSError)`, which surface as exit 3.

---

## 16. `migrate-config`, as invoked by autoupdate (`pre_commit/commands/migrate_config.py:114-135`)

1. Read the file. A YAML that fails to parse becomes InvalidConfigError
   `\n==> File {config}\n=====> {yaml error}` (exit 1).
2. `_migrate_map`: if the top level is a list, insert `repos:` after the header lines (`#`, `---` or blank). If the
   result fails to parse, indent the rest by 4 spaces.
3. `_migrate_composed`: AST-position-based text replacement that preserves quote style:
   - `repos[*].sha` key → `rev`;
   - `repos[*].hooks[*].language: python_venv` → `python`;
   - `default_stages[*]` and `repos[*].hooks[*].stages[*]` values `commit|push|merge-commit` → `pre-<value>`.
4. If changed, write it and print `Configuration has been migrated.`. Otherwise print
   `Configuration is already migrated.` unless quiet (autoupdate passes `quiet=True`).
5. Exit 0.

OBS: a list-style config with `sha:` printed `Configuration has been migrated.`, and autoupdate then updated the
migrated file.

---

## 17. Environment variables (this topic)

| Variable | Read or set | Semantics |
|---|---|---|
| `PRE_COMMIT_HOME` | read | Store dir when non-empty (realpath'd) |
| `XDG_CACHE_HOME` | read | `$XDG_CACHE_HOME/pre-commit` when non-empty |
| `HOME` / `USERPROFILE` | read | `~` expansion (default store, `~/.pre-commit`, init.templateDir) |
| `PRE_COMMIT_ALLOW_NO_CONFIG` | read | Any non-empty value skips a missing config in hook-impl (exit = legacy rc) |
| `PRE_COMMIT_RUNNING_LEGACY` | read and set | Set to `1` for the legacy hook process. If already set when hook-impl starts, the recursion-bug exit described in §6.3 |
| `PRE_COMMIT_COLOR` | read | Default for `--color` |
| `TERM` | read | `dumb` disables auto color |
| `_PRE_COMMIT_SKIP_POST_CHECKOUT` | read and set | Suppresses post-checkout hooks during pre-commit's own checkouts |
| `PRE_COMMIT`, `PRE_COMMIT_FROM_REF`, `PRE_COMMIT_TO_REF`, `PRE_COMMIT_ORIGIN`, `PRE_COMMIT_SOURCE`, `PRE_COMMIT_REMOTE_NAME`, `PRE_COMMIT_REMOTE_URL`, `PRE_COMMIT_LOCAL_BRANCH`, `PRE_COMMIT_REMOTE_BRANCH`, `PRE_COMMIT_CHECKOUT_TYPE`, `PRE_COMMIT_IS_SQUASH_MERGE`, `PRE_COMMIT_REWRITE_COMMAND`, `PRE_COMMIT_COMMIT_MSG_SOURCE`, `PRE_COMMIT_COMMIT_OBJECT_NAME`, `PRE_COMMIT_PRE_REBASE_UPSTREAM`, `PRE_COMMIT_PRE_REBASE_BRANCH` | set (by run, for hooks) | See §6.6 |
| `GIT_*` | filtered | Removed for store clones and init (`no_git_env`, §7.6). **Not** filtered for repo-introspection or autoupdate's `-C tmp` commands |
| `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY` | set | try-repo shadow staging |
| `GIT_AUTHOR_NAME/EMAIL`, `GIT_COMMITTER_NAME/EMAIL` | set | try-repo shadow commit |
| `TMPDIR`/`TEMP`/`TMP` | read (by tempfile) | autoupdate and try-repo temp dirs |
| `PATH`, `PATHEXT` | read | Executable resolution (`find_executable`, PATHEXT on Windows) |
| `__PYVENV_LAUNCHER__`, `PYTHONEXECUTABLE` | removed | Popped from the process env at startup |

---

## 18. Platform differences

| Area | Linux | macOS | Windows |
|---|---|---|---|
| Hook script first line | `#!/usr/bin/env bash` | same | `#!/bin/sh` followed by the bash shebang line. Git for Windows runs hooks through its bundled `sh`/bash (POSIXLY_CORRECT bash still supports arrays) |
| Hook file newlines | LF | LF | Python text mode writes CRLF (derived) |
| exec bit | `mode \| 0o111` | same | no-op; git for Windows ignores it |
| `INSTALL_PYTHON` | shlex-quoted POSIX path | same | `'C:\…\python.exe'` single-quoted, backslashes literal |
| Legacy hook executable test | `os.access(X_OK)` | same | any existing file passes; shebang parsed by `normalize_cmd` |
| Store default | `~/.cache/pre-commit` | `~/.cache/pre-commit` (not `~/Library/Caches`) | `%USERPROFILE%\.cache\pre-commit` |
| Lock | `flock` exclusive | `flock` | `msvcrt.locking` 0xffff-byte region |
| `rmtree` | – | – | must clear the read-only attribute (git pack and objects) |
| realpath | – | `/tmp` → `/private/tmp`, `/var` → `/private/var`; affects `configs` rows and init-templatedir comparisons | drive letter case; `SUBST` drives (hence `--show-cdup`) |
| cygwin | – | – | a cygwin/windows python vs git mismatch prints a warning (`git.py:210-230`) |
| Color | auto | auto | only if enabling VT processing succeeds |

---

## 19. Ideas from lefthook and lessons from prek (for this topic)

| Idea | Source | Recommendation for snp |
|---|---|---|
| Resolve all paths with **one** `git rev-parse --path-format=absolute --show-toplevel --git-path hooks --git-path info --git-dir --git-common-dir` (git ≥ 2.31) | `lefthook/internal/git/wrapper/paths.go` | **Adopt** for hook-impl/install performance. Keep pre-commit's `--show-cdup` semantics for the root (SUBST). Note that in a bare repo `--show-cdup` prints **no line** (OBS), so do not rely on positional line parsing without ordering `--is-inside-work-tree`/`--is-bare-repository` first, or use NUL-free fixed ordering plus validation. |
| `core.hooksPath` handling: ignore when it equals `<repo>/.git/hooks`; `--force` installs into it; `--reset-hooks-path` unsets it; error shows local and global values | `lefthook/internal/command/install.go:515-590` | **Adapt as opt-in.** The default stays pre-commit's exact refusal. Add `snp install --force-hooks-path` to install into `git rev-parse --git-path hooks`, treat a hooksPath equal to `<common-dir>/hooks` as unset, and add `--reset-hooks-path` (local scope only). |
| Repo-scoped `core.hooksPath` (local/worktree/include) is safe to honor; global/system is not; `--force` overrides | `prek/crates/prek/src/cli/install.rs:40-90` | Consider as the snp opt-in policy (open question). |
| Respect `core.sharedRepository` when choosing hook file modes | `prek/…/install.rs:96` | Adopt: group-shared repos get g+w. |
| Refuse to overwrite an existing backup (`.old`) unless `--force` | `lefthook/internal/command/lefthook.go:122-157` | **Adopt** as a safety deviation: never silently clobber an existing `.legacy` (open question). |
| Config checksum file `.git/info/lefthook.checksum` (`<md5> <mtime> <hooks>`) plus a "ghost" `prepare-commit-msg` hook that auto-reinstalls hook scripts when the config changes | `lefthook/internal/command/install.go:401-485`, `run.go:77-110` | Optional opt-in: `snp` could re-sync hook scripts when `default_install_hook_types` changes (write `<git-dir>/info/snp.checksum`). It needs a config switch outside `.pre-commit-config.yaml`, because pre-commit warns on unknown root keys. |
| `check-install` command (exit 0/1) | `lefthook/internal/command/check_install.go` | Adopt as `snp check-install` (snp-only, harmless). |
| Hook script: env overrides `LEFTHOOK=0` (skip all) and `LEFTHOOK_BIN` (binary override); multiple fallback locations | `lefthook/internal/templates/hook.tmpl` | Adopt `SNP_BIN` (path override) in the script. Skipping everything is already `git … --no-verify`; an `SNP_SKIP_ALL`-like switch is optional. |
| uninstall scans the whole hooks dir and restores `.old` | `lefthook/internal/command/uninstall.go:50-84` | Optional `snp uninstall --all`. |
| uninstall removes a `.legacy` that is itself a manager script | `prek/…/install.rs:448-455` | Adopt: a `.legacy` recognized as "ours" (pre-commit or snp hash) is deleted instead of restored, because it would recurse. |
| Store keyed by a hash of (repo, rev) with a marker file written last (`.prek-repo.json`), and JSON config tracking (`config-tracking.json`) instead of sqlite | `prek/crates/prek/src/store.rs:60, 146, 299, 345` | Adopt the idea, not the code (see §20.5). |
| Parallel clones with terminal prompts disabled, then sequential retry with prompts for auth failures; no retry under CI | `prek/…/store.rs` (`clone_repos`) | Adopt. |
| pre-push: if the remote sha exists but is **not an ancestor** of the local sha (force push), compute the base like a new branch | `prek/…/hook_impl.rs:323` | Behavior change vs pre-commit. Keep pre-commit semantics by default (open question). |
| pre-push: treat all-zero ids of any length as null; skip malformed stdin lines instead of crashing | `prek/…/hook_impl.rs:301-320` | Adopt (no compatibility downside). |
| git ≥ 2.54 config hooks: hook-impl without `--hook-dir`; docs recommend a trailing `--` | prek cookbook; pre-commit 4.6 docs | Adopt: `snp install --config-hooks` writes `hook.pre-commit.<type>.{event,command}` (pre-commit's naming, so both tools agree), and detects the hooks-dir script to avoid double runs. |

---

## 20. Implementation notes for snp

### 20.1 Types (Rust 1.98, edition 2024)

- `#![forbid(unsafe_code)]` in every crate. Clippy `deny(unwrap_used, expect_used, panic, indexing_slicing,
  string_slice)` outside tests.
- Top level: `fn main() -> ExitCode` with a `catch_unwind` boundary that prints
  `An unexpected error has occurred: …` and exits 3, so a bug never shows a raw panic. Use `panic = "unwind"`.
- ```rust
  #[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
  pub enum HookType { CommitMsg, PostCheckout, PostCommit, PostMerge, PostRewrite,
                      PreCommit, PreMergeCommit, PrePush, PreRebase, PrepareCommitMsg }
  impl HookType {
      pub const ALL: [HookType; 10] = [/* alphabetical, as in HOOK_TYPES */];
      pub const fn as_str(self) -> &'static str;
      pub const fn arg_range(self) -> (usize, usize);   // (min, max) inclusive, table §6.5
      pub const fn reads_stdin(self) -> bool;           // PrePush (and PostRewrite for legacy forwarding, see open questions)
  }
  pub enum Stage { Hook(HookType), Manual }            // FromStr also accepts legacy commit/push/merge-commit
  ```
- `ExitCode` mapping: `Usage = 2` (clap), `Fatal = 1`, `Unexpected = 3`, `Interrupted = 130`. Hook-impl combines codes
  with bitwise OR (`legacy | run`) as `u8`, saturating any code above 255 the same way the shell would.
- Keep paths as `PathBuf`/`OsString` end to end. Git hook args, refs and the config path are bytes on Unix
  (`OsStr::as_encoded_bytes`). Never `to_string_lossy` before comparisons. Lossy conversion is only for display.

### 20.2 Git introspection

- `GitRepo::discover(cwd) -> Result<GitRepo, FatalError>`. Make one `git rev-parse` call that returns, for non-bare
  work trees, cdup, git-dir, common-dir and `--git-path hooks`. Detect `--is-inside-git-dir`/bare first; see §19 for
  the empty-line pitfall in bare repos.
- Map errors to pre-commit's two FatalError messages exactly (§1.2).
- Pass `-c core.useBuiltinFSMonitor=false` on store and autoupdate commands (Windows #2046).
- Run store/autoupdate/try-repo git commands with a sanitized env built by `no_git_env()` semantics (§7.6). Apply it
  **also** to autoupdate's `config`/`fetch`/`describe` (pre-commit does not, which is a bug when run from inside a
  hook).
- Spawn with `std::process::Command`. Resolve `git` once (PATH + PATHEXT on Windows) and cache it in a `OnceLock`.
  Avoid a shell.
- Treat object ids as `ObjectId(Box<[u8]>)` validated as hex of length 40 or 64. `is_null()` means all `b'0'`.

### 20.3 install, uninstall and init-templatedir

- Hook types: `-t` (ordered, duplicates kept) or `default_install_hook_types`. On **any** config load error, fall back
  to `[pre-commit]` but still print the load warnings, to match pre-commit.
- The `core.hooksPath` check uses `git config core.hooksPath` (effective value, any scope, `GIT_CONFIG_*` env). If the
  trimmed value is non-empty, refuse with the verbatim message and exit 1. Do not check in uninstall or
  init-templatedir.
- Hook dir is `--git-common-dir` + `hooks`. Print paths exactly as pre-commit does: relative `.git/hooks/<t>` in the
  main worktree, absolute in linked worktrees and submodules.
- **Atomic, symlink-safe writes.** Write to `<hooks>/.<type>.snp-XXXXXX` (via `tempfile::Builder::tempfile_in`), set
  the mode, then `persist()`, which is a rename onto `<type>`. This replaces a symlink rather than writing through it
  (deviation; document it). Mode: create, read back the umask-applied mode, then OR `0o111`. That is exactly Python's
  result without calling `umask()` (which would need unsafe libc). Optionally add g+w for `core.sharedRepository`.
- Legacy handling: same messages as pre-commit. **Safety deviation (proposed):** if `.legacy` exists and the
  `<type>` being moved is a different foreign script, do not clobber the existing `.legacy`. Error with
  `refusing to overwrite existing {legacy}; use -f to discard it` unless `-f`.
- `is_our_script`: read at most 64 KiB and search with `memchr::memmem` for the snp hash, `CURRENT_HASH`, or any of
  `PRIOR_HASHES` (§2.5). **Recognizing pre-commit's scripts as ours is essential**: otherwise `snp install` in a repo
  already managed by pre-commit would rename pre-commit's script to `.legacy` and every hook would run twice.
- Proposed snp script. It is POSIX `sh` with no bash arrays, the same on all platforms, and written with **LF**:

  ```sh
  #!/bin/sh
  # File generated by snp: https://github.com/grok-rs/snp
  # ID: <SNP_SCRIPT_HASH_V1>
  # pre-commit compat ID: 138fd403232d2ddd5efb44317e38bf03

  # start templated
  SNP_EXE='/abs/path/to/snp'
  set -- hook-impl '--config=.pre-commit-config.yaml' '--hook-type=pre-commit' --hook-dir "$(cd "$(dirname "$0")" && pwd)" -- "$@"
  # end templated

  if [ -n "$SNP_BIN" ] && [ -x "$SNP_BIN" ]; then
      exec "$SNP_BIN" "$@"
  elif [ -x "$SNP_EXE" ]; then
      exec "$SNP_EXE" "$@"
  elif command -v snp > /dev/null; then
      exec snp "$@"
  else
      echo '`snp` not found.  Did you forget to install it?' 1>&2
      exit 1
  fi
  ```
  - Including pre-commit's current hash lets `pre-commit install` and `pre-commit uninstall` treat snp's script as
    their own, which makes the swap reversible with no `.legacy` chains.
  - `SNP_EXE` comes from `std::env::current_exe()`, canonicalized. On Windows use forward slashes (`C:/…`) inside
    single quotes.
    - Pitfall: package managers expose versioned real paths, for example Homebrew's
      `/opt/homebrew/Cellar/snp/1.2.3/bin/snp`, which vanish on upgrade.
    - If the first `snp` on PATH resolves (via `same-file`) to the current exe, embed that PATH entry's non-canonical
      path instead.
    - The `command -v snp` fallback covers the remaining cases.
  - POSIX-quote every embedded argument with a local `sh_quote` that matches `shlex.quote` (§2.4). Property-test it
    against `sh -c 'printf %s "$1"'` round-trips.
  - Bump the snp hash when the template changes, and keep the old ones in `SNP_PRIOR_HASHES`.
- Messages: keep pre-commit's strings with `pre-commit` replaced by the product name only where the user would type
  that command. Examples: `snp installed at {path}`, `Use -f to use only snp.`. Exact wording is an open question.
- init-templatedir: no chdir; config relative to the invocation cwd; `overwrite = true`; skip-on-missing-config
  defaults to true; no hooksPath check; the `init.templateDir` comparison uses canonicalize of `expand_tilde`. Print
  the two warnings verbatim, with the product name in `git config --global init.templateDir {dest}`, and always exit 0.

### 20.4 hook-impl

- Parse argv strictly with clap: `--hook-type` must be a `HookType`, which gives a clap error (exit 2) instead of
  pre-commit's AssertionError (exit 3); `--hook-dir` optional; `--config`; `--skip-on-missing-config`; trailing
  `-- <args>`.
  For compatibility with pre-commit's `rest[1:]`, drop exactly one leading `--` if present (see the open question
  about the non-`--` case).
- Order is significant and must be kept:
  1. Read stdin for pre-push.
  2. Run the legacy hook.
  3. Check the config.
  4. Validate the argument count.
  5. Build `RunArgs`.
  6. `exit = legacy | run`.
- Legacy run:
  - Refuse if `PRE_COMMIT_RUNNING_LEGACY` **or** `SNP_RUNNING_LEGACY` is set.
  - Set **both** for the child, so that a pre-commit script sitting in `.legacy` cannot recurse into snp and the other
    way round.
  - Executable test: Unix mode bits on `metadata()` (following symlinks); Windows: the file exists.
  - Shebang handling on Windows goes through the same `normalize_cmd` port the run spec uses.
- Stdin for pre-push is a `Vec<u8>` read fully. Parse it with `bstr`-style splitting on `\n` (strip `\r`) and `rsplitn(4,
  ' ')` over ASCII whitespace. **Malformed lines are skipped with a debug log, never a crash** (deviation).
- The missing-config texts are verbatim. `PRE_COMMIT_ALLOW_NO_CONFIG` counts as set when non-empty (`"0"` included);
  `SNP_ALLOW_NO_CONFIG` is accepted as an alias.
- Performance target: hook-impl with an unchanged config should spawn **one** `git rev-parse` before `run`'s own git
  commands. It must not open sqlite or take the store lock (config tracking is a cheap `stat`, §20.5).

### 20.5 Store design (recommended)

1. **Location:** `SNP_HOME`, else `$XDG_CACHE_HOME/snp`, else `~/.cache/snp` (Windows `%USERPROFILE%\.cache\snp`,
   mirroring pre-commit). `PRE_COMMIT_HOME` interplay is an open question.
2. **Do not share pre-commit's `db.db` or env dirs.** Health-check expectations, lock semantics (msvcrt region vs
   `LockFileEx`) and env formats differ. Sharing would corrupt one tool's view of the other.
3. **Content-addressed directories instead of sqlite rows:**
   - `repos/<hex(blake3|sha256(canonical_repo, rev, deps_json))[..16]>/`. `canonical_repo` is the absolute canonical
     path for local directories, which fixes the relative-path collision (§7.10), and the verbatim string for URLs.
   - Inside: `src/` (the clone), `envs/<lang>-<sanitized version>-<short hash of version>/`, and `.snp-repo.json`
     (`{"repo":…, "rev":…, "deps":[…], "created":…}`) written **last** with an atomic rename as the commit marker.
   - Lookup is a single `stat`. Self-heal: a missing marker means "not cloned", so stale entries cannot crash anything.
   - Clone into `scratch/<random>/`, then `rename` into place under the lock. On a rename race (`EEXIST`/`ENOTEMPTY`),
     discard ours and use theirs.
4. **Locking:** `std::fs::File::lock()`/`try_lock()` (stable since 1.89; `flock` on Unix, `LockFileEx` on Windows) on
   `<store>/.lock`.
   - When `try_lock` returns `WouldBlock`, log `[INFO] Locking snp directory` and block.
   - Keep the double-checked pattern: check the marker, lock, re-check.
   - Use per-repo lock files (`repos/<key>.lock`) so that independent repos can be cloned and installed in parallel.
     Use a global lock only for gc and clean.
5. **Config tracking:** `configs/<hash(realpath)>.path`, containing the path bytes. `mark_config_used` is
   `stat`-then-create (`create_new`, ignore `AlreadyExists`). It is lock-free, idempotent and cheap on every hook run.
   Skip it when the store is read-only.
6. **gc:**
   1. Take the global lock.
   2. Read all tracked configs. Configs that are missing or invalid are removed.
   3. Mark used `(repo, rev, deps)` keys with exactly pre-commit's `_mark_used_repos` rules (§9), including "manifest
      unreadable means unused".
   4. Delete each unused repo by **renaming it into `trash/` first, then removing it**, ignoring `NotFound` on missing
      dirs.
   5. Also delete `scratch/*` orphans and patch files older than N days.
   6. Print `{n} repo(s) removed.` (count repo keys).
7. **clean:** take the global lock, `rename(store, store.trash-<pid>)`, then remove. Print `Cleaned {dir}.`.
   - Do not touch pre-commit's directories; `~/.pre-commit` belongs to pre-commit.
   - Create the store **lazily**, so that `clean` on a non-existent store prints nothing. This is a deviation from
     pre-commit's always-`Cleaned` (open question).
8. **Install state:**
   - Write `<env>/.snp-install-state.json` with
     `{"v":1,"additional_dependencies":[…],"language_version":…,"snp_version":…}` atomically, as the commit point.
   - Treat malformed or missing state as not installed, with `rmtree` and a reinstall. Never crash.
   - Run the health check after install with the same "BUG: expected environment …" wording.
9. **Env dir name sanitization:** `<ENV_DIR>-<version>` only when the version matches `^[A-Za-z0-9._+-]{1,64}$`.
   Otherwise use `<ENV_DIR>-h<hex16(hash(version))>`. This avoids nesting and traversal (§8.3).
10. **Local template files** (§7.7): embed them with `include_bytes!` from the reference resources. Version the set
    with `LOCAL_REPO_VERSION` (start at `"1"`) as part of the local repo key.
11. **Clones:**
    - Command sequence exactly as in §7.6, including the shallow-then-full fallback.
    - Set `GIT_TERMINAL_PROMPT=0` for parallel first attempts and retry auth failures sequentially without it, except
      when `CI` is set.
    - Print `[INFO] Initializing environment for {key}.` once per new repo, in **config order**. Buffer completion and
      print in order, so output stays deterministic under parallelism.
12. **Patches** (from run's stash) go to `<store>/patches/patch<epoch>-<pid>`. gc prunes ones older than 7 days.
13. **Crash log:** `<store>/snp.log`. The format mirrors §1.4: version information (snp version, git version, OS/arch,
    exe path) and error information including a `std::backtrace::Backtrace` if `RUST_BACKTRACE` is set. Print
    `Check the log at {path}`.

### 20.6 autoupdate

- Run updates on a bounded worker pool: `std::thread::scope` plus an `mpsc` channel. `jobs` uses the pre-commit
  formula with `std::thread::available_parallelism()` (respects affinity/cgroups) for `0`. Print results as they
  complete, which is identical to pre-commit for `-j1`.
- Per-repo failures, **including git fetch errors**, should become `[repo] <error>` lines with exit 1, so the other
  repos still get written. This deviates from pre-commit's exit 3 abort (open question). Unknown `--repo` gives
  `repos (a, b) were not found in {config}` and exits 1 (4.6.1 behavior).
- Git sequence exactly as in §11.2, including `extensions.partialClone=true`, `--filter=blob:none`, `--tags`,
  `describe … --abbrev=0`/`--exact`, the `tag --points-at` "first with a dot" rule, `rev-parse <tag>` for freeze
  (**tag-object sha for annotated tags**, kept for parity), and `show` + `checkout -- .pre-commit-hooks.yaml`.
  Optimization: `git cat-file blob <rev>:.pre-commit-hooks.yaml` straight to memory instead of a checkout.
- Run migrate-config first (quiet). A missing config should be a FatalError with exit 1 (pre-commit: exit 3; prefer 1
  with the cfgv `is not a file` message).
- YAML rewrite:
  - Port `REV_LINE_RE` exactly with the `regex` crate in multi-line-free per-line mode. The `\s` semantics must match
    Python's Unicode `\s` for str patterns.
  - Keep CRLF and LF.
  - Compute new values with a **PyYAML-compatible scalar emitter** for `{'rev': value}` with style `'`, `"` or
    plain-auto.
  - Write atomically (temp in the same dir + rename) and preserve the file mode.
  - The fallback full reformat needs a PyYAML-compatible block emitter (`indent=4`, indentless sequences
    `-   `, `[]`/`{}` for empties, single-quote quoting rules, width-80 folding). Implement it as a small dedicated
    emitter over the parsed config value tree, golden-tested against `pre-commit` output. **Prefer to avoid the
    reformat:**
    - when the last line lacks `\n`, treat EOF as a line end (the deviation that saves comments);
    - for flow style, fall back to the reformat.
- Keep `# frozen: <tag>` comment semantics exactly (§11.3).

### 20.7 try-repo

- Use a throwaway store in a `tempfile::TempDir` (auto-removed even on error) and the shadow-repo procedure (§12)
  verbatim, including the committer identity `pre-commit <asottile+pre-commit@umich.edu>`. Switching to an snp
  identity is harmless, because the commit is temporary.
- Print the `=`×79 banner and the YAML through the same emitter as §20.6.

### 20.8 hazmat

- `snp hazmat cd|ignore-exit-code|n1` must also accept being invoked as `pre-commit hazmat …`. Existing configs embed
  `pre-commit hazmat` in `entry`, so the run spec must decide whether snp rewrites `entry` tokens
  (`pre-commit` → current exe) or requires `pre-commit` on PATH. This is a key compatibility point; see the open
  questions.
- Implement `_cmd_filenames` by finding the **last** `--`. Keep the error strings, including the Python-repr rendering
  `prefix='sub/'` (use a repr helper: single quotes unless the string contains `'` and no `"`).
- `ignore-exit-code` passes `--` and the filenames through unchanged.

### 20.9 Output, I/O and errors

- Use one `Printer` that writes **stdout** for logs and command output, flushed per line, with color decided once
  (`--color`, `PRE_COMMIT_COLOR`, `std::io::IsTerminal` on **stderr**, `TERM=dumb`, and Windows VT enablement via
  `anstyle-query`/`anstream`).
- Exact log prefixes are in §1.3, including the `[INFO]\x1b[m` quirk when colored.
- EPIPE on stdout: stop writing and exit with the code computed so far. Never panic. Install no SIGPIPE handler (Rust
  ignores SIGPIPE by default, so a write returns `BrokenPipe`).
  **Never use `println!`/`print!`.** They panic on a write error. Use `writeln!(lock, …)` and handle
  `ErrorKind::BrokenPipe`.
- SIGINT: children share the process group and receive it. snp waits for them and exits 130 with
  `Interrupted (^C): KeyboardInterrupt: ` (or a product-specific variant; open question).

### 20.10 Testing

- Port the OBS scenarios of §2.6, §6.4-6.7, §9, §11 and §12 into integration tests that run snp in temp repos with an
  isolated `HOME`/`GIT_CONFIG_GLOBAL`/`GIT_CONFIG_NOSYSTEM`/store. Where a pre-commit binary is available (dev
  machines), add a differential mode that diffs stdout and exit codes.
- Concurrency tests: N parallel `snp install-hooks` on the same store, and gc concurrent with run.
  Crash-injection: kill during a clone and assert no partial repo is used.
- Property tests: `sh_quote` round-trip, pre-push line parser on arbitrary bytes (no panics), `REV_LINE_RE` rewrite
  idempotence.

---

## 21. Open questions

1. **Store sharing and location.** Use `SNP_HOME` → `$XDG_CACHE_HOME/snp` → `~/.cache/snp` (recommended)? Should
   `PRE_COMMIT_HOME` be ignored, used as is (mixing layouts with pre-commit's `db.db`), or used as a parent
   (`$PRE_COMMIT_HOME/snp/`, so existing CI cache keys on `~/.cache/pre-commit`/`PRE_COMMIT_HOME` keep working and
   `pre-commit clean` also wipes snp)? Should a one-shot importer from pre-commit's `db.db` `configs` table seed gc
   tracking?
2. **Hook script identity.** Embed pre-commit's `CURRENT_HASH` in snp scripts, so that pre-commit treats them as its
   own (a reversible swap), or keep them distinct like prek does (no cross-tool cleanup, `.legacy` chains)?
   Recommended: embed it.
3. **Fallback binary in the script.** If `snp` is not found, should the script fall back to `pre-commit` (its argv is
   compatible), or fail with the "not found" message?
4. **`core.hooksPath` policy.** Use pre-commit's exact refusal by default (recommended), and which opt-ins should
   exist: `--force` into hooksPath, ignoring a hooksPath equal to the default dir, allowing repo-scoped hooksPath
   (prek), `--reset-hooks-path` (lefthook)?
5. **`.legacy` clobbering.** Replicate the silent overwrite, or refuse unless `-f` (recommended)? Also: should a
   `.legacy` that is itself a pre-commit/snp script be auto-deleted at install?
6. **User-facing product strings.** Should messages say `pre-commit installed at …`, `Use -f to use only
   pre-commit.`, `To uninstall pre-commit run \`pre-commit uninstall\``, `pre-commit version …` verbatim (tools and
   tests may grep them), or use `snp`? Should `--version` print `pre-commit 4.6.2` (for scripts that parse it), or
   `snp X.Y.Z (pre-commit 4.6.2 compatible)`? Which version is compared against `minimum_pre_commit_version`?
7. **autoupdate failure mode.** Per-repo error lines with exit 1 (recommended), or pre-commit's global exit 3 abort on
   git errors? Should a final newline be treated as optional (keeping comments), instead of triggering a full reformat?
8. **YAML reformat fidelity.** Is byte-identical PyYAML output (including 80-column folding) required for the fallback
   reformat and the try-repo banner, or is semantic equivalence enough?
9. **pre-push semantics.** Keep "first pushed ref only" and pre-commit's force-push handling (recommended for parity),
   or check all refs and use prek's ancestor-aware base? Fix SHA-256 nulls and skip malformed lines (recommended:
   yes).
10. **hook-impl argv.** pre-commit drops the first token after options unconditionally (`rest[1:]`). Should snp drop it
    only when it is `--` (safer), accepting the divergence for manual invocations without `--`?
11. **post-rewrite stdin.** pre-commit gives legacy `post-rewrite` hooks an empty stdin (it never reads git's stdin
    for them). Should snp forward stdin to legacy hooks for `post-rewrite` (a fix), or keep parity?
12. **Lazy store creation.** pre-commit creates the store for every command, which makes `clean` always print
    `Cleaned …`. Is lazy creation, and the resulting output difference, acceptable?
13. **`pre-commit hazmat` in `entry`.** How does snp handle configs whose `entry` invokes `pre-commit hazmat …` (or
    other `pre-commit` subcommands) when pre-commit is not installed? Options: provide a `pre-commit` shim or alias
    (multicall binary via argv[0]), or rewrite the entry.
14. **git ≥ 2.54 config hooks.** Should `snp install` gain a mode that writes `hook.pre-commit.<type>.{event,command}`
    (pre-commit's announced future), and should plain `snp install` detect existing config hooks to avoid double
    execution?
15. **Annotated tags under `--freeze`.** pre-commit stores the tag-object sha. Keep it (recommended, to avoid churn
    between the tools) or peel to the commit (`rev-parse <tag>^{commit}`)?
16. **Crash log name and content.** `snp.log` vs `pre-commit.log`, and how much environment detail to include
    (privacy)?
17. **Relative local-path repos.** Fix the cache-key collision by keying on the canonical absolute path (recommended)?
    It changes nothing user-visible except correctness.


---

## Addendum: `pre-commit hazmat` run from a hook entry: actual semantics (doc 03 §15 is wrong)

Gap-fill #2. This addendum **supersedes §15, §20.8 and open question 13 of this document wherever they conflict**, and it
corrects doc 10 §6 item 1. Nothing above was edited. The errata are in §H.9.

**Provenance**

- **REF**: pre-commit 4.6.2 source (`refs/pre-commit`, commit `c429238`). Citations are relative to `refs/pre-commit/`.
- **ORC**: the 4.6.2 oracle `scratchpad/tmp-test-strategy/venv462/bin/pre-commit` on CPython 3.12.3, git 2.43.0, Linux,
  22 CPUs. It runs in throwaway repos with an isolated `HOME`/`PRE_COMMIT_HOME`/`GIT_CONFIG_GLOBAL` and
  `PATH=/usr/bin:/bin` unless noted.
- **Probe files**: harness `scratchpad/tmp-hazmat/scripts/{probe.sh,cli.sh,sig.sh,argcmp.sh}`, one config per case in
  `scratchpad/tmp-hazmat/cfg/<ID>.yaml`, raw outputs in `scratchpad/tmp-hazmat/out/`.
- **Test repo** (`scratchpad/tmp-hazmat/repo`), 10 tracked files: `b.txt`, `bin/reltool` (`#!/bin/sh`, +x),
  `noexec.sh` (`#!/bin/sh`, mode 644), `noshebang.sh` (+x, no shebang), `sub/a.txt`, `sub/bin/reltool` (+x),
  `sub/d/.keep`, `sub/subonly.sh` (+x), `sub/x y.txt`, `tool.sh` (`#!/bin/sh`, +x).
- 4.1.0 (`~/.local/bin/pre-commit`) predates hazmat (added in 4.5.0, `CHANGELOG.md:55`), so every observation here is
  from 4.6.2.
- **Cross-version argparse**: CPython 3.11.13, 3.12.3, 3.13.14 and 3.14.0rc1. These ran an independent re-creation of
  the hazmat parser definitions (`scratchpad/tmp-hazmat/scripts/argp/hazmat.py`). 3.14.0rc1 was installed temporarily
  with `uv python install` and uninstalled afterwards.

### H.0 TL;DR

1. **There are two execution paths with different error semantics.**
   - **H (in-hook)**: the entry's first two shlex tokens are exactly `pre-commit` `hazmat`, in a language that calls
     `lang_base.hook_cmd`. They are rewritten to `sys.executable -m pre_commit.commands.hazmat …`
     (`lang_base.py:174-178`). This path runs `hazmat.main()` (`hazmat.py:86-95`). It does **not** use `main.py`: no
     `error_handler`, no `logging_handler`, no `Store`, no git, no `--color`/`PRE_COMMIT_COLOR`.
   - **C (CLI)**: anything that reaches `pre-commit hazmat` through `main.py:202-450`. That is a user at a shell, or a hook
     entry the rewrite does not match. That path has `error_handler` (exit 3 plus a crash log) and creates the `Store`.
2. **On path H, "unexpected" failures print a full Python traceback into the hook output and exit 1. The hook shows
   `- exit code: 1`.** This covers executable not found, is a directory, not executable, empty command, missing `cd`
   directory, and exec errors. Exit 3 never happens on path H. Doc 03 §15's "surface as exit 3" applies to path C only.
3. On path H, argparse errors exit **2** with prog **`hazmat.py`**, e.g. `usage: hazmat.py cd [-h] subdir ...` /
   `hazmat.py cd: error: the following arguments are required: subdir, cmd`. `-h` prints help and exits **0**, so the hook
   passes. `SystemExit("msg")` texts (`` hazmat entry must end with `--` ``, `unexpected file without prefix='sub/': x`)
   exit **1**. All of these land in the **hook output**, not on pre-commit's stderr.
4. There is one hazmat process **per xargs partition**. Messages repeat once per partition, and partitions run in
   parallel unless `require_serial`. The hook exit code is xargs' max-abs over the partitions.
5. `n1` ORs the child return codes inside a partition. A signal death is `-N`, so `SystemExit(-9)` gives exit status
   **247** (SIGTERM 241, and `-9 OR 8 = -1` gives 255). `cd` returns the child's code the same way. `ignore-exit-code`
   returns 0 for any child status, **but still 1/2 for resolution, spawn and usage errors**.
6. argparse quirks that are observable:
   - a `--` immediately after `SUBDIR` (or right before it) is **eaten**, so `cd sub --` + files fails with
     ``must end with `--` ``.
   - a leading `--` in `n1`/`ignore-exit-code` is **kept**.
   - a first command token that starts with `-` is an error (`unrecognized arguments: -x`).
   - hook `args:` are appended **after** the entry's trailing `--`, so they are treated as filenames.
7. The rewrite works for `unsupported_script`/`script` too, contrary to the docs: `prefix.path(<absolute exe>)` is a
   no-op (ORC D8). For `julia`, `docker` and `docker_image` it produces a nonsensical command. Nothing else is
   rewritten: not `pre-commit.exe hazmat`, not `/abs/pre-commit hazmat`, not `entry: pre-commit` + `args: [hazmat, …]`,
   not a nested `pre-commit hazmat` inside a hazmat command. Those take path C via PATH.
8. Stdio: hazmat's children inherit the hook's stdio. stdin is an **empty pipe** without color and **`/dev/null`** under the
   pty. stdout and stderr are the merged capture pipe or pty (ORC D10/D11). hazmat captures nothing itself.
9. **snp decision**: rewrite `pre-commit hazmat` **and** `snp hazmat` to `[self_exe, "__hazmat", …]`.
   - **subprocess, not in-process**
   - `__hazmat` is a hidden internal entry point that reproduces path H. It uses a Python-3.12 argparse emulation, prints
     the **final traceback line only** instead of the traceback, and keeps exit codes 1/2/0/child.
   - the public `snp hazmat` reproduces path C.
10. Corrections to other docs are in §H.9 and §H.10:
    - stdin is an empty pipe without color, not `Stdio::null()`.
    - "unrecognized arguments" errors come from the top-level parser.
    - an invalid `PRE_COMMIT_COLOR` errors only when `--color` is absent.
    - do not use the `which` crate (relative PATH entries).

### H.1 The two paths

| | **H: rewritten in-hook** | **C: CLI** |
|---|---|---|
| Reached by | `entry` whose `shlex.split` starts with exactly `['pre-commit', 'hazmat']`, in a language that calls `lang_base.hook_cmd` (§H.2) | A shell or script calling `pre-commit hazmat …`, or a hook entry that is **not** rewritten (§H.2.3), resolved through PATH |
| argv actually run | `(sys.executable, '-m', 'pre_commit.commands.hazmat', *tokens[2:], *hook.args, *partition_files)` (`lang_base.py:174-178`, `xargs.py:166`) | `pre-commit hazmat [--color C] TOOL …` |
| Code path | `hazmat.py:94-95` → `main()` `:86-91` → `impl()` `:74-83` | `main.py:202-370` (argparse, prog `pre-commit`) → `main.py:377-399` `with error_handler(), logging_handler(args.color): … store = Store() … hazmat.impl(args)` |
| Parser prog (3.12) | `hazmat.py` (basename of `sys.argv[0]`, the module file) | `pre-commit hazmat`, `pre-commit hazmat cd`, … and top-level `pre-commit` |
| `--color` / `PRE_COMMIT_COLOR` | not defined. `--color` is an unknown option (§H.3) | accepted between `hazmat` and the tool (`main.py:215-218`, `color.py:103-109`). An invalid env value is an error when `--color` is absent (§H.6) |
| Unexpected exceptions | Uncaught. The **Python traceback goes to stderr**, which is the hook's capture, and the **exit status is 1** | `error_handler` (`error_handler.py:70-81`): stdout `An unexpected error has occurred: <Type>: <msg>` + `Check the log at <store>/pre-commit.log`, crash log written, **exit 3** |
| `SystemExit("msg")` | msg + `\n` to stderr (= hook output), exit 1 | msg + `\n` to stderr, exit 1. `error_handler` catches only `Exception`/`KeyboardInterrupt` |
| argparse error / `-h` | stderr usage+error, exit 2 / stdout help, exit 0, all in the hook output | same on the terminal, exit 2 / 0. Handled **before** `Store()` |
| Store / log side effects | none | `Store()` creates `README`, `db.db`, `.lock` on every parsed invocation, even a successful `n1` (ORC E5-E7). `pre-commit.log` only on exit 3 |
| git needed / chdir | no (hook cwd = repo toplevel) | no (`COMMANDS_NO_GIT`, `main.py:44-47`). Works outside a repo (ORC E7) |
| SIGINT | Python `KeyboardInterrupt` in `subprocess.call` (§H.7.4), traceback, the process dies by SIGINT | `Interrupted (^C): KeyboardInterrupt: ` + log, exit 130 (via `error_handler`), and the child is killed the same way |

### H.2 Where the rewrite applies

#### H.2.1 Rewrite rule (`lang_base.py:174-178`)

```python
cmd = shlex.split(entry)                      # POSIX shlex, no comments
if cmd[:2] == ['pre-commit', 'hazmat']:       # exact, case-sensitive token equality
    cmd = [sys.executable, '-m', 'pre_commit.commands.hazmat', *cmd[2:]]
return (*cmd, *args)                           # hook args appended; xargs appends filenames
```

- Matching happens **after shlex**. `'pre-commit' hazmat …` and `pre-commit  hazmat` (two spaces) are rewritten (ORC D5,
  D12). `pre-commit Hazmat` is not; its PATH lookup then fails at xargs level with ``Executable `pre-commit` not found``
  (no traceback, ORC D13).
- **Only `entry` is inspected.** `entry: pre-commit` with `args: [hazmat, …]` is not rewritten (ORC D7).
- An entry of exactly one token `pre-commit` is not rewritten (`cmd[:2]` has length 1).

#### H.2.2 Per language

| Language(s) | Calls `hook_cmd`? | Result for `entry: pre-commit hazmat …` |
|---|---|---|
| `conda coursier dart dotnet golang haskell lua node perl python ruby rust swift unsupported` (+ alias `system`) | yes, `basic_run_hook` (`lang_base.py:181-196`) | Works. For env languages, `in_env` has already patched `os.environ` (PATH, VIRTUAL_ENV, …), so the hazmat process resolves tools in the hook env |
| `unsupported_script` (+ alias `script`) | yes (`unsupported_script.py:25-26`: `cmd = (prefix.path(cmd[0]), *cmd[1:])`) | **Works** (ORC D8): `Prefix.path` = `normpath(join(prefix_dir, abs))` = `abs`. The docs (`refs-docs/advanced.md:363-364`) say it does not work; they are wrong in practice |
| `julia` | yes (`julia.py:38-40`) | `julia --startup-file=no <sys.executable> -m pre_commit.commands.hazmat …`: julia tries to parse the python binary as a script. It fails |
| `docker` | yes (`docker.py:173-177`) | `docker run … --entrypoint <sys.executable> <tag> -m pre_commit.commands.hazmat …`: the host python path does not exist in the container. It fails |
| `docker_image` | yes (`docker_image.py:26`) | `docker run … <sys.executable> -m …`: the python path is parsed as the **image reference** and docker reports an invalid reference. It fails. (Not rewriting would be worse: docker would try to pull an image named `pre-commit`.) |
| `pygrep`, `fail` | no | The entry is a regex or message. No rewrite |
| `r` | no (`r.py:183`, own `shlex.split`) | ``ValueError('entry must start with `Rscript`.')`` → exit 3 |

#### H.2.3 Forms that are NOT rewritten and take path C through PATH (verified)

| Entry form | ORC | Observed |
|---|---|---|
| `/abs/path/to/pre-commit hazmat n1 nope-not-here --` | D6 | Hook `- exit code: 3`, output ``An unexpected error has occurred: ExecutableNotFoundError: Executable `nope-not-here` not found`` / `Check the log at <PRE_COMMIT_HOME>/pre-commit.log`. The crash log goes to the **same** store |
| `entry: pre-commit` + `args: [hazmat, n1, nope-not-here, --]`, 4.6.2 on PATH | D7 | identical to D6 |
| `pre-commit hazmat cd sub pre-commit hazmat n1 echo -- --` (nested) | F3 | The inner `pre-commit` is resolved by hazmat's `normalize_cmd`. Without pre-commit on PATH: traceback ``…ExecutableNotFoundError: Executable `pre-commit` not found``, exit 1. With it: works (`a.txt`) |

### H.3 Argument parsing on path H (argparse, CPython 3.12.3)

Definitions (`hazmat.py:10-27`):
- top level: `ArgumentParser()` with `add_subparsers(dest='tool')`. It is **not required**, and there is no `--color`.
- subcommands: `cd` with `subdir` and `cmd=REMAINDER`; `ignore-exit-code` with `cmd=REMAINDER`; `n1` with `cmd=REMAINDER`.

The exact rules below were verified with the oracle and with `argcmp.sh` on 3.11, 3.12 and 3.13.

**Top level** (tokens after `-m pre_commit.commands.hazmat`, i.e. `tokens[2:] + args + files`):

| Input | Result |
|---|---|
| no tokens at all | `Namespace(tool=None)`, then `impl` runs `args.cmd = tuple(args.cmd)` (`hazmat.py:75`) → **`AttributeError: 'Namespace' object has no attribute 'cmd'`**, traceback, exit 1 (ORC D1) |
| first token `-h`, `-h<chars>` (e.g. `-hx`), or a prefix of `--help` with ≥3 chars (`--h`, `--he`, `--hel`, `--help`) | top-level help on stdout, exit 0 (ORC D3) |
| first token is `cd` / `ignore-exit-code` / `n1` | sub-parse (below) |
| any other first token, including `--`, `-`, `''`, negative numbers `-1`, and tokens containing a space | stderr ``usage: hazmat.py [-h] {cd,ignore-exit-code,n1} ...`` + ``hazmat.py: error: argument tool: invalid choice: '<tok>' (choose from 'cd', 'ignore-exit-code', 'n1')``, exit 2. `<tok>` is Python `repr` (ORC D2, D4: `'b.txt'` when the entry is bare `pre-commit hazmat` and files follow) |
| other option-like first tokens (`-x`, `--color`, …) | treated as unknown options (no arguments). The next positional is then taken as the tool. `--color never n1 echo --` gives `invalid choice: 'never'` (ORC D9). If the tool is valid, the error is `hazmat.py: error: unrecognized arguments: --color`, exit 2 |

**Sub-parsers** (prog `hazmat.py <tool>`):

| Tool | Rule | Examples (3.12) |
|---|---|---|
| all | Leading option-like tokens (start with `-`, not exactly `-` or `--`, not a negative number, no space) are processed as options. Help forms print the tool help and exit 0. Others become "extras". They are reported **by the top-level parser** as `hazmat.py: error: unrecognized arguments: <extras joined by ' '>` with the **top-level** usage line, exit 2. Option-like tokens **after** the first positional belong to `cmd` | `n1 -x --` → `unrecognized arguments: -x`. `n1 echo -h --` → cmd `['echo','-h','--']` (ORC B12). `cd sub -x --` → cmd `['-x','--']`. `cd -x sub --` → `unrecognized arguments: -x` |
| `n1`, `ignore-exit-code` | `cmd` = the remaining tokens, **verbatim**. A leading `--` is **kept**. An empty `cmd` is allowed. REMAINDER is not "required" when it matches empty | `n1 -- echo -- f` → cmd `['--','echo','--','f']`. `n1` → cmd `[]`. `ignore-exit-code -- x` → cmd `['--','x']` |
| `cd` | `subdir` = the first positional. Only the **first** `--` among the sub-arguments is special: if it is immediately before or immediately after `SUBDIR`, it is consumed. Later `--` tokens stay in `cmd`. No positional at all gives the required-args error | `cd sub -- echo -- f` and `cd -- sub echo -- f` → subdir `sub`, cmd `['echo','--','f']` (ORC A17, A18). `cd sub --` → cmd `[]`. `cd sub -- --` → cmd `['--']`. `cd -- -d echo --` → subdir `-d`. `cd` → stderr `usage: hazmat.py cd [-h] subdir ...` + `hazmat.py cd: error: the following arguments are required: subdir, cmd`, exit 2 (ORC A9) |

**Help texts on path H** (stdout, exit 0, verified byte for byte; each ends with `\n`):

```
usage: hazmat.py [-h] {cd,ignore-exit-code,n1} ...

positional arguments:
  {cd,ignore-exit-code,n1}
    cd                  cd to a subdir and run the command
    ignore-exit-code    run the command but ignore the exit code
    n1                  run the command once per filename

options:
  -h, --help            show this help message and exit
```
```
usage: hazmat.py cd [-h] subdir ...

positional arguments:
  subdir
  cmd

options:
  -h, --help  show this help message and exit
```
```
usage: hazmat.py n1 [-h] ...

positional arguments:
  cmd

options:
  -h, --help  show this help message and exit
```
(`ignore-exit-code`: identical to `n1` with `usage: hazmat.py ignore-exit-code [-h] ...`.) Error form: the usage line,
then `<prog>: error: <msg>`, both on stderr.

**Python-version variance of path H texts.** Users' pre-commit may run on 3.10-3.14, so these texts are not stable in
the wild:

| Aspect | 3.11 / 3.12 | 3.13 | 3.14 (rc1) |
|---|---|---|---|
| prog | `hazmat.py` | `hazmat.py` | `<basename(sys.executable)> -m pre_commit.commands.hazmat` (e.g. `python3.14 -m …`; usage wraps at 80 columns) |
| `cd` with no args | `…required: subdir, cmd` | `…required: subdir` | `…required: subdir` |
| invalid choice | `(choose from 'cd', 'ignore-exit-code', 'n1')` | same | `(choose from cd, ignore-exit-code, n1)` |
| ANSI color in usage/help under the pty (`--color always`) | no | no | **yes** (`\x1b[1;34musage: \x1b[0m…`) unless `NO_COLOR`, `PYTHON_COLORS=0` or `TERM=dumb` |
| `--` / REMAINDER handling | as above | as above | as above (same results for all vectors tested) |

argparse wraps help at `$COLUMNS-2`. A pipe or a 0×0 pty falls back to 80. All path H strings fit in 80 columns.

### H.4 Tool algorithms (`hazmat.py:30-83`) and check order

`_cmd_filenames(cmd)` (`:30-41`): find the **last** `--` (scanning from the end). Return `cmd[:i]`, `cmd[i+1:]`.
- If there is none: ``SystemExit('hazmat entry must end with `--`')``.
- A **filename equal to `--`** becomes the split point. That is parity, not a bug to fix.

| Tool | Steps, in order (first failure wins) | Return |
|---|---|---|
| `cd SUBDIR CMD… -- FILES…` (`:44-56`) | 1. `_cmd_filenames`. 2. `prefix = SUBDIR + '/'`. Then **for each file in order**: if not `file.startswith(prefix)`, ``SystemExit(f'unexpected file without {prefix=}: {file}')``. Only the **first** offending file of the partition is reported. The file is printed raw and the prefix with `repr`. 3. `normalize_cmd(head)`, resolved against the **current** cwd (the toplevel) and the env. 4. `subprocess.call((*cmd, *stripped), cwd=SUBDIR)` | child returncode |
| `ignore-exit-code CMD…` (`:59-62`) | 1. `normalize_cmd(cmd)` on the **whole** cmd. `--` and the filenames are kept as arguments; an empty cmd raises `IndexError`; if cmd came only from filenames, the **first filename becomes the program** (ORC C7b). 2. `subprocess.call(cmd)` | `0` (even if the child was killed) |
| `n1 CMD… -- FILES…` (`:65-71`) | 1. `_cmd_filenames`. 2. `normalize_cmd(head)` **once**, even with zero files, so a missing exe fails even when no file is passed. 3. For each file **sequentially**: `ret \|= subprocess.call((*cmd, file))`. A spawn `OSError` aborts the loop (traceback, exit 1) | bitwise OR of returncodes (Python ints, two's complement) |

- `SUBDIR` is not validated. Absolute paths and `..` work, and a trailing `/` gives prefix `sub//`, which never matches
  (ORC A16).
- The filenames for `cd` are the ones passed **after** the last `--`. With hook `args:` non-empty and the documented
  entry ending in `--`, the args become "files". With `cd` that gives `unexpected file without prefix='sub/': --fix`
  (ORC F1). With `n1` the args are run as separate invocations (`echo --fix`, ORC F2).
- `args: [--x, --]` with an entry **without** the trailing `--` works as intended (ORC B14).
- `cd` runs the command **once even with zero files** (pass_filenames false / always_run).

**`normalize_cmd` inside hazmat** (`parse_shebang.py:65-85`) is the same function xargs uses, with the hazmat process's
environment. That environment is the hook env.
- A bare name is searched in PATH with `os.path.join(dir, name)`. A **relative PATH entry yields a relative path**,
  which the OS then resolves against the **child's** cwd (`SUBDIR` for `cd`).
  - ORC A12: `PATH=bin:/usr/bin:/bin`, `cd sub reltool --`. The resolver returns `bin/reltool`, whose `#!/bin/sh`
    turns the command into `/bin/sh bin/reltool a.txt`, run in `sub/`. Output: `SUB-bin-tool pwd=<root>/sub a.txt`.
- A path token is checked at the toplevel but executed relative to `SUBDIR`:
  - A13: `./tool.sh` exists only at the root and has a shebang, so `/bin/sh ./tool.sh` runs in `sub/`: hook exit **2**,
    `/bin/sh: 0: cannot open ./tool.sh: No such file`.
  - A14: exists only in `sub/` → ``Executable `./subonly.sh` not found``.
  - A15: no shebang, exists only at the root → `FileNotFoundError: [Errno 2] No such file or directory: './noshebang.sh'`.

### H.5 Observed path H results (ORC; `pre-commit run -c cfg/<ID>.yaml --all-files --color never`; `language: system` unless noted)

"Final line" means the last line of a Python traceback. The full traceback precedes it, with 5-10 frames that contain
the venv's absolute paths. "×N" means the message appears once per partition (10 files → 3 partitions, 5 files → 2,
≤4 files → 1, on this 22-CPU host; `max_args = max(4, ceil(n/jobs))`, `xargs.py:86`).

| ID | entry after `pre-commit hazmat` / other keys | hook exit | hook output |
|---|---|---|---|
| A1 | `cd sub sh -c 'pwd; printf "<%s>" "$@"; echo' sh --`, `files: ^sub/` (5 files) | 0 | 2 partitions (shuffled): `<root>/sub` `<bin/reltool><a.txt><d/.keep><subonly.sh>` `<root>/sub` `<x y.txt>` |
| A2 | `cd sub echo --`, `require_serial: true` (10 files, git order) | 1 | `unexpected file without prefix='sub/': b.txt` |
| A3 | `cd sub nope-not-here --`, `files: ^sub/` | 1 | traceback ×2, final ``pre_commit.parse_shebang.ExecutableNotFoundError: Executable `nope-not-here` not found`` |
| A4 | `cd sub ./sub --` | 1 | final ``…ExecutableNotFoundError: Executable `./sub` is a directory`` |
| A4b | `cd sub sub --` (bare name of a directory) | 1 | final ``…ExecutableNotFoundError: Executable `sub` not found`` (PATH search uses `isfile`) |
| A5 | `cd sub ./noexec.sh --` (mode 644) | 1 | final ``…ExecutableNotFoundError: Executable `./noexec.sh` is not executable`` |
| A6 | `cd sub echo` | 1 | ``hazmat entry must end with `--` `` ×2 |
| A7 | `cd nosuch echo --`, `pass_filenames: false`, `always_run: true` | 1 | final `FileNotFoundError: [Errno 2] No such file or directory: 'nosuch'` (from `subprocess.py` `_execute_child`) |
| A8 | `cd sub --`, `files: ^sub/` | 1 | ``hazmat entry must end with `--` `` ×2 (the `--` was eaten after SUBDIR, so cmd = files only) |
| A9 | `cd`, `pass_filenames: false` | **2** | `usage: hazmat.py cd [-h] subdir ...` / `hazmat.py cd: error: the following arguments are required: subdir, cmd` |
| A10 | `cd sub`, `pass_filenames: false` | 1 | ``hazmat entry must end with `--` `` |
| A11 | `cd -h` | 0 | cd help (§H.3) |
| A16 | `cd sub/ echo --` | 1 | `unexpected file without prefix='sub//': sub/a.txt` |
| A19 | `cd "it's" echo --` | 1 | `unexpected file without prefix="it's/": sub/a.txt` (repr picks `"`) |
| B1 | `n1 sh -c 'echo "[$#] $1"' sh --` (4 files) | 0 | `[1] bin/reltool` `[1] b.txt` `[1] sub/a.txt` `[1] sub/x y.txt` |
| B2 | `n1 …` where a.txt exits 1 and b.txt exits 2 | **3** | (empty) |
| B2b | both exit 2 | **2** | (empty): OR, not sum or max |
| B3/B4/B5 | `n1 nope-not-here --` / `n1 ./sub --` / `n1 ./noexec.sh --` (10 files) | 1 | final line ×3 (`not found` / `is a directory` / `is not executable`) |
| B6 | `n1 echo` | 1 | ``hazmat entry must end with `--` `` |
| B7 | `n1 sh -c 'kill -9 $$' sh --` | **247** | (empty) |
| B7c | `… kill -15 …` | **241** | (empty) |
| B7b | a.txt killed by 9, b.txt exits 8, `require_serial` | **255** | (empty): `-9 OR 8 = -1` → 255 |
| B8 | `n1 -h` | 0 | n1 help |
| B9 | `n1`, `pass_filenames: false` | 1 | ``hazmat entry must end with `--` `` |
| B10 | `n1 --` (files follow) | 1 | final `IndexError: tuple index out of range` (cmd before the last `--` is empty) |
| B11 | `n1 -- echo --` | 1 | final ``…ExecutableNotFoundError: Executable `--` not found`` (leading `--` kept) |
| B13 | `n1 ./noshebang.sh --` | 1 | final `OSError: [Errno 8] Exec format error: './noshebang.sh'` (**no `/bin/sh` fallback**) |
| C1 | `ignore-exit-code sh -c 'printf "<%s>" "$@"; echo; exit 7' sh --`, `require_serial` | 0 | `<--><b.txt><bin/reltool><sub/a.txt>` (`--` passed through literally) |
| C2-C4 | `ignore-exit-code` + missing / dir / noexec | **1** | final line ×3 |
| C5 | child `kill -9 $$` | 0 | (empty) |
| C6 | `ignore-exit-code -h` | 0 | help |
| C7 | `ignore-exit-code`, `pass_filenames: false` | 1 | final `IndexError: tuple index out of range` |
| C7b | `ignore-exit-code` + files `b.txt bin/reltool` | 1 | final ``…ExecutableNotFoundError: Executable `b.txt` not found`` |
| C8 | `ignore-exit-code ./noshebang.sh` | 1 | final `OSError: [Errno 8] Exec format error: './noshebang.sh'` |
| D1 | (nothing), `pass_filenames: false` | 1 | final `AttributeError: 'Namespace' object has no attribute 'cmd'` |
| D2 | `bogus` | **2** | `usage: hazmat.py [-h] {cd,ignore-exit-code,n1} ...` / `hazmat.py: error: argument tool: invalid choice: 'bogus' (choose from 'cd', 'ignore-exit-code', 'n1')` |
| D3 | `-h` | 0 | top-level help |
| D4 | (nothing) + files | **2** | as D2 with `'b.txt'` |
| D9 | `--color never n1 echo --` | **2** | as D2 with `'never'` |
| D14 | `cd` + 2 files | 1 | ``hazmat entry must end with `--` `` (the first file became SUBDIR) |
| F5 | `ignore-exit-code sh -c 'printf "[%s]" "$@"; echo; exit 3' sh` + `args: [--fix]` | 0 | `[--fix][b.txt][bin/reltool]` |

With `--color always` (pty), the bytes are identical apart from pre-commit's own colored status lines (ORC A3-color,
A9-color): no `\r\n` (ONLCR is off) and no argparse color on 3.12.

Example of what snp replaces (ORC A3, `<site>` = `…/venv462/lib/python3.12/site-packages`):

```
Traceback (most recent call last):
  File "<frozen runpy>", line 198, in _run_module_as_main
  File "<frozen runpy>", line 88, in _run_code
  File "<site>/pre_commit/commands/hazmat.py", line 95, in <module>
    raise SystemExit(main())
                     ^^^^^^
  File "<site>/pre_commit/commands/hazmat.py", line 91, in main
    return impl(args)
           ^^^^^^^^^^
  File "<site>/pre_commit/commands/hazmat.py", line 77, in impl
    return cd(args.subdir, args.cmd)
           ^^^^^^^^^^^^^^^^^^^^^^^^^
  File "<site>/pre_commit/commands/hazmat.py", line 55, in cd
    cmd = normalize_cmd(cmd)
          ^^^^^^^^^^^^^^^^^^
  File "<site>/pre_commit/parse_shebang.py", line 77, in normalize_cmd
    exe = normexe(cmd[0], env=env)
          ^^^^^^^^^^^^^^^^^^^^^^^^
  File "<site>/pre_commit/parse_shebang.py", line 53, in normexe
    _error('not found')
  File "<site>/pre_commit/parse_shebang.py", line 48, in _error
    raise ExecutableNotFoundError(f'Executable `{orig}` {msg}')
pre_commit.parse_shebang.ExecutableNotFoundError: Executable `nope-not-here` not found
```

### H.6 Observed path C results (ORC; direct `pre-commit …`, fresh `PRE_COMMIT_HOME` each time)

| ID | argv | exit | stdout / stderr | store after |
|---|---|---|---|---|
| E1 | `hazmat` | **3** | stdout `An unexpected error has occurred: AttributeError: 'Namespace' object has no attribute 'cmd'` / `Check the log at <home>/pre-commit.log` | README db.db .lock pre-commit.log |
| E2 | `hazmat cd` | 2 | stderr `usage: pre-commit hazmat cd [-h] subdir ...` / `pre-commit hazmat cd: error: the following arguments are required: subdir, cmd` | (none) |
| E3 | `hazmat bogus` | 2 | stderr `usage: pre-commit hazmat [-h] [--color {auto,always,never}]` / `                         {cd,ignore-exit-code,n1} ...` / `pre-commit hazmat: error: argument tool: invalid choice: 'bogus' (choose from 'cd', 'ignore-exit-code', 'n1')` | (none) |
| E4 | `hazmat n1 nope-not-here -- a` | **3** | stdout ``An unexpected error has occurred: ExecutableNotFoundError: Executable `nope-not-here` not found`` / `Check the log at …` | + pre-commit.log |
| E5 | `hazmat n1 echo` | 1 | stderr ``hazmat entry must end with `--` `` | README db.db .lock |
| E6 | `hazmat cd sub echo -- sub/a.txt b.txt` | 1 | stderr `unexpected file without prefix='sub/': b.txt` | README db.db .lock |
| E7 | `hazmat n1 echo -- a b` (outside git) | 0 | stdout `a` `b` | README db.db .lock |
| E8 | `hazmat -h` | 0 | stdout: the E3 usage (2 lines), the §H.3 top-help body, plus `  --color {auto,always,never}` / `                        Whether to use color in output. Defaults to `auto`.` | (none) |
| E8b-d | `hazmat cd -h`, `hazmat n1 -h`, `hazmat ignore-exit-code -h` | 0 | as the §H.3 texts, with `usage: pre-commit hazmat <tool> …` | (none) |
| E9, G5-G7 | `hazmat cd <dir> echo --` with dir missing / a file / mode 000 | **3** | `An unexpected error has occurred: FileNotFoundError: [Errno 2] No such file or directory: 'nosuch'` / `NotADirectoryError: [Errno 20] Not a directory: 'afile'` / `PermissionError: [Errno 13] Permission denied: 'noperm'` | + pre-commit.log |
| E10 | `hazmat n1 sh -c 'kill -9 $$' sh -- a` | **247** | (none) | README db.db .lock |
| E11 | `hazmat --color never n1 echo -- a` | 0 | `a` | README db.db .lock |
| E12, G9 | `hazmat n1 --` / `hazmat ignore-exit-code` | **3** | `An unexpected error has occurred: IndexError: tuple index out of range` | + pre-commit.log |
| E13 | `hazmat ignore-exit-code sh -c 'exit 5'` | 0 | (none) | README db.db .lock |
| E14 | `hazmat n1` | 1 | stderr ``hazmat entry must end with `--` `` | README db.db .lock |
| G1 | `hazmat n1 -x --` | 2 | stderr: the **top-level** usage `usage: pre-commit [-h] [-V]` / `                  {autoupdate,clean,gc,hazmat,init-templatedir,install,install-hooks,migrate-config,run,sample-config,try-repo,uninstall,validate-config,validate-manifest,help,hook-impl}` / `                  ...` / `pre-commit: error: unrecognized arguments: -x` | (none) |
| G3 | `PRE_COMMIT_COLOR=bogus` `hazmat n1 echo -- a` | 2 | stderr (E3 usage) + `pre-commit hazmat: error: argument --color: invalid use_color value: 'bogus'` | (none) |
| G8 | `PRE_COMMIT_COLOR=` (empty) same | 2 | `… invalid use_color value: ''` | (none) |
| G4 | `PRE_COMMIT_COLOR=bogus` `hazmat --color never n1 echo -- a` | 0 | `a` (the env default is never converted when the flag is given) | README db.db .lock |

The in-hook equivalents of G5-G7 (run via `python -m pre_commit.commands.hazmat …`) give the same final lines with exit
1.

### H.7 Stdio, cwd, environment and signals on path H

#### H.7.1 Stdio

hazmat never captures. `subprocess.call` inherits fds 0/1/2 from the hazmat process, and those are what xargs gave the
hook:

| Mode | stdin | stdout | stderr | Evidence |
|---|---|---|---|---|
| no color (`cmd_output_b`) | **pipe**, closed immediately by `communicate()` → EOF | pipe | same pipe (`stderr=STDOUT`) | `util.py:81-83` (`setdefault` PIPE), `:103-107`, `xargs.py:171-173`. ORC D10 (through hazmat) and D11 (plain): `readlink /proc/self/fd/0` = `pipe:[N]`, `[ -t 1 ]` false, `cat \| wc -c` = `0` |
| color, POSIX (`cmd_output_p`) | **`/dev/null`** | pty slave (OPOST/ONLCR off, no setsid, 0×0) | same pty | `util.py:176-178`. ORC D10/D11 with `--color always`: `/dev/null`, `tty1`, `0` |
| Windows | pipe (closed) | pipe | same pipe | `util.py:201-202` (`cmd_output_p = cmd_output_b`) |

- Output ordering is the time order of writes. hazmat writes its own error before or after the child runs, never
  concurrently.
- Help goes to stdout and errors go to stderr. Both end up in the same merged stream.

#### H.7.2 cwd and environment

- The hazmat process's cwd is the hook's cwd, i.e. the toplevel. `cd` sets `cwd=SUBDIR` for the child only.
- The environment is the hook env: the full process env plus the language `in_env` patches. It is inherited unchanged.
  `PRE_COMMIT_COLOR` is not read on path H.

#### H.7.3 Exit status of the hazmat process

`raise SystemExit(main())` (`hazmat.py:95`). An integer `n` becomes the process status:
- POSIX: `n & 0xFF`, so -9 → 247, -15 → 241, -1 → 255.
- Windows: the 32-bit value. Some CPython versions mishandle values ≥ 2^31 (gh-125842).

Child statuses come from `subprocess.call`: a negative signal number on POSIX, a DWORD on Windows. xargs then takes
max-abs across the partitions (`xargs.py:179-181`) and prints it as `- exit code: N`.

#### H.7.4 SIGINT (ORC, `scripts/sig.sh`)

On ^C:
1. `Popen.wait` waits up to **0.25 s** for the child (`_sigint_wait_secs`).
2. `subprocess.call`'s `except:` then calls **`p.kill()` (SIGKILL)**.
3. `KeyboardInterrupt` propagates. A traceback ending in `KeyboardInterrupt` is printed, and the process dies by SIGINT
   (shell `rc=130`).

A child that ignores SIGINT (`trap "" INT; sleep 1.5; echo survived > marker`) was killed: no marker was written.

All of this sits behind pre-commit's own `Interrupted (^C)` exit 130 in the parent, so it is invisible in the hook output.

### H.8 snp specification

#### H.8.1 Rewrite (in snp's `hook_cmd`)

- After the Python-compatible shlex split (doc 04a §14.3), if `tokens.len() >= 2` and `tokens[1] == "hazmat"` and
  `tokens[0]` is **`"pre-commit"` or `"snp"`**, replace `tokens[0..2]` with `[self_exe, "__hazmat"]`. Then append
  `args`. xargs appends the files.
  - Exact, case-sensitive matching. Do **not** match paths, `pre-commit.exe`, `args`, or nested occurrences (parity,
    §H.2.3).
  - Rewriting `snp hazmat` is an snp extension. Without it, snp would resolve a possibly different `snp` from PATH and
    get path C semantics.
- Apply it in exactly the languages listed in §H.2.2 as calling `hook_cmd`, including `script`/`unsupported_script`,
  `julia`, `docker` and `docker_image`. Do not special-case them, because the nonsense results are parity and are safer
  than an image pull. Do not apply it to `pygrep`, `fail`, `r` or meta hooks.
- **`self_exe`** is computed once (`OnceLock<Result<PathBuf, …>>`):
  1. `std::env::current_exe()`.
  2. Then `std::path::absolute()`, so it never becomes relative. This matters for the `script` prefix join and for
     callers' cwd changes.
  3. Do **not** canonicalize. Canonicalizing would resolve symlinks and produce `\\?\` paths on Windows.
  4. On Linux, if the path does not exist and ends with ` (deleted)` (binary replaced during an upgrade), use
     `/proc/self/exe` when it exists.
  5. If all of this fails, the hook result is `(1, "snp: cannot locate the running snp executable: <io error>\n")`
     without spawning.
- The rewritten argv goes through the normal xargs path: `normalize_cmd(self_exe)` passes, and partitions are computed
  with `self_exe` + `__hazmat` counted in the length.
  - Partition boundaries can differ from pre-commit's, whose prefix is `sys.executable -m pre_commit.commands.hazmat`.
    This is visible only near the ARG_MAX limit (Tier C).

#### H.8.2 Process model: **subprocess, not in-process**

Run `__hazmat` as a real child process, exactly like any hook command (one per partition, pipe/pty capture, parallel
unless `require_serial`).

In-process emulation is **rejected**. It saves about 1 ms per partition, but it would have to re-create everything a
process boundary gives for free:
- the hook env for `normalize_cmd`
- cwd handling
- the children writing into the partition's capture
- `SystemExit(-N) & 0xFF` status folding
- the SIGINT kill-after-0.25 s behaviour
- isolation from a hang or crash

This is consistent with 09 §0/§2.2 ("run once per partition for parity"). The partition-visible effects in §H.5
(repeated messages, first offending file per partition, OR inside a partition and max-abs across partitions) are further
evidence for that decision.

#### H.8.3 `__hazmat` entry point (hidden)

- Dispatch on raw `std::env::args_os()` **before** clap:
  `if argv.get(1) == Some("__hazmat") { std::process::exit(hazmat::in_hook_main(&argv[2..])) }`.
  - No clap: user tokens such as `-h` or `--version` must not be interpreted by clap.
  - No logging or color setup, no config, git, store, tokio, rayon or thread pool. Target startup is well under 1 ms.
  - argv is `OsString`. Never panic on non-UTF-8 input. Output is rendered lossily, and non-UTF-8 bytes are escaped as
    Python's `backslashreplace` would print surrogates (`\udcXX`); Tier C, because pre-commit cannot even get there.
- Do not list `__hazmat` in help or in the "invalid choice … (choose from …)" list. pre-commit's top-level choice list
  must stay byte-identical (doc 10 CLI-02).
- `__hazmat` must behave identically whether snp was started as `snp` or through a `pre-commit` symlink/multicall name.

#### H.8.4 Parser for `__hazmat`

Implement the rules of §H.3 by hand: about 80 lines, no argparse crate.
- Pin the **CPython 3.12 texts and prog `hazmat.py`**. They match the reference oracle and 3.10-3.12 users.
- Print help to stdout and exit 0.
- Usage errors: print the usage line + `<prog>: error: <msg>` to stderr and exit 2.
- "unrecognized arguments" is always reported with the **top-level** usage and prog `hazmat.py`.
- The quoted token in `invalid choice: …` uses a Python-`repr` helper. That is the same helper as for `prefix=` (doc 03
  §20.8): single quotes unless the string contains `'` and no `"`; escape `\\`, `\n`, `\r`, `\t` and non-printables.
- Tier A: exit codes and stream. Tier B: message bytes, because the oracle's Python version determines them (§H.3 table).

#### H.8.5 Tool execution in `__hazmat`

- Follow §H.4 exactly: last-`--` split, check order, prefix check on raw strings (`str::starts_with`/`strip_prefix` on
  UTF-8; `OsStrExt::as_bytes` on Unix / `encode_wide` on Windows as fallbacks). Use **no `Path::strip_prefix`**: it is
  component-based and would accept `sub//`.
- The `n1` OR uses `i32` (identical to Python for these ranges).
- Resolve with the **same** `find_executable`/`normexe`/`normalize_cmd` implementation as the hook runner (doc 04a §1.8,
  §14.3), reading this process's env.
  - Return relative results for relative PATH entries. Do **not** use the `which` crate (§H.10 item 5).
- Spawn with `std::process::Command` and **`.status()`**, never `.output()`. stdio is inherited (it is the hook's
  pipe/pty). There are no env changes. `cd` sets `current_dir(SUBDIR)`.
  - **Relative program path under `cd`**: Rust documents the resolution as platform-specific. Make it explicit.
    - POSIX: program = `std::path::absolute(Path::new(SUBDIR).join(prog))`, with `CommandExt::arg0(prog)` so the
      child's `argv[0]` stays the original token (matches Python: exec after chdir).
    - Windows: program = `std::path::absolute(prog)` against the parent cwd (CreateProcess semantics, which Python
      inherits).
    - A bare name, possible only through an empty PATH entry, is treated as `./name` (Tier C).
  - **No `/bin/sh` fallback on ENOEXEC.** ORC B13/C8 require `OSError: [Errno 8] Exec format error: '<prog>'`. Rust's
    posix_spawn path does not fall back, but its fork+`execvp` path does on glibc.
    - Keep the spawn posix_spawn-eligible: no `pre_exec`, uid/gid or groups.
    - Gate it with a test (HAZ-B13).
- Map `ExitStatus` to a Python returncode: `code()`, else on Unix `-signal()`. On Windows use the full code as `i32`.
- Exit via `std::process::exit(rc)`: POSIX truncates to `rc & 0xFF`, Windows passes the u32 bits. Flush stdout first.
  Never use `println!`/`eprintln!` (they panic on EPIPE); use `writeln!` on locked handles and ignore write errors.

#### H.8.6 What replaces the Python traceback

Print **only the exact final traceback line** + `\n` to stderr, then exit **1**:
- It is byte-identical to the last line of pre-commit's hook output, so it can be grepped and compared in tests.
- It is concise.
- Never print a fake `Traceback (most recent call last):`.

| Condition | Line (exit 1) |
|---|---|
| no tool token | `AttributeError: 'Namespace' object has no attribute 'cmd'` |
| empty command (`n1 --`, `cd SUB -- --`, bare `ignore-exit-code`) | `IndexError: tuple index out of range` |
| resolution failure (first exe or shebang interpreter) | ``pre_commit.parse_shebang.ExecutableNotFoundError: Executable `<orig>` not found`` / `… is a directory` / `… is not executable`. `<orig>` is the raw token, not repr. "is not executable" never occurs on Windows (`parse_shebang.py:59`) |
| shebang read error (e.g. +x but unreadable) | `PermissionError: [Errno 13] Permission denied: '<path>'` (OSError rule below) |
| spawn error, `cd` directory problem | `<Class>: [Errno <n>] <strerror>: <repr(SUBDIR)>`, using the spawn's errno. Rust reports the child's failed `chdir` errno. The filename is decided **after** the failure: if SUBDIR is not an existing directory (`fs::metadata`) or not searchable (`rustix::fs::access(EXEC_OK)`), it is SUBDIR, otherwise it is the program. This keeps Python's check order (prefix check → `normalize_cmd` → chdir+exec) and adds no syscalls on the success path. ORC A7/G5-G7 |
| spawn error, exec | `<Class>: [Errno <n>] <strerror>: <repr(program as given to Popen)>` |

- `<Class>` follows CPython's errno → OSError subclass map:
  - `ENOENT` → `FileNotFoundError`
  - `EACCES`, `EPERM` → `PermissionError`
  - `ENOTDIR` → `NotADirectoryError`
  - `EISDIR` → `IsADirectoryError`
  - `EAGAIN`, `EALREADY`, `EWOULDBLOCK`, `EINPROGRESS` → `BlockingIOError`
  - `ECHILD` → `ChildProcessError`
  - `EEXIST` → `FileExistsError`
  - `EINTR` → `InterruptedError`
  - `ESRCH` → `ProcessLookupError`
  - `ETIMEDOUT` → `TimeoutError`
  - anything else → `OSError`
- `<strerror>` is `io::Error::from_raw_os_error(n).to_string()` with the trailing ` (os error N)` removed. That is libc
  `strerror_r`, as in CPython.
- On Windows CPython prints `[WinError N] <FormatMessage text>`. The exact filename suffix is unverified (§H.11).
- This differs from the **xargs-level** spawn-error format, which has no class name: `[Errno 8] Exec format error:
  './x'` (doc 02 §11.8, `util.py:86-87`). Do not share the formatter blindly.
- `SystemExit` messages: print the exact text + `\n` to stderr, exit 1:
  - ``hazmat entry must end with `--` ``
  - `unexpected file without prefix=<repr(prefix)>: <file>`
- Internal bugs must not panic (global snp policy). As defense in depth, wrap `in_hook_main` so that any escaped error
  prints `snp: internal error: <msg>` and exits 1 (Python's uncaught-exception code).

#### H.8.7 SIGINT (SHOULD, P2)

Replicate §H.7.4:
1. On SIGINT, wait ≤ 250 ms for the child.
2. Then SIGKILL it.
3. After reaping, die by SIGINT (`signal_hook::low_level::emulate_default_handler(SIGINT)`).

Use `signal-hook` (safe) with a dedicated thread, which avoids PID-reuse races:
- the main thread blocks in `rustix::process::waitid(P_PID, pid, WEXITED | WNOWAIT)` (safe), then sets `done` under a
  mutex, then reaps with `child.wait()`.
- the signal thread sleeps 250 ms, locks the mutex, and kills only if `!done`. While the child is an unreaped zombie its
  pid cannot be reused.

Windows: on a console control event, `TerminateProcess` after 250 ms, then exit with `STATUS_CONTROL_C_EXIT` (0xC000013A), which is what CPython ≥ 3.8 uses for an uncaught `KeyboardInterrupt`; unverified on a Windows oracle. Printing
`KeyboardInterrupt` is optional, since nobody sees it.

#### H.8.8 Public `snp hazmat` (path C)

| Aspect | Behavior |
|---|---|
| Parser | Same grammar as path H, but with prog `pre-commit hazmat`/`pre-commit hazmat <tool>` (or the snp product name; follow §21 Q6). Top-level `--color {auto,always,never}` is accepted before the tool. The default is `PRE_COMMIT_COLOR`, validated only when the flag is absent (`argument --color: invalid use_color value: '<v>'`, exit 2). "unrecognized arguments" comes from the top-level parser with the full top-level usage and `pre-commit: error: unrecognized arguments: …` (ORC G1) |
| `SystemExit` texts | identical, stderr, exit 1 |
| Unexpected errors (the §H.8.6 table) | stdout `An unexpected error has occurred: <Class>: <msg>` + `Check the log at <log>`, crash log, **exit 3**. `<Class>` is the bare type name, e.g. `ExecutableNotFoundError` with no module prefix (`error_handler.py:25`) |
| Child status | as path H (`n1` OR, `-N & 0xFF`, e.g. 247 for SIGKILL, ORC E10) |
| SIGINT | shared error handler: `Interrupted (^C): KeyboardInterrupt: `, exit 130 (child kill as §H.8.7) |
| Store | **Deviation**: snp creates the store only if it writes a crash log. pre-commit creates `README`/`db.db`/`.lock` for every parsed invocation (Tier C side effect; consistent with §21 Q12 lazy store) |
| git / config | none needed; works outside repos |

#### H.8.9 Test vectors to add (HAZ-10 … HAZ-60; complements doc 10 §5.18)

- Port every row of §H.5 and §H.6 as an integration test.
- Use `require_serial: true` or ≤4 matching files, so there is one partition and no shuffle-dependent order.
- Leave `COLUMNS` unset.
- Compare the full output for `SystemExit`, argparse and help cases. For traceback cases, compare snp's whole output to
  the **last line** of the oracle's output.
- Exit codes are Tier A. That includes 247/241/255 and B2's 3 vs B2b's 2.
- Add:
  - A12 (relative PATH entry under `cd`) as a resolver test.
  - B13/C8 (ENOEXEC: no sh fallback).
  - D10 (stdin type through hazmat in both color modes; `pty_` and `posix_` gated).
  - D5/D12/D13 (rewrite matching).
  - D8 (`script` language).
  - F1/F2/B14 (`args:` placement).
  - A sig test: the child ignores INT and is killed within ~0.3 s (`posix_` gated).
- Windows: a code ≥ 2^31 passes through `n1` and `cd` unchanged.

### H.9 Errata (replacement text; the original sections were left untouched)

1. **03 §15 bullet "`normalize_cmd` errors (…) … surface as exit 3."** Replace with:
   - "On the documented in-hook path (entry rewritten to `python -m pre_commit.commands.hazmat`), `normalize_cmd`
     errors, an empty command, a missing `cd` directory and exec failures are **uncaught**. They print a Python
     traceback into the hook output, and the hook reports `- exit code: 1`. Exit 3 with `An unexpected error has
     occurred: …` and a crash log happens only when `pre-commit hazmat` runs through the CLI (§H.1, §H.6)."
2. **03 §15 intro "They are in `COMMANDS_NO_GIT` (no chdir; cwd is whatever the hook runner set …)."**
   - `COMMANDS_NO_GIT`, `--color` and `Store` concern only the CLI path. The in-hook path never imports `main.py`.
   - The cwd is the hook's cwd, i.e. the toplevel.
3. **03 §15 `_cmd_filenames` paragraph: "goes to stderr with exit 1"**. Add:
   - "…to the hazmat process's stderr, which in a hook is the merged capture. It appears in the hook output, **once per
     xargs partition**."
4. **03 §15 table, `ignore-exit-code` row "always 0"**. Replace with:
   - "0 whenever the command was spawned, whatever its status or signal. 1 on resolution or spawn errors. 2 on usage
     errors. 0 for `-h`."
5. **03 §15 table, `n1` row**. Add:
   - "a signal death contributes `-N`, and the process status is `ret & 0xFF` (SIGKILL alone → 247)."
6. **03 §15 bullet "Docs: these do not work with … `unsupported_script`"**. Add:
   - "In practice `unsupported_script`/`script` works, because the rewritten `cmd[0]` is absolute and `prefix.path` keeps
     it (ORC D8). `julia`, `docker` and `docker_image` produce broken commands."
7. **03 §15**: add the argparse facts of §H.3:
   - `--` eaten around SUBDIR
   - a leading `--` is kept for `n1`/`ignore-exit-code`
   - a first token starting with `-` → `unrecognized arguments`, exit 2
   - prog `hazmat.py`, exit 2 for usage errors
   - `-h` → exit 0
   - hook `args:` land after the trailing `--`
8. **03 §20.8**: superseded by §H.8. Key changes:
   - rewrite target `[self_exe, "__hazmat", …]` (hidden entry point, in-hook semantics)
   - also rewrite `snp hazmat`
   - `snp hazmat` reproduces CLI semantics
   - subprocess model
   - final-line error rendering with exit 1
9. **03 §21 Q13**: resolved **for hazmat** by §H.8.1. Other `pre-commit <subcommand>` entries, absolute paths and nested
   uses are still PATH lookups. Whether snp ships a `pre-commit` shim stays open (§H.11 Q4).
10. **10 §6 item 1**. Replace the stderr clause with:
    - "stderr is used only by argparse (usage + `error:`; exit 2) and by `SystemExit("msg")` paths **of the process that
      raised them**: hook-impl argument errors (exit 1) and a **direct** `pre-commit hazmat …` call (exit 1).
    - When hazmat runs as a hook entry (rewritten to `python -m pre_commit.commands.hazmat`), its stderr is the hook's
      merged capture pipe or pty. Its SystemExit texts, argparse usage/errors (prog `hazmat.py`) and Python tracebacks
      therefore appear inside the hook's output block on pre-commit's **stdout** (under git hooks: on the terminal via
      git's redirection).
    - Its status becomes the hook's `- exit code:`: 1 for SystemExit or an uncaught exception, 2 for usage errors, 0 for
      `-h`, and otherwise the child's code (`n1`: OR; signals `-N & 0xFF`).
    - hazmat exit 3 exists only on the CLI path."
11. **10 §5.18 HAZ**: add the vectors of §H.8.9. **10 Q4**: answered by §H.8.1 for hazmat.

### H.10 Related contradictions resolved here (with evidence)

1. **Hook stdin** (02 §21.3 `stdin(Stdio::null())` and 06 §8 `stdin Stdio::null()` vs 04a §14.3 `Stdio::piped()` + drop
   `ChildStdin`).
   - **04a and 02's TL;DR #9 are correct. 02 §21.3 and 06 §8 are wrong.**
   - Evidence: `util.py:81-83, 103-107` (stdin=PIPE, closed by `communicate()`); `util.py:176-178` (`/dev/null` only in
     the pty path); `util.py:201-202` (Windows always uses pipes).
   - ORC D10/D11: `pipe:[N]` without color and `/dev/null` with color, 0 bytes readable, both directly and through hazmat.
   - Rule for snp:
     - without color, or on Windows: `Stdio::piped()` and drop `ChildStdin` right after `spawn()`, before reading
       output, to avoid deadlock with stdin-reading tools.
     - color on POSIX: `Stdio::null()`.
   - prek's #1154 (a hang on a TTY stdin) is avoided by both.
2. **hazmat failure exit code** (03 §15 "exit 3"): see §H.0 #2, §H.5 and §H.6. Exit 1 in-hook (traceback), exit 3 only on
   the CLI. Exit 2 for argparse in both.
3. **Argparse error prefix** (02 §2.2 `pre-commit run: error: <msg>` vs 10 CLI-07).
   - ORC G2: `pre-commit run --bogus` → the top-level usage (3 lines) + `pre-commit: error: unrecognized arguments:
     --bogus`, exit 2. ORC G1 shows the same for `pre-commit hazmat n1 -x --`.
   - Rule: **"unrecognized arguments" is always emitted by the top-level parser** (its prog and usage). Every other
     argparse error is emitted by the innermost parser that detected it:
     - `pre-commit hazmat cd: error: the following arguments are required: …`
     - `pre-commit hazmat: error: argument tool: invalid choice: …`
     - `pre-commit hazmat: error: argument --color: invalid use_color value: …`
   - On path H the same rule holds with prog `hazmat.py`. **10 CLI-07 is correct.** 02 §2.2 is correct only for errors
     the `run` subparser itself detects.
4. **`PRE_COMMIT_COLOR`** (02 §2.2 / 03 §1.1 "invalid env value → exit 2").
   - ORC G3/G8: with no `--color`, `PRE_COMMIT_COLOR=bogus` or empty → exit 2, `pre-commit hazmat: error: argument --color:
     invalid use_color value: 'bogus'` (`''`).
   - ORC G4: `PRE_COMMIT_COLOR=bogus pre-commit hazmat --color never …` → exit 0.
   - argparse converts a string default only when the option was not given. Precise rule: "an invalid or empty
     `PRE_COMMIT_COLOR` is an error only when `--color` is absent from that command line".
   - Path H never reads it.
5. **Executable lookup** (04a §14.18 "do not use `which`" vs 06 §8 "PATHEXT via `which`").
   - **04a is correct.**
   - ORC A12: pre-commit returns the **relative** `bin/reltool` for the relative PATH entry `bin`. Under `hazmat cd`
     that path is re-resolved in `sub/`, so `SUB-bin-tool` runs.
   - `which` 8.0.6 (`src/finder.rs:171-185`):
     - joins ordinary relative PATH entries onto the cwd (absolute result), so it would pick `<root>/bin/reltool` and
       run `TOP-bin-tool`.
     - also tilde-expands entries (`tilde_expansion`), which Python does not.
     - drops empty entries on Windows (`:175`).
   - These are observable divergences. snp owns a ~40-line resolver (04a §1.8).
6. **In-process emulation granularity** (04a Q8 vs 09 §0/§2.2): not decided here for pygrep. hazmat shows that per-partition
   effects are user-visible:
   - messages repeated per partition (A3, A6, B3)
   - the first offending file is reported per partition
   - `n1` ORs only within a partition
   This supports 09's "once per partition".

### H.11 Open questions (new)

1. **Name of the hidden entry point.** Is `__hazmat` acceptable, or should it be `hazmat-impl` (in the style of
   `hook-impl`)? Recommended: `__hazmat`, never listed and never documented as stable.
2. **Python-version texts.** snp pins CPython 3.12 argparse texts (prog `hazmat.py`, `required: subdir, cmd`, quoted
   choices) for path H. Should a config switch emulate 3.13 or 3.14 for users comparing logs? Recommended: no (Tier B).
3. **SIGINT kill-after-250 ms** (§H.8.7): implement now, or accept "child keeps running if it ignores SIGINT" as a
   documented deviation for v1? Recommended: implement. It prevents a stray tool from modifying files after ^C.
4. **`pre-commit` shim.** When only snp is installed, should `snp install` (or packaging) provide a `pre-commit`
   multicall alias? It would make the non-rewritten forms of §H.2.3 work: absolute paths excluded, but `entry:
   pre-commit` + `args`, nested `pre-commit hazmat` and `sh -c 'pre-commit …'` included.
5. **Nested rewrite.** Should snp's `__hazmat` also rewrite a nested `pre-commit hazmat`/`snp hazmat` at the start of its
   own `cmd` (F3)? Parity says no. Recommended: no.
6. **Windows OSError texts.** CPython reports CreateProcess failures as `[WinError N] …`. It is not verified whether a
   `: 'filename'` suffix follows, or how a bad `cwd` is reported (likely `[WinError 267] The directory name is
   invalid`). This needs a Windows oracle run. Until then, Tier B.
7. **Rust batch-file argument escaping** (CVE-2024-24576 hardening). `n1 tool.cmd --` with filenames containing
   characters Rust refuses for `.bat`/`.cmd` gives `InvalidInput` where Python would run (possibly unsafely). Should snp
   keep Rust's refusal and render it as `OSError: <msg>`, exit 1? Recommended: yes, as a documented safety deviation.
