# 05 — lefthook (Go): features, config schema, runtime model, and what snp should adopt

Status: research spec. Audience: engineers implementing snp (Rust 1.98.0, edition 2024) who will NOT re-read the Go sources.

- Reference: **lefthook 2.2.0** (`internal/version/version.go:10`), repo HEAD `c702ccf` (2026-10-07, "fix: generate different patch names when in a worktree (#1582)"). `refs/lefthook` now tracks tag **v2.2.2** (`eb4e504b`, 2026-10-13); citations below are of 2.2.0 and may be off by a few lines. The 2.2.0–2.2.2 changes were reviewed and need no snp change: the trailing NUL of a `{staged_files}`-style NUL list is already skipped by `--stdin-files=nul`; the stash/backup cleanup fixes for worktree flows concern lefthook's own `lefthook auto backup` stash, which snp does not use (snp-stash keeps pre-commit's patch-file model); the rest is packaging (composer, android npm package).
- All `path:LINE` citations are relative to `refs/lefthook/` unless prefixed: `pc:` = `refs/pre-commit/` (4.6.2), `prek:` = `refs/prek/`.
- Confidence tags: **[src]** read in Go source; **[test]** asserted by a lefthook integration fixture (`tests/integration/*.txt`, testscript format); **[doc]** docs only (docs drift from code in several places, see §17); **[derived]** inferred from a dependency's documented semantics (koanf v2.3.7, mapstructure, gobwas/glob v0.2.3, doublestar v4.10.2, lipgloss v2, Go `fmt`) — not executed. lefthook itself was **not** built or run (instructions: reference repos are data only). pre-commit 4.1.0 **was** run for the extension-key warning test in §18.1.
- Dependency versions (`go.mod`): koanf/v2 v2.3.7, mapstructure v1.5.0, gobwas/glob v0.2.3, bmatcuk/doublestar/v4 v4.10.2, gabriel-vasile/mimetype v1.4.15, creack/pty v1.1.24, al.essio.dev/pkg/shellescape v1.6.1, urfave/cli v3, lipgloss v2.

---

## 0. Executive summary

1. lefthook is a **single static binary** + a tiny POSIX-sh **hook shim** per git hook (`.git/hooks/<hook>` → `lefthook run "<hook>" "$@"`). The config is re-read on every hook invocation, so you don't need to reinstall after editing it. A checksum file (`.git/info/lefthook.checksum`) lets `run` silently re-install shims when the config changes ("auto-install").
2. The config is **one YAML/TOML/JSON/JSONC document**: top-level options plus **one map per git hook name** (or custom name). Each hook holds `jobs` (an ordered list, newest model), `commands` (a map, legacy), and `scripts` (a map of files under `.lefthook/<hook>/`). The format is dynamically typed: koanf + mapstructure with `WeaklyTypedInput`, so a scalar becomes a 1-element list and a number becomes a string, and **unknown keys are silently ignored at runtime**.
3. **Layered config**: main → `extends` (local files/globs, recursive) → `remotes` (git repos cloned into `.git/info/lefthook-remotes/`) → `lefthook-local.*` → local `extends`. Named jobs merge by `name`, unnamed jobs append, and `{cmd}` in an override's `run` expands to the overridden command.
4. **Files are passed by textual template substitution into a `sh -c` string** (`{staged_files}`, `{push_files}`, `{all_files}`, `{files}`, plus `{0}`, `{1}`… git args, custom `templates`, `{lefthook_job_name}`). Files are shell-escaped. Quoted templates (`"{staged_files}"`) re-quote each file with `"…"`, which **re-enables `$()` command substitution in file names** (§17 #1). Git args are **not escaped**. snp must never copy this design. It must pass argv.
5. **Filtering** happens in this order: `glob` → `exclude` → `root` → `file_types`. The default glob engine is gobwas with **no separator**, so `*` matches `/` and `*.rb` matches `a/b/c.rb`. glob matching is **case-insensitive**, but `exclude` matching is **case-sensitive**. A list of globs is OR'ed and can produce **duplicates**. A bad pattern **panics** (`MustCompile`).
6. **Execution**: jobs run sequentially by default. With `parallel: true`, every job gets its own goroutine with **no concurrency bound**, and the summary is in **completion order**. `piped: true` means "stop after the first failure". `group` nests jobs with their own parallel/piped flags. `priority` exists only for the legacy `commands`/`scripts` (0 means last). On unix, when stdout is a TTY, each job runs under a **PTY** with merged stdout/stderr captured into a buffer, and the buffer is printed after the job finishes (unless `follow`/`interactive`).
7. **Partially staged files** (pre-commit hook only): lefthook hides unstaged hunks **only for files that are both staged and unstaged**, never for the whole tree. It saves a patch to `.git/info/lefthook-unstaged.patch`, makes a full backup with `git stash create` + `git stash store -m "lefthook auto backup"`, then runs `git checkout --force -- <files>`. After the jobs, it checks the patch with `git apply --check` **before** `git add`-ing the `stage_fixed` files, applies the patch, and drops the stash. On conflict it runs `git checkout .`, re-applies a whole-tree patch, and fails with `conflict while merging unstaged changes`.
8. `stage_fixed: true` runs `git add --force -- <files>` after the job, but only in the `pre-commit` hook. The files added are the job's template files, or the filtered staged files. lefthook's default `fail_on_changes: never` means a job that **modifies files does not fail** the hook. This is the opposite of pre-commit, which always fails on modifications.
9. Other notable features include:
   - skip/only conditions: `merge`, `rebase`, `merge-commit`, `{ref: glob}`, `{run: sh}`
   - tags with `exclude_tags`, `LEFTHOOK_EXCLUDE`, and `--tag`/`--job`
   - per-job `env` (values are `$VAR`-expanded by lefthook), `root` (monorepo subdir), `timeout`, `fail_text`, `interactive` (/dev/tty), `use_stdin`, `setup`, `templates`, `min_version`, `rc` (sourced by the shim)
   - `assert_lefthook_installed`, built-in git-lfs passthrough, and `ai:` (generates Claude/Codex/Cursor/Copilot hook configs)
10. **Recommendation:** adopt the ideas as **optional hook-level extension keys in `.pre-commit-config.yaml`**. pre-commit 4.x only *warns* about them (verified below). Where prek already defined semantics (`priority`, `env`, `groups`, `x-*` ignored), use prek's semantics. MUST items: bounded parallel scheduling via `priority`, `env`, a local override file, `stage_fixed` (fix → stage → verify), a hardened partial-staging guard, a `SNP=0` kill switch, and robust shim binary discovery. §18 has the ranked list and §19 the `snp migrate lefthook` converter.

---

## 1. Architecture and the `lefthook run` data flow

| Package | Role |
|---|---|
| `cmd/` | urfave/cli v3 command + flag definitions (`cmd/run.go`, `cmd/install.go`, …) |
| `internal/command/` | command implementations (run, install/sync, uninstall, add, dump, validate, check-install, AI hook files) |
| `internal/config/` | structs (`config.go`, `hook.go`, `job.go`, `command.go`, `script.go`), loader/merger (`loader.go`), skip checker, embedded JSON schema |
| `internal/run/controller/` | hook runner: scope inheritance, jobs/groups, guard (hide unstaged / stage fixed / fail-on-changes), LFS, setup |
| `internal/run/controller/command{,/replacer}` | builds shell command strings, template substitution, command-line splitting |
| `internal/run/controller/filter/` | glob / exclude / root / file_types filtering |
| `internal/run/controller/exec/` | process spawning (PTY on unix, `sh.exe` on Windows) |
| `internal/git/{,wrapper/}` | git CLI wrapper, per-run caches, repo state, remotes |
| `internal/logger/` | output sections, colors, spinner |
| `internal/templates/` | hook shim (`hook.tmpl`), default config (`config.tmpl`), checksum line |
| `internal/updater/` | `self-update` (GitHub releases + sha256) |

`lefthook run <hook> [git-args…]` step by step:

1. `main.go:12-17`: runs the CLI. On any error it prints `Error: <err>` to **stderr** (it prints nothing if the message is empty) and calls `os.Exit(1)`.
2. `cmd/run.go`: `NewLefthook(verbose, colors)` (`internal/command/lefthook.go:41-65`):
   - Sets the logger to **stdout**.
   - Applies `--colors`.
   - Turns on debug if `--verbose` is given or `LEFTHOOK_VERBOSE` is enabled.
   - Calls `git.BuildRepo` (`internal/git/repo.go:119-148`), which:
     - runs `git version`. If git is older than 2.31.0, it warns `Git version is too old. Minimum supported version is 2.31.0`.
     - runs `git rev-parse --path-format=absolute --show-toplevel --git-path hooks --git-path info --git-dir --git-common-dir` (`internal/git/wrapper/paths.go:7-14`). This is the "worktree" case when git-dir ≠ common-dir.
     - creates `<info>` with mode `0775` if it is missing.
   - Not being in a git repo is an error, so the exit code is 1.
3. `cmd/run.go:120`: no positional argument gives `hook name missing`. The first argument is the hook name and the remaining ones are `GitArgs`.
4. `internal/command/run.go:50`: `LEFTHOOK` = `0` or `false` → return success immediately.
5. `run.go:54`: `Cache.WarmUp()` starts 3 goroutines (`staged files`, `staged files incl. deleted`, `git status --porcelain -z`) **while** the config loads (`internal/git/cache.go:26-46`).
6. Load + merge the config (§2). If no config is found, it prints a **warning** and exits **0** (`run.go:65-69`).
7. `min_version` check (`run.go:72`).
8. Ghost hook: when the hook is `prepare-commit-msg` and it is not in the config and `--verbose` is not set, the log level is lowered to warn (`run.go:80-83`).
9. Output settings: `LEFTHOOK_OUTPUT` (string) overrides `output` (`run.go:85-92`). Then the meta box is printed.
10. Auto-install/sync: unless `--no-auto-install` or `no_auto_install` is set, call `syncHooks` (§14.5). Errors here are warnings only.
11. `resolveHook`:
    - A known git hook that is not in the config → exit 0 silently.
    - An unknown name → `hook <name> doesn't exist in the config`.
    - `parallel` and `piped` both set → error (`run.go:162-177`).
12. Forced files: `--files-from-stdin` / `--all-files` / `--file` (`run.go:179-195`), plus the script source dirs (`run.go:197-221`) and `fail_on_changes` resolution.
13. Convert `commands` and `scripts` into jobs, appended **after** `jobs` (`run.go:136-140`). `--command` values are folded into `--job`.
14. Wait for the cache warm-up, then `runHook`:
    - `signal.NotifyContext(SIGINT)`.
    - `controller.RunHook`: hook skip/only → LFS → setup → spinner → guard{ sequential | concurrent } → "no job matched" check.
    - Summary.
    - Exit 1 if any result failed.

---

## 2. Configuration files

### 2.1 File names and lookup order [src]

| Kind | Base names (`internal/config/loader.go:36-37`) | Extensions (`loader.go:38-44`) |
|---|---|---|
| main | `lefthook`, `.lefthook`, `.config/lefthook` | `.yml`, `.yaml`, `.json`, `.jsonc`, `.toml` |
| local | `lefthook-local`, `.lefthook-local`, `.config/lefthook-local` | same |

- **Loader order is extension-major** (`loader.go:89-102`): `lefthook.yml`, `.lefthook.yml`, `.config/lefthook.yml`, `lefthook.yaml`, `.lefthook.yaml`, `.config/lefthook.yaml`, `lefthook.json`, …, `.config/lefthook.toml`. **The first file that exists wins.** All others are silently ignored. The docs say "you'll never know which one", but in fact the order is deterministic.
- **`install`/`check-install` use name-major order** (`internal/command/install.go:134-141`), e.g. `lefthook.yml, lefthook.yaml, lefthook.json, lefthook.jsonc, lefthook.toml, .lefthook.yml…`. That order picks the file used for the checksum **mtime** (§14.5). If several main files exist, the loader and the installer can disagree (§17).
- All paths are relative to the **repo root** (`--show-toplevel`), never to the cwd.
- The docs say a dot-named main config needs a dot-named local config. **The code does not enforce this.** Every local name is tried regardless.
- **Local-only mode**: if no main config exists but a local one does, loading proceeds with an empty main (`loader.go:105-126`).
- Neither exists → `ConfigNotFoundError` with message `No config files with names ["lefthook" ".lefthook" ".config/lefthook"] have been found in "<root>"`. This is Go `%q` of a `[]string` (`loader.go:101`). `run` prints it as a **warning and exits 0** (`run.go:65-69`).
- `LEFTHOOK_CONFIG=<path>` replaces **only** the main config lookup (`loader.go:128-152`):
  - A relative path is joined to the repo root.
  - If the file is missing: `Config file "<abs path>" not found!`, again only a warning and exit 0 under `run`.
  - extends, remotes, and the local config still load.

### 2.2 Parsing and typing [src + derived]

- Parsers: YAML via go.yaml.in/yaml v3, which uses **YAML 1.2 booleans**: only `true`/`false`, while `on`/`yes` stay strings. JSON is standard. JSONC is passed through `tidwall/jsonc.ToJSON`, which strips comments and trailing commas (`internal/config/jsonc_parser.go`). TOML uses go-toml v2.
- koanf is built with delimiter `"."` (`koanf.New(".")`). Unmarshal goes through mapstructure with `WeaklyTypedInput: true`, `TagName: "koanf"`, and the decode hooks `StringToTimeDuration` + `TextUnmarshaler` (koanf v2.3.7 `koanf.go:265-273`). Consequences:
  - A scalar becomes a 1-element list. `glob: "*.js"` ≡ `glob: ["*.js"]`. `tags: "frontend style"` ⇒ **one** tag `"frontend style"`. The docs claim commas separate tags, but koanf v2 has no StringToSlice hook, so `tags: "a,b"` is one tag `"a,b"` [derived].
  - A number becomes a string: `glob: 3` → `"3"` [test: `tests/integration/job_merging.txt`, dump shows `- "3"`].
  - `"1"`/`"true"` become bool. `"15"` becomes an int.
  - `timeout` uses Go `time.ParseDuration` syntax: `300ms`, `1.5s`, `2h45m`, units `ns us µs μs ms s m h`. A bare integer `timeout: 15` decodes weakly to **15 ns** [derived].
  - **Unknown keys are ignored silently at runtime.** There is no `ErrorUnused`, so typos like `stage_fix:` are no-ops. Only `lefthook validate` (JSON schema) catches them.
  - Field matching is case-insensitive (mapstructure fallback when there is no tag) [derived].
- JSON schema: `schema.json` is byte-identical to `internal/config/jsonschema.json`. It is draft 2020-12 with `$id: https://json.schemastore.org/lefthook.json` and is published at `https://lefthook.dev/schema.json`.
  - Every object has `additionalProperties: false`.
  - Root `additionalProperties` is the Hook schema, so custom hook names are allowed.
  - `Job` is `oneOf` required `run` | `script` | `group`.
  - `$schema` is allowed at the root.

### 2.3 `extends` [src]

- `extends: [path-or-glob, …]`.
- **Relative paths resolve against the repo root**, and this applies to nested extends too, because `extendRecursive` passes the same root (`loader.go:339-376`). For extends declared inside a **remote** config, paths resolve against the *directory of that remote config file* (`loader.go:318`). The docs say "remote repository root", which is drift.
- Globs use `afero.Glob`, i.e. Go `filepath.Match` semantics: `*`, `?`, `[…]`. `*` does **not** cross `/`, and there is no `**`. A path or glob that matches nothing is **silently ignored**.
- Each extended file is loaded together with its own `extends`, recursively, and **the nested file overrides the file that includes it**. Example: main `run: echo 0` ← e1 `run: echo 1` ← e2 `run: echo 2` gives `echo 2` [test: `tests/integration/many_extends_levels.txt`].
- Duplicate or cycle detection: a `visited` set per top-level `extend()` call produces the error `possible recursion in extends: path <p> is specified multiple times` (`loader.go:352`). Legitimate diamond includes also trip it.
- An unsupported extension is an error:
  ```
  can't parse config '<p>', file has unsupported or no extension
  hint: rename <p> to <p>.yml
  ```
  (`loader.go:359`).
- A bad glob gives `bad glob syntax for '<p>': <err>` (`loader.go:347`).

### 2.4 `remotes` [src]

`remotes: [{git_url, ref?, configs?=[lefthook.yml], refetch?=false, refetch_frequency?}]`

**Where remotes live.** Remotes are stored under `<git-info-dir>/lefthook-remotes/<dir>` (`internal/git/remote.go:10, 58-75`). `<dir>` is built like this:
- Start with `basename(url minus extension)`.
- If `ref` is set, append `"-" + ref`, after first escaping `-`→`--` and then `/`→`-`.
- Example: `feat/a-b` → `feat-a--b`.
- Two remotes whose URLs share a basename (e.g. `github.com/a/configs` and `github.com/b/configs`) **collide** (§17).

**Git commands.** All of these remove `GIT_DIR` and `GIT_INDEX_FILE` from the env, add `LEFTHOOK=0`, and only debug-log their errors:
- clone: `git -C <remotes> clone --quiet --origin origin --depth 1 [--branch <ref>] <url> <dir>`, followed by a fetch when `ref` is set (`internal/git/wrapper/clone.go:14-18`, `remote.go:38-56`).
- update with a ref: `git -C <dir> fetch --quiet --depth 1 origin -- <ref>`, then `git -C <dir> checkout FETCH_HEAD` (`wrapper/fetch.go`).
- update without a ref: `git -C <dir> pull --quiet` (`wrapper/pull.go`).

**When remotes are fetched:**
- `lefthook install` always syncs every remote. `--force` first runs `rm -rf` on the remote dir. A failure gives the warning `Couldn't sync from <url>. Will continue anyway: <err>` (`install.go:50-60`).
- `lefthook run` (auto-sync) decides per remote with `shouldRefetch` (`install.go:234-266`):
  1. `refetch: true` or `refetch_frequency: always` → fetch.
  2. `refetch_frequency: never` → no.
  3. `<dir>/.git/FETCH_HEAD` missing → fetch. A fresh `--depth 1` clone without a ref has no FETCH_HEAD, so the next run pulls once.
  4. `refetch_frequency` empty → no.
  5. Otherwise parse a Go duration and fetch if `now > mtime(FETCH_HEAD) + d`. A parse error gives the warning `Couldn't parse refetch frequency <v>. Will continue anyway: <err>` and no fetch.
- On a failed sync, `findAvailableRemoteRef` looks for an older checkout of the same URL (`install.go:268-287`). It then warns either `Couldn't sync <url> <ref>. Will continue with fallback version: <ref>.` or `… Will continue with old version.`, or `Couldn't sync from <url>. Will continue without that remote.`
- Remote dirs no longer referenced are `RemoveAll`'d during run-sync (`install.go:202-218`).

**How remote configs load.** `configs` are loaded in order (`loader.go:287-330`):
- A missing file is silently skipped.
- Each config's `extends` are merged right after it.
- After each remote, `extends` is reset to nil.
- Only the **main** config's `remotes` key is honored (`loader.go:157-160`).

**Scripts from remotes** are looked up in `<remote dir>/<source_dir>/<hook>/<script>` (`run.go:207-218`).

**Security note:** remote configs execute arbitrary commands with the user's privileges on every commit. With `refetch`, unreviewed upstream changes take effect silently. The `lefthook` key is stripped from extends and remotes for this reason (`loader.go:181`).

### 2.5 Merge algorithm and precedence [src + test]

Precedence (later wins), built as `main` + `secondary` (`loader.go:154-201`):

1. main config
2. main `extends` (in list order; nested extends override their parent)
3. `remotes`, in list order. Each remote's `configs` are processed in order, and each config is followed by its own extends.
4. **the `lefthook` key is deleted from secondary here**, so only main or local can set it (`loader.go:181`)
5. local config (`lefthook-local.*`)
6. local `extends`, only when the local list differs from main's list (`loader.go:192-198`)

Merge rules:

| What | Rule | Cite |
|---|---|---|
| maps (generic) | deep merge (koanf `maps.Merge`); scalars and **lists are replaced** by the later source | `loader.go:453`, `:568` |
| non-hook top-level keys | `main.Merge(secondary)`, so secondary wins | `loader.go:409` |
| hook `jobs` (list) | `mergeJobsSlice(later, earlier)`: keep the earlier order. A later job with a `name` equal to an earlier named job is deep-merged into it (later wins per key), and **`{cmd}` in the later `run` is replaced by the earlier `run`**. If both have `group.jobs`, those are merged recursively with the same rule. Unnamed or new-named jobs are **appended**. Non-map entries are dropped. | `loader.go:612-701`; test `job_merging.txt` |
| hook `commands` (map) | merged per command name. In the final main←secondary merge, `{cmd}` in the override's `run` is replaced with main's `run` | `loader.go:425-472` |
| hook `scripts` (map) | merged per script file name. The docs show `{cmd}` in `runner`, but **the code only replaces in `run`** | `loader.go:455-472` |
| hook `setup` (list) | successive loads (extends/remotes/local into secondary) **append** in load order. The final main←secondary step **prepends** secondary's setup before main's. | `loader.go:558-566`, `:451` |
| `exclude_tags` | after merge, `LEFTHOOK_EXCLUDE` (comma-split, **not trimmed**) is appended for every hook | `loader.go:494-496` |

Custom (non-git) hooks are detected when the top-level key is one of the 28 `AvailableHooks` (`internal/config/available_hooks.go:11-40`), **or** when a flattened key matches `^([^.]+)\.(?:scripts|commands|jobs)` (`loader.go:35`, `:393-406`). A custom hook that only has `setup` is therefore not recognized.

`AvailableHooks` (git-scm list): `applypatch-msg pre-applypatch post-applypatch pre-commit pre-merge-commit prepare-commit-msg commit-msg post-commit pre-rebase post-checkout post-merge pre-push pre-receive update proc-receive post-receive post-update reference-transaction push-to-checkout pre-auto-gc post-rewrite sendemail-validate fsmonitor-watchman p4-changelist p4-prepare-changelist p4-post-changelist p4-pre-submit post-index-change`.

`HookUsesStagedFiles` is true only for `pre-commit`. `HookUsesPushFiles` is true only for `pre-push` (`available_hooks.go:42-48`). Notably, `pre-merge-commit` gets **no** staged-file handling.

### 2.6 `lefthook dump` output format [src + test]

- The default is YAML (2-space indent). `--format`/`-f` accepts `yaml|json|toml`. Any other value gives `invalid 'format' value, supported: 'toml', 'yaml', 'json'`.
- Top-level keys are emitted in **alphabetical order**, with hooks intermixed with the options (YAML map encoding) [test `dump.txt`, `job_merging.txt`].
- `source_dir`/`source_dir_local` are omitted when they equal the defaults (`internal/config/config.go:90-95`).
- Hook fields keep struct order (`parallel, piped, follow, fail_on_changes, …, setup, jobs, commands, scripts`), and empty fields are omitted.
- JSON output is pretty-printed with a 2-space indent plus a trailing newline.
- The checksum is the **md5 of the compact-JSON dump** (`config.go:66-82`).

---

## 3. Global (top-level) options — complete [src]

`internal/config/config.go:28-64`; defaults applied in `loader.go:232-233`.

| Key | Type | Default | Semantics | Merge / notes |
|---|---|---|---|---|
| `min_version` | string (semver, optional `v`) | — | `run` fails if binary < value. Messages: `required lefthook version (<min>) is higher than current (<ver>) at <exe path>`; invalid → `format of 'min_version' setting is incorrect` (`run.go:355-373`) | checked only by `run` |
| `lefthook` | string | — | path or sh command used by the hook shim (trimmed, `\n`→`;`, `\`→`/`, `templates/templates.go:54`); also the command written into AI hook files | **not** taken from extends/remotes; local may set it |
| `source_dir` | string | `.lefthook` | scripts live in `<source_dir>/<hook>/<file>` | dump omits default |
| `source_dir_local` | string | `.lefthook-local` | personal scripts dir | — |
| `rc` | string | — | shim does `[ -f <rc> ] && . <rc>` **raw-interpolated** (`templates/hook.tmpl:11-14`) — user must quote paths with spaces | baked into shim at install/sync |
| `output` | `false` \| list of names | all sections | §12.1 | env `LEFTHOOK_OUTPUT` overrides |
| `colors` | `true`/`false` \| `"on"`/`"off"`/`"auto"` \| map `{cyan,gray,green,red,yellow: <ANSI-256 int or "#hex">}` | `auto` | §12.3 | CLI `--colors` (run only) wins (forced flag) |
| `extends` | list of path/glob | — | §2.3 | — |
| `no_tty` | bool | false | no spinner, `interactive` ignored | OR'ed with `--no-tty` |
| `assert_lefthook_installed` | bool | false | shim exits 1 if no lefthook binary found (§14.2) | baked into shim |
| `skip_lfs` | bool | false | don't run built-in `git lfs <hook>` (§11) | OR'ed with `--skip-lfs` |
| `no_auto_install` | bool | false | don't sync shims during `run` | OR'ed with `--no-auto-install` |
| `install_non_git_hooks` | bool | false | also write shims for custom hook names into hooks dir (2.1.0+) | — |
| `glob_matcher` | `gobwas` \| `doublestar` | `gobwas` | engine for `glob` and `exclude` (§7.2) | global only |
| `remotes` | list of Remote | — | §2.4 | only main's honored |
| `templates` | map string→string | — | custom `{name}` replacements in `run`/`args`/`setup`, **not** shell-escaped | map-merged (local overrides keys) |
| `ai` (beta) | `{claude,codex,cursor,copilot: {event: hook-name}}` | — | §14.6 | — |
| `$schema` | string | — | ignored | schema-only |
| `<hook-name>` | Hook | — | §4 | — |

Removed in 2.0.0 (CHANGELOG): `skip_output` (use `output`), env `LEFTHOOK_QUIET`, regexp support in `exclude`, old CLI flag names (`--commands` etc.).

---

## 4. Hook options (`<hook-name>:`) [src]

`internal/config/hook.go:5-23`.

| Key | Type | Default | Semantics |
|---|---|---|---|
| `parallel` | bool | false | run top-level jobs concurrently (one goroutine each, **unbounded**) |
| `piped` | bool | false | sequential; after first failure remaining jobs are skipped with reason `broken pipe`. `parallel`+`piped` both true at hook level → error `conflicting options 'piped' and 'parallel' are set to 'true', remove one of this option from hook group` (`run.go:27`, `:172-174`). Inside a `group`, both true → `parallel` silently wins (`job.go:69-73`) |
| `follow` | bool | false | stream job stdout live (no buffering, spinner disabled); with `parallel` output interleaves |
| `fail_on_changes` | string `never`/`false`/`0`/`""` → off; `always`/`true`/`1` → on; `ci` → on iff env `CI` **exists** (any value, `LookupEnv`); `non-ci` → on iff `CI` absent; other → error `invalid value for fail_on_changes: <v>` (`run.go:223-242`) | `never` | §10.4. CLI `--fail-on-changes[=false]` overrides |
| `fail_on_changes_diff` | bool (tri-state) | unset ⇒ true iff `CI` exists (`run.go:244-254`) | print `git diff` of changed files when fail_on_changes triggers. (CLI flag exists in code but is **not registered**, §17) |
| `files` | string (sh command) | — | default `{files}` source for all jobs (job-level overrides) |
| `exclude_tags` | list of strings | — | skip jobs whose tags (incl. inherited group tags) **or name** match; `LEFTHOOK_EXCLUDE` appended |
| `exclude` | list of globs | — | prepended to every job's exclude list (plus CLI `--exclude`) |
| `skip` / `only` | condition (§8.1) | — | whole hook skipped → log `<hook> (skip) hook setting`, **no results**, exit 0 |
| `setup` | list of `{run: string}` | — | run before jobs (§6.5) |
| `jobs` | list of Job | — | §5 |
| `commands` | map name→Command | — | legacy; converted to jobs after `jobs` |
| `scripts` | map filename→Script | — | legacy; converted to jobs after commands |

---

## 5. Job options [src]

Applicability: **J** = `jobs:` entry, **C** = `commands:` entry, **S** = `scripts:` entry, **G** = a job that wraps a `group:` (only these keys are inherited by the group's children; see §6.3).

| Key | J | C | S | G | Type | Semantics |
|---|---|---|---|---|---|---|
| `name` | ✓ | (map key) | (map key = file name) | ✓ | string | display name; merge key for local/extends; `--job` selector; `{lefthook_job_name}`. Printable name = `name` → `run` → `script` → `[<index>]` (`internal/config/job.go:40-52`) |
| `run` | ✓ | ✓ (required in schema) | — | — | string | shell command (`sh -c`), templates §7.3 |
| `script` | ✓ | — | (key) | — | string | file name looked up in `<source_dir…>/<hook>/` |
| `runner` | ✓ | — | ✓ | — | string | prefix: `<runner> '<script path>'` (e.g. `bash`, `node`, `go run`) |
| `args` | ✓ | — | ✓ | — | string | appended to command (`run + " " + args`) or script; templates allowed; for scripts: when `args` is set, git args are **not** appended (use `{0}`) |
| `root` | ✓ | ✓ | — | ✓ | string | cwd = `<repo>/<root>`; file filter + path rewrite (§7.2) |
| `files` | ✓ | ✓ | — | ✓ | string | sh command producing `{files}` list |
| `glob` | ✓ | ✓ | — | ✓ | string \| list | include filter (§7.2) |
| `exclude` | ✓ | ✓ | — | ✓ | string \| list | exclude globs (since 2.0 globs only) |
| `file_types` | ✓ | ✓ | — | ✓ | string \| list | §7.2 |
| `tags` | ✓ | ✓ | ✓ | ✓ | list (string ⇒ 1 element) | for `exclude_tags`, `--tag` |
| `env` | ✓ | ✓ | ✓ | ✓ | map string→string | extra env; values `$VAR`-expanded by lefthook (§9) |
| `fail_text` | ✓ | ✓ | ✓ | — | string | replaces the failure text in summary |
| `timeout` | ✓ | ✓ | ✓ | — | Go duration string | kill job after duration; result text `timeout (<d>)` (2.0.16+, undocumented in docs index) |
| `interactive` | ✓ | ✓ | ✓ | — | bool | stdin from `/dev/tty`, stdout to terminal; ignored with `no_tty` (§9) |
| `use_stdin` | ✓ | ✓ | ✓ | — | bool | pass lefthook's (cached) stdin |
| `stage_fixed` | ✓ | ✓ | ✓ | — | bool | `git add` job files after success, `pre-commit` only (§10.3) |
| `skip` / `only` | ✓ | ✓ | ✓ | ✓ | condition | §8.1 |
| `priority` | — | ✓ | ✓ | — | int | ordering for commands/scripts only (§6.1) |
| `group` | ✓ | — | — | ✓ | `{root?, parallel?, piped?, jobs: [Job] (required)}` | nested jobs. Note `group.root` exists in the struct but the runner only uses the *wrapping job's* `root` (`controller/scope.go:58`); `group.root` is effectively ignored |

Validity rules at run time (`internal/run/controller/job.go:20-79`):
- `run` and `script` both set → result **failure** `either \`run\`,\`script\`, or \`group\` must be provided for a job`.
- none of run/script/group → same failure.
- `run`/`script` set **and** `group` set → the group is silently ignored (run/script win).
- group with zero jobs → failure `group must have \`jobs\``.

---

## 6. Job model, ordering, scheduling [src]

### 6.1 Assembling the job list

`hook.Jobs` (definition order) ++ `CommandsToJobs(commands)` ++ `ScriptsToJobs(scripts)` (`run.go:136-140`).

Commands and scripts are each sorted with the same comparator (`internal/config/command.go:59-92`, `script.go:50-82`):
1. if either has non-zero `priority`: priority 0 sorts **last**; otherwise ascending priority;
2. else both names parsed for a leading decimal number (`parseNum`, `script.go:87-106`): both non-numeric → byte-wise name order; numeric before non-numeric; ascending number (`1_create < 2_migrate < 10_seed`).

### 6.2 Sequential / piped / parallel

- `sequentially(jobs, piped)` (`controller/controller.go:159-180`): run in order; if `piped` and a result is a failure, every later job logs `<name> (skip) broken pipe` and **produces no result entry**.
- `concurrently(jobs)` (`controller.go:134-157`): `go` per job, `WaitGroup`, results read from a channel ⇒ **completion order**; no concurrency limit; no cancellation of siblings on failure.
- IDs: `strconv.Itoa(index)` within the current list; unnamed group name = `group (<index>)` (`job.go:54`).
- Log name for nested jobs: ancestors' group names joined with ` ❯ `, e.g. `group (0) ❯ lint` (`job.go:86`).

### 6.3 Scope inheritance into groups (`controller/scope.go:51-81`)

| Field | Rule (child = job inside group, parent = wrapping job) |
|---|---|
| `glob` | concatenated (parent ++ child) — so a child glob list is OR'ed with the parent's |
| `tags` | concatenated |
| `file_types` | concatenated |
| `exclude` | appended (hook-level + CLI first, then each ancestor, then job) |
| `root` | child non-empty wins, else parent |
| `files` | child non-empty wins, else parent, else hook-level |
| `env` | map merge, child wins (copied to avoid races) |
| `skip`/`only` | evaluated at the group level (skips the whole group) and again per child |
| not inherited | `stage_fixed`, `fail_text`, `interactive`, `use_stdin`, `timeout`, `priority` |

`--job` selection: a group whose `name` is in `--job` runs **all** its children (`scope.go:63-66`).

### 6.4 Results (`internal/run/result/result.go`)

`Result{Name, status ∈ {success, failure, skip}, text, Duration, Sub}`. Group status: `skip` if **all** children skipped; `failure` if any child failed; else `success`; duration = **sum** of children (`result.go:50-71`). Skipped jobs do not appear in the summary. Exit code 1 iff any top-level result is a failure (`run.go:278-285`).

`RunHook` extra check (`controller.go:113-129`): if `--job`/`--command` was given and **every** top-level result is a skip → error `no job matching <a, b> found in hook "<hook>"`.

### 6.5 `setup` (2.1.2+, `controller/setup.go:13-46`)

Runs before the guard/jobs: all `setup[].run` strings get templates + git args substituted (file templates too, but with a **nil filter** — a non-empty `{staged_files}` in setup would dereference nil, §17), are executed sequentially through the same executor (merged output streamed to stdout under a yellow `setup ❯` header when output `setup` enabled). A failure only logs warning `Failed to run setup: <err>` — **the hook continues**.

### 6.6 Hook-level order of operations (`controller.go:70-132`)

1. hook `skip`/`only` → if skipped: log `<hook> (skip) hook setting`, return no results;
2. LFS passthrough (§11) — error aborts the hook;
3. `setup`;
4. spinner start (unless `no_tty`/`--no-tty`/`follow`);
5. guard (§10) wrapping sequential/parallel execution;
6. `--job` "nothing matched" check.

---

## 7. Files: sources, filters, templates, splitting, skip rules

### 7.1 File sources (exact git commands) [src]

All list commands run through `wrapper.Cmd.execute` (`internal/git/wrapper/cmd.go:109-146`). **Every command whose argv[0] is `git` takes one process-wide mutex** (prevents `index.lock` races, also serializes read-only calls). Every git call runs with env `LEFTHOOK=0` appended (`internal/system/command.go:41-58`) and stdin = null. Output: whole stdout `TrimSpace`d, split on `\n`; each line `TrimSpace`d, empty lines dropped, then `strconv.Unquote` attempted (undoes `core.quotePath` C-quoting when the line is `"…"`; errors ignored) (`wrapper/utils.go:11-55`). **No `-z` is used for file lists** (except `status`).

| Template / list | Command | Post-processing | Cached |
|---|---|---|---|
| `{staged_files}` | `git diff --name-only --cached --diff-filter=ACMR` (`wrapper/staged_files.go`) | keep only paths that `stat` OK and are **not dirs** (stat follows symlinks ⇒ dangling symlinks and symlinks-to-dir dropped; submodules dropped) (`utils.go:79-94`) | once per run (`git/cache.go`) |
| staged incl. deleted (skip decision only) | `git diff --name-only --cached --diff-filter=ACMRD` | no existence check | once per run |
| `{push_files}` | 1) `git diff --name-only HEAD @{push}`; on error 2) `git diff --name-only <base>...HEAD --` where `<base>` = `origin/<name>` from `.git/refs/remotes/origin/HEAD` (`ref: refs/remotes/origin/<name>`) else first `HEAD -> X` line of `git branch --remotes`; on error/none 3) `git ls-tree -r --name-only HEAD` (`wrapper/push_files.go:22-42`). Errors of 1–2 logged at debug only | existence filter | no |
| `{all_files}` | `git ls-files --cached` | existence filter | no |
| `{files}` | hook/job `files` string executed as `sh -c <cmd>` with cwd `<repo>/<root>` (**Windows: `strings.Split(cmd, " ")` executed directly — no shell, no pipes**, `wrapper/files_by_command.go:8-17`) | existence filter, but relative paths are resolved against the **repo root**, not `root` (`utils.go:79-83`) ⇒ the command must print repo-relative paths (true for `git diff --name-only`, false for `ls`) | no |
| forced (`--file`, `--all-files`, `--files-from-stdin`) | `replacer.NewMocked`: **all four** templates return the forced list (`replacer.go:83-96`) | **no** existence check; still filtered | — |

`{push_files}` ignores the pre-push stdin ref lines entirely (pre-commit derives `from_ref/to_ref` from them, `pc:pre_commit/commands/hook_impl.py:120-170`).

`git status --short --porcelain -z` (`wrapper/status_short.go:8-56`) is parsed as NUL-separated `XY<space>path`; entries with `R`/`C` in X or Y swallow the next NUL item (old name). Used for partially-staged detection and changesets.

### 7.2 Filter pipeline (`internal/run/controller/filter/filter.go:56-75`)

Order: **glob → exclude → root → file_types**. glob and exclude see repo-root-relative paths (root is *not* applied to them, documented: "Globs are always calculated from the actual root").

**glob** (`filter.go:77-133`):
- Empty list or all-empty strings → no filtering.
- Each non-empty pattern selects matching files; results **concatenated** across patterns (OR, **duplicates possible**, order = pattern order then input order).
- Both pattern and path are **lowercased** ⇒ case-insensitive.
- `glob_matcher: gobwas` (default): `glob.MustCompile(pattern)` **with no separator runes** ⇒ `*` = any sequence of any chars **including `/`**; `**` = same; `?` = any single char (incl. `/`); `[abc]`, `[a-z]`, `[!abc]`; `{a,b,c}` alternatives (nestable); `\x` escapes. Whole-string anchored. Invalid pattern ⇒ **panic** (crash).
- `glob_matcher: doublestar`: `doublestar.Match` (path-aware: `*` and `?` don't match `/`, `**` = zero or more path segments, `{a,b}`, `[..]`, `[^..]`/`[!..]`); invalid pattern ⇒ treated as no match.

| Pattern | Path | gobwas (default) | doublestar |
|---|---|---|---|
| `*.rb` | `folder/subfolder/0.rb` | ✓ | ✗ |
| `*.rb` | `2.RB` | ✓ (lowercased) | ✓ (lowercased) |
| `**/*.rb` | `0.rb` | ✗ (needs a `/`) | ✓ |
| `**/*.rb` | `folder/1.rb` | ✓ | ✓ |
| `src/*.js` | `src/lib/util.js` | ✓ (docs claim ✗ — drift) | ✗ |
| `*.rb?` | `1.rBs` | ✓ | ✓ |
| `*.{a,b,cn}` | `f.cn` | ✓ | ✓ |
(vectors from `filter/filter_test.go`)

**exclude** (`filter.go:135-188`): same engine choice but **case-sensitive** (no lowercasing); gobwas `MustCompile` per pattern (panic on invalid); file kept iff it matches **none**. Hook-level `exclude` + CLI `--exclude` + ancestors' + job's are all combined [test `exclude.txt`: `exclude: '*.txt'` removes `dir/a.txt`; `exclude: a.txt` keeps `dir/a.txt`].

**root** (`filter.go:190-204`): `r = TrimRight(root, "/")`; keep files with `strings.HasPrefix(path, r)` — **no path-boundary check** (`root: client` also keeps `client-old/x`); rewrite with `strings.Replace(path, r, ".", 1)` ⇒ `client/src/a.js` → `./src/a.js` (paths passed to the command are root-relative with a `./` prefix; `client-old/x` → `.-old/x`).

**file_types** (`filter.go:209-326`): resolve path as `<repo>/<root>/<path>`; `Lstat` (symlinks not followed); stat error → log `Couldn't check file type of <f>: <err>` and drop.

| Value | Keep file iff |
|---|---|
| `text` | regular file AND first ≤1024 bytes are text: has a UTF-8/16/32 BOM, or contains no byte in `0x00–0x08, 0x0B, 0x0E–0x1A, 0x1C–0x1F` (`detect_text.go:31-47`) |
| `binary` | regular file AND not text |
| `executable` | any of `0o111` bits AND not symlink |
| `not executable` | NOT (exec bits AND not symlink) — symlinks included |
| `symlink` | is symlink |
| `not symlink` | not symlink |
| contains `/` and known to mimetype lib (`text/x-python`, `text/x-sh`, `text/x-shellscript`, `text/x-perl`, `text/x-lua`, `text/x-php`, `text/html`, `text/xml`, `text/javascript`, `application/json`, …) | regular file AND `mimetype.DetectFile(path).Is(any listed)` (content sniffing incl. shebang) |
| anything else | warning `Unknown filter type: <t>`, ignored |

Simple types combine with **AND** (bit flags); MIME types with **OR**; the two groups AND together. Text/binary detection is done per file per job (no cache).

### 7.3 Template substitution (`internal/run/controller/command/replacer/replacer.go`) [src]

Templates recognized in `run` + `" " + args` (commands), in `args` (scripts), and in `setup[].run`:

| Template | Value | Escaping |
|---|---|---|
| `{staged_files}` `{push_files}` `{all_files}` `{files}` | filtered file lists (§7.1/7.2) | each file `shellescape.Quote` (single quotes only when needed) |
| `{0}` | all git hook args joined with `" "` | **none** |
| `{1}`…`{N}` | N-th git hook arg | **none** |
| `{<name>}` from `templates:` | raw string | **none** (2.0.8: "do not escape custom templates") |
| `{lefthook_job_name}` | printable job name | `shellescape.Quote` |
| `{cmd}` | only at config-merge time (§2.5) | — |
| any other `{x}` | left verbatim | — |

- Discovery (`replacer.go:99-126`): for each file template, `strings.Count(source, tmpl)`; the file list is computed **only if the template occurs** (lazy). Errors → job failure `error replacing <tmpl>: <err>`.
- Quote-aware replacement (`replacer.go:267-294`): for each template, three passes: `"{t}"` → every file has its surrounding `'…'` (from shellescape) stripped by regex `^'(.*)'$` and is wrapped in `"…"`; `'{t}'` → stripped and wrapped in `'…'`; bare `{t}` → shellescaped list. Files joined with a single space. ⚠️ `"{t}"` turns `'$(cmd).js'` into `"$(cmd).js"` ⇒ command substitution executes; names containing `'` are mangled (§17 #1).
- Command-line splitting (`ReplaceAndSplit`, `replacer.go:175-225`): budget `maxlen = MaxCmdLen + Σ(cnt·len(template)) − len(command)`, divided by the total template occurrence count; files are taken greedily (`getNChars`, space-separated byte length; at least one file per chunk) for every template in each round, producing N complete command strings executed **sequentially** (stop at first failure). `MaxCmdLen` (`internal/system/limits.go:9-24`): Linux/other **130000**, macOS **260000**, Windows **7000** bytes (deliberately below 8191, issue #655).

### 7.4 Skip-when-no-files decision table (commands, `command/build_command.go:14-84`)

Evaluated in order; `--force` (`-f`) disables steps 3, 4, 6, 7.

| # | Condition | Outcome |
|---|---|---|
| 1 | `run` contains both `{staged_files}` and `{push_files}` | **failure** `one of your runners contains incompatible file types` |
| 2 | template discovery error | failure `error replacing …` |
| 3 | any discovered template (file **or custom**) has an empty value list | skip `no files for inspection` |
| 4 | job has `files` cmd but `{files}` not used in run, and filtered `files` output is empty | skip `no files for inspection` |
| 5 | some file template was substituted with ≥1 file (or `--force`) | **run** |
| 6 | hook is `pre-commit`: filtered staged files empty AND filtered staged-incl-deleted empty | skip `no matching staged files` |
| 7 | hook is `pre-push`: filtered push files empty | skip `no matching push files` |
| 8 | otherwise | **run** (e.g. `post-checkout` jobs always run; `glob` without template is ignored there) |

Consequences: in `pre-commit`, a template-less job with `glob: "*.js"` runs only if a staged (incl. deleted) path matches — i.e. glob doubles as a trigger [test `pre-commit_issue_919.txt`]; a commit with nothing staged (`--allow-empty`) skips every job; a commit staging only deletions skips `{staged_files}` jobs with `no files for inspection` [test `run_deleted_only.txt`].

Scripts (`command/build_script.go:28-98`): only `args` templates are checked (step 3); **no implicit pre-commit/pre-push skip** (scripts always run). Script lookup iterates **all** source dirs in order: `<root>/<source_dir>`, `<root>/<source_dir_local>`, `<root>/.config/lefthook`, `<root>/.config/lefthook-local`, then each remote's `<remote>/<source_dir>` (`run.go:197-221`); **every** existing copy is executed (sequential commands). Non-regular file → skip `script files is not a regular file`; none found → failure `script does not exist: <name>`; no exec bit → `chmod 0751` the file in place (`build_script.go:16, 68-74`). Command: `[<runner> ]'<shell-quoted path>' [<args with templates> | <git args joined raw>]`.

---

## 8. Conditions, tags, selection

### 8.1 `skip` / `only` grammar and evaluation (`internal/config/skip_checker.go`) [src + test]

```
condition := bool | STATE | [ item, ... ]
item      := STATE | { ref: <branch or glob> } | { run: <sh command> }
STATE     := "merge" | "rebase" | "merge-commit"
```

- `skip(c)` true ⇒ skipped. Else if `only` set: skipped iff `only` does **not** match. `skip` takes precedence (`skip_checker.go:27-43`).
- A list matches if **any** item matches (OR). A map item with both `ref` and `run` matches if **either** does (OR, not AND).
- Unrecognized types/values → no match (silently).
- Repo state (`wrapper/state.go:27-112`, computed lazily once per run, exactly one value): `merge` if `<git-dir>/MERGE_HEAD` exists; else `rebase` if `<git-dir>/rebase-merge` or `rebase-apply` exists; else `merge-commit` if `git show --no-patch --format="%P"` output contains a space (HEAD has ≥2 parents) [test `skip_merge_commit.txt`]; else `""`.
- `ref`: branch from `<git-dir>/HEAD` line `ref: refs/heads/<branch>` (detached ⇒ `""`); matches if equal **or** `glob.MustCompile(ref).Match(branch)` (gobwas, no separators ⇒ `dev/*` matches `dev/a/b`; invalid glob ⇒ panic).
- `run`: `sh -c <cmd>` (Windows: located sh.exe), stdin null, cwd = **lefthook's process cwd** (repo root when invoked by git; the subdirectory when `lefthook run` is invoked manually from one), output captured to debug log; match iff exit 0 (`command_executor.go:18-50`). Not cached — evaluated every time a skip check runs.
- Evaluated at hook level (once), group level, and job level.

### 8.2 Tags and names

- Job skipped with reason `tags` if `exclude_tags` (hook + `LEFTHOOK_EXCLUDE`) intersects the job's tags ∪ ancestor group tags; reason `name` if `exclude_tags` contains the job's printable name (`controller/job.go:189-203`).
- `--tag t` (repeatable): run job iff its tags or inherited tags intersect; `--job n` / `--command n`: run iff `name` ∈ list (a selected group runs all children). Non-matching jobs → silent skip results. [test `cli_run_only.txt`]

---

## 9. Process execution [src]

| Aspect | Unix (`exec/exec_unix.go`) | Windows (`exec/exec_windows.go`) |
|---|---|---|
| shell | `sh -c <cmd>` (`sh` via PATH) (`:81`) | `sh.exe` resolved by `system.Sh()` (`internal/system/sh_windows.go:17-56`): if env `GIT_INDEX_FILE` set (inside a git hook) → plain `sh`; else `C:\Program Files\Git\bin\sh.exe` if it exists; else `sh` on PATH; else `<dir of git.exe>\..\..\bin\sh.exe`. Command line built raw: `"<sh>" -c "<cmd>"` via `SysProcAttr.CmdLine` — **no escaping of `"` in cmd** (`:79`) |
| cwd | `abs(<repo>/<root>)` | same |
| env | `os.Environ()` + job env (`NAME=os.ExpandEnv(value)` — lefthook expands `$VAR`/`${VAR}` from *its own* env, unset → `""`) + color var (`NO_COLOR=true` if colors disabled; else `CLICOLOR_FORCE=1` if colors explicitly forced and `CLICOLOR_FORCE` not already set in env or job env) (`exec/executor.go:33-51`). **`LEFTHOOK=0` is not set for jobs** (only for lefthook's own git calls) | same (bug: `make([]string, len(env))` prepends empty strings) |
| interactive / use_stdin | stdin: `/dev/tty` if `interactive` and stdin is not a TTY (open failure → error log `Couldn't enable TTY input: <err>`); else cached stdin if `use_stdin`; else null. stdout → buffer (or `os.Stdout` when follow/interactive and execution logging on); stderr → `os.Stderr` directly (`:86-93`) | stdin via `go-tty` (`CONIN$`); stdout → buffer/out; stderr → `os.Stderr` |
| default, stdout is a TTY | `pty.StartWithSize` with the real terminal size; stdin/stdout/stderr = PTY; output copied into the job buffer (`:94-102`) | — (no PTY) |
| default, no TTY | `Setpgid`; stdout **and stderr merged** into the buffer; stdin null; cancel = `kill(-pgid, SIGKILL)` (`:103-124`) | stdout → buffer, stderr → `os.Stderr` |
| multiple chunks | executed sequentially; first error stops (`:70-77`) | same |
| timeout | `context.WithTimeout`; on expiry result failure text `timeout (<Go Duration.String()>)`, e.g. `timeout (100ms)` [test `timeout.txt`] | same |
| SIGINT | `signal.NotifyContext(os.Interrupt)` cancels ctx ⇒ children killed (pgid SIGKILL in no-TTY mode; PTY mode kills only the session leader [derived]) ⇒ guard restores state ⇒ `Error: Interrupted`, exit 1 [test `run_interrupt.txt` expects `signal: killed` and `Error: Interrupted`] | — |
| SIGTERM/SIGHUP | **not handled** ⇒ Go default (immediate exit, no restoration of hidden unstaged changes) [derived] | — |

Stdin caching (`utils/cached_reader.go`): the first reader drains `os.Stdin` into memory; after EOF later readers replay the buffer. Not goroutine-safe; with `parallel` several `use_stdin` jobs race. The docs' claim "only one will receive the data" is inaccurate for sequential runs.

Job output (non-follow) is held in a `bytes.Buffer` per job (unbounded) and printed once the job ends, under the logger mutex, so parallel outputs don't interleave.

---

## 10. Partially staged files, `stage_fixed`, `fail_on_changes` — the guard

Source: `internal/run/controller/guard.go`, `internal/git/repo.go:216-347`, `internal/git/wrapper/*`.

### 10.1 Activation (`controller.go:93-100`, `guard.go:50-62`)

- `stashUnstagedChanges = !--no-stage-fixed && hook == "pre-commit"` — note: it is tied to the hook *name*, not to whether any job has `stage_fixed`. `lefthook run pre-commit` invoked manually also hides/restores.
- `failOnChanges` from §4.
- If both false → jobs run with no wrapping. Otherwise `withHiddenUnstagedChanges(withFailOnChanges(run jobs))`.

### 10.2 Algorithm `withHiddenUnstagedChanges` (exact git commands)

Patch paths (`wrapper/wrapper.go:18-19, 120-152`): `<info>/lefthook-unstaged.patch` and `<info>/lefthook-unstaged-all.patch`; in a linked worktree (git-dir ≠ common-dir) a suffix `.<basename(git-dir)>` is inserted before `.patch` (2.2.0, #1582) because `<info>` is shared by all worktrees.

1. `partial = PartiallyStagedFiles()` from `git status --short --porcelain -z`: entries with index status ∉ {` `, `?`} **and** worktree status ∉ {` `, `?`} (`repo.go:219-245`). Error → warn `Failed to find partially staged files: <err>` and **abort the hook** (jobs not run).
2. If `partial` is empty: run jobs; then `git add --force -- <filesToStage…>` (batched by length); failure → warn `Couldn't stage fixed files:<err>` and the hook fails with `couldn't stage fixed files: <err>`. Done. **Fully-unstaged files are never hidden** — tools that read the worktree see unrelated unstaged edits.
3. Save: `git diff --binary --unified=0 --no-color --no-ext-diff --src-prefix=a/ --dst-prefix=b/ --patch --submodule=short --output=<unstaged.patch> -- <partial…>` (batched by length; with several batches each later batch **overwrites** the same `--output` file — §17) then the same without pathspec to `<unstaged-all.patch>` (`wrapper/save_unstaged_diff.go`). Then backup: `git stash create` → `<sha>`; `git stash store --quiet --message "lefthook auto backup" <sha>` (`wrapper/store_stash.go`). Error → warn `Failed to save unstaged changes: <err>`, abort.
4. Hide: `git checkout --force -- <partial…>` (batched) — only those files revert to their index content. Error → warn `Failed to hide unstaged files: <err>`, abort.
5. Run `fn` (fail-on-changes wrapper + jobs).
6. If `fn` returned `FailOnChangesError`: `git checkout --force -- <changed files>` to **revert the hooks' modifications** (on error warn `Failed to revert file changes: <err>` and return).
7. Applicability: if the patch file is missing/empty → applicable; else `git apply -v --whitespace=nowarn --recount --unidiff-zero --check -- <unstaged.patch>` (`wrapper/unstaged_diff_applicable.go`).
8. **Applicable**: `git add --force -- <filesToStage…>` (stage fixes *before* restoring unstaged hunks, so the index gets exactly the fixed staged content); then restore: `git apply -v --whitespace=nowarn --recount --unidiff-zero -- <unstaged.patch>` (skipped if empty), delete both patch files, drop every stash entry whose `git stash list` line matches `^<ref>:\s*lefthook auto backup` — iterating **bottom-up**, `git stash drop --quiet -- <ref>` (`wrapper/drop_stash.go`).
9. **Not applicable** (hook fixes overlap unstaged hunks): log `Error: <fn error>` if any; result error becomes `conflict while merging unstaged changes`; `git checkout .` (**discards every unstaged change in the repo, including all hook fixes**); warn block:
   ```
   Unable to restore previously hidden unstaged changes.
   This may happen when changes introduced by the hook conflict with your unstaged changes.
   Stage all changes with `git add -A` and try again.
   ```
   then apply `<unstaged-all.patch>` (restores the user's unstaged work everywhere), delete patches, drop the backup stash. Nothing is staged. [test `restore_unstaged_on_conflict.txt`: unrelated unstaged `c.txt` survives; hook edit to `b.txt` is discarded; no patch files or stash entries remain]
10. Restore errors → warn `Failed to restore unstaged files: <err>`; if the patch file vanished → warn `Saved unstaged changes not found. Restore them from the 'lefthook auto backup' stash: git stash list` and **keep** the stash (`repo.go:279-298`).

Final user-visible error (via `run.go:263-275`): `Error: failed to run the hook: conflict while merging unstaged changes`.

### 10.3 `stage_fixed` (`controller/job.go:154-180`)

After a **successful** job (non-zero exit ⇒ nothing staged), if hook == `pre-commit` and `stage_fixed` and not `--no-stage-fixed`:
- files = the file list substituted into the command (all file templates combined); if none (no template / script) → `git diff --cached --name-only --diff-filter=ACMR` filtered by the job's glob/exclude/root/file_types (staged-files lookup failure → warn `Couldn't stage fixed files:<err>` and the job still succeeds);
- if `root` set, each file is re-joined `root/<file>`;
- added to a mutex-protected set; actual `git add --force --` happens once in the guard (§10.2 step 2/8).
- Pathspec magic is not disabled: a file literally named `[file].js` is passed as a glob pathspec (works because git matches exact names too, but may also stage other files matching the pattern) [test `stage_fixed.txt` uses `[file].js`].

### 10.4 `fail_on_changes` (`guard.go:157-207`, `repo.go:307-347`)

- `Changeset()` = map path→hash over `git status --short --porcelain -z`: `"deleted"` if X or Y is `D`; `"directory"` if the path ends with `/` or is a dir (submodules, dir symlinks); else `git hash-object -- <paths…>` (batched). Taken before and after the jobs; any difference ⇒ `FailOnChangesError` → message `files were modified by a hook, and fail_on_changes is enabled` (`Error: …`, exit 1, not wrapped by "failed to run the hook").
- With `fail_on_changes_diff` the changed files (sorted) are shown via `git diff [--color] -- <files>` as a warning.
- Inside the stash guard, step 6 reverts the modified files so the worktree returns to the pre-hook state [test `fail_on_changes_recover_previous_change.txt`].

### 10.5 Comparison with pre-commit (`pc:pre_commit/staged_files_only.py`)

| Aspect | pre-commit 4.6.2 | lefthook 2.2.0 | snp recommendation |
|---|---|---|---|
| What is hidden | **all** unstaged changes of tracked files: `git diff-index --ignore-submodules --binary --exit-code --no-color --no-ext-diff <write-tree> --` | only unstaged hunks of partially staged files | keep pre-commit (compat: hooks must see exactly the index) |
| Hide mechanism | `git -c submodule.recurse=0 checkout -- .` | `git checkout --force -- <files>` | pre-commit |
| Intent-to-add files | `git rm --cached` then re-`add --intent-to-add` | not handled | pre-commit |
| Backup | patch file `~/.cache/pre-commit/patch<unix-ts>-<pid>`; log `Stashing unstaged files to <p>.` | patch in `.git/info` + **`git stash store` entry "lefthook auto backup"** | pre-commit's patch file + message; optional stash backup (opt-in, costs a `stash create`) |
| Applicability check | none — tries `git apply`, on failure rolls back | `git apply --check` first | **check first** (lets stage_fixed decide before touching the index) |
| Conflict | warn `Stashed changes conflicted with hook auto-fixes... Rolling back fixes...`, `checkout -- .`, re-apply patch; the hook that modified files has already failed | `checkout .`, apply all-patch, hook fails with conflict error | pre-commit messages; never stage on conflict |
| Fix staging | never | `stage_fixed` | opt-in `stage_fixed` (§18) |
| Signals | Python finally-blocks on KeyboardInterrupt | SIGINT only | SIGINT/SIGTERM/SIGHUP + leftover-patch detection |
| autocrlf retry | `git -c core.autocrlf=false apply` retry | none | pre-commit |

---

## 11. Git LFS passthrough (`controller/lfs.go`, `internal/git/lfs.go`) [src]

Because lefthook *replaces* the hooks git-lfs installs, it re-implements them for `post-checkout`, `post-commit`, `post-merge`, `pre-push`:
- skipped if `skip_lfs`/`--skip-lfs`, or for `pre-push` with zero git args (manual run);
- `git-lfs` not on PATH: if `<root>/.lfs-required` or `<root>/.lfsconfig` exists → error log `This Repository requires Git LFS, but 'git-lfs' wasn't found.\nInstall 'git-lfs' or consider reviewing the files:\n  - <root>/.lfs-required\n  - <root>/.lfsconfig\n` and the hook fails (`git-lfs is required`); else silently continue;
- runs `git lfs <hook> <git args…>` (stdin = cached hook stdin for `pre-push` only, 2.1.15 fix), stdout printed at info level `[git-lfs] stdout: …`; failure only fatal if one of the two files exists (warn `git-lfs command failed` + output).

pre-commit's model instead: `pre-commit install` moves an existing hook (e.g. git-lfs's) to `<hook>.legacy` and its shim runs the legacy hook first (`pc:pre_commit/commands/hook_impl.py:30-46`).

---

## 12. Output and logging [src; rendering details derived from lipgloss]

All lefthook logging goes to **stdout** (`logger.New(os.Stdout)`); only `main`'s final `Error: …` and interactive/no-TTY job stderr go to stderr. (Git forwards hook stdout to the terminal's stderr, which is why integration tests assert on `stderr`.)

### 12.1 `output` sections (`internal/logger/execution_settings.go`)

| Name (aliases) | Enables |
|---|---|
| `meta` | version/hook box |
| `summary` | summary header **+ success + failure** lines |
| `success` | ✔️ lines |
| `failure` | 🥊 lines |
| `empty_summary` | `summary: (skip) empty` when no results |
| `skips` | `(skip)` lines |
| `execution` (`jobs`) | job header + output + error |
| `execution_out` (`jobs_out`) | job output (implies execution) |
| `execution_info` (`jobs_info`) | job header `name ❯` (implies execution) |
| `setup` | setup header + output |

- Unset/any non-bool non-list → **everything** on. `output: false` → nothing (errors/warnings still print). `output: true` → everything.
- `LEFTHOOK_OUTPUT` (non-empty) replaces the config value; `false` → nothing; else comma-separated names. Unknown name → warning `unknown output setting: "<name>"` and **the remaining names in the env string are ignored** (`break`) (`execution_logger.go:61-74`); in a config list unknown names are warned and skipped.

### 12.2 Line formats (plain-text form; colors in §12.3)

| Element | Format | Source |
|---|---|---|
| meta box | rounded border, padding 0/1: `🥊 lefthook  v2.2.0   hook:  <hook>` (no-color: `lefthook  v2.2.0   hook:  <hook>`; spacing from lipgloss joining args with a space [derived]) | `execution_logger.go:179-203` |
| skip | left border `│`, padding 2: `│  <name> (skip) <reason>` | `:90-107` |
| execution header | thick left border `┃`: `┃  <name> ❯ ` then the captured output verbatim, then (on error) the Go error text, e.g. `exit status 1`, `signal: killed` | `:117-157` |
| setup header | `┃  setup ❯ ` (yellow) | `:159-177` |
| separator | two spaces + 36 × `─` (only when summary and execution are both enabled) | `:109-115` |
| summary header | `summary: (done in 0.52 seconds)` (`%.2f`) | `run.go:290-318` |
| empty | `summary: (skip) empty` | same |
| success | `✔️ <name> (0.12 seconds)`; no-color `✓ <name> (0.12 seconds)`; nested group children indented 2 spaces per level | `execution_logger.go:205-219` |
| failure | `🥊 <name>: <fail_text or error text> (0.12 seconds)`; no-color `✗ …`; without text `🥊 <name> (0.12 seconds)` | `:221-240` |

Summary prints **all successes first, then all failures** (each in results order — completion order for parallel), skipped jobs omitted (`run.go:320-348`). Warnings/errors/debug lines get a colored left border `│ ` (yellow/red/gray); multiple args are space-joined. Info lines use `fmt.Sprint` (no space between string operands ⇒ e.g. `Added config:/repo/lefthook.yml`, `sync hooks: ✔️(pre-commit, pre-push)` [derived]).

### 12.3 Colors (`internal/logger/{logger,colors}.go`, `exec/executor.go`)

- Default palette by detected profile (`colorprofile.Detect(stdout, env)`, so non-TTY stdout ⇒ no escapes unless forced): cyan ANSI 14 / 256:73 / `#5FAFAF`; gray 8/102/`#878787`; green 2/34/`#00AF00`; red 9/203/`#FF5F5F`; yellow 3/3/`#808000`.
- Env precedence at startup (`logger.go:166-184`): `FORCE_COLOR` enabled → default colors; else `CLICOLOR_FORCE` enabled → default; else `NO_COLOR` enabled → none; else `CLICOLOR` disabled (`0`/`false`/`off`) → none; else default. "Enabled" = non-empty and not `0`/`false`/`off` (so `NO_COLOR=0` does **not** disable — deviates from no-color.org).
- `--colors on|yes|true|1` / `off|no|false|0` (run only) sets a *forced* flag that later config cannot override. Config `colors: true|"on"` forces ANSI-or-better even off-TTY; `false|"off"` disables; map → custom colors (ints → ANSI-256 code, strings → hex/ANSI).
- Propagation to jobs: `NO_COLOR=true` when disabled; `CLICOLOR_FORCE=1` when explicitly forced (never overwrites an existing `CLICOLOR_FORCE`).

### 12.4 Spinner (`internal/logger/spinner.go`)

Shown only when stdout is a TTY and not `no_tty`/`--no-tty`/`follow`; char set 14 of briandowns/spinner, 100 ms refresh, suffix ` waiting` or ` waiting: job1, job2` truncated to terminal width (fallbacks ` waiting: N hooks`, `… , ... (N more)`). Every log line stops/restarts it.

---

## 13. CLI reference [src]

Global: `lefthook [--help|-h] [--version|-v] <command>`; urfave/cli v3 with `EnableShellCompletion` and `Suggest` (did-you-mean). Description lists env vars `LEFTHOOK`, `LEFTHOOK_CONFIG`, `LEFTHOOK_OUTPUT`, `LEFTHOOK_VERBOSE` (`cmd/lefthook.go:15-21`). Exit code: **0** success, **1** any error (message `Error: <msg>` on stderr unless empty). Every command except `version`/`self-update` requires being inside a git repo (else `Error: …` from `git rev-parse`, exit 1; `validate` exits **0** silently — §17).

### 13.1 `lefthook run <hook-name> [git-args...] [options]` (`cmd/run.go`)

| Flag | Type | Semantics |
|---|---|---|
| `--verbose`, `-v` | bool | debug logs |
| `--colors <on\|off\|auto>` | string, default `auto` | §12.3 |
| `--job <name>` | repeatable | run only named jobs (group names select whole group) |
| `--tag <tag>` | repeatable | run only jobs with tag (incl. inherited) |
| `--command <name>` | repeatable | alias merged into `--job` |
| `--exclude <glob>` | repeatable | extra exclude globs for all jobs |
| `--file <path>` | repeatable | replace **all** file templates with these paths (no existence check) |
| `--force`, `-f` | bool | don't skip when file lists are empty (§7.4) |
| `--all-files` | bool | replace file templates with `git ls-files --cached` (appended to `--file` list — docs say `--all-files` is ignored when `--file` given; code unions them) |
| `--files-from-stdin` | bool | read stdin, split on **NUL only** (`\0`), one trailing NUL ignored (2.2.0), appended to `--file` (`run.go:351-353`); takes precedence over `--all-files` |
| `--no-auto-install` | bool | skip shim sync |
| `--no-stage-fixed` | bool | ignore `stage_fixed` **and** disable the partial-staging guard |
| `--no-tty` | bool | no spinner, no interactive |
| `--skip-lfs` | bool | skip LFS passthrough |
| `--fail-on-changes` / `--fail-on-changes=false` (also `--no-fail-on-changes` via `BoolWithInverseFlag`) | tri-state | overrides hook `fail_on_changes` |
| (`--fail-on-changes-diff`) | — | **declared but not registered** in `Flags` (`cmd/run.go:20` vs `:97`) ⇒ unusable |

Errors: `hook name missing`; `hook <h> doesn't exist in the config` (only for names that are not standard git hooks); piped+parallel; `no job matching …`; min_version; `failed to run the hook: <err>`; `files were modified by a hook, and fail_on_changes is enabled`; `Interrupted`; `failed to read the files from standard input: <err>`; `failed to get all files: <err>`.

### 13.2 `lefthook install [hook-names...] [--force|-f] [--reset-hooks-path|-r] [--verbose|-v]`

1. If no main config (name-major lookup or `LEFTHOOK_CONFIG`): info `Config not found, creating...`, write `lefthook.yml` (mode 0666 pre-umask) from `templates/config.tmpl` (all-comment example), info `Added config:<path>`.
2. Sync every remote (failures are warnings); if any synced, reload config.
3. `core.hooksPath` check (§14.3), then write shims for every configured hook (or only the named ones), checksum file, `sync hooks: ✔️(…)`.
4. AI hook files (§14.6).

`--force`: overwrite existing `<hook>.old` (info `\nFile <hook>.old already exists, overwriting\n`), install despite `core.hooksPath`, re-clone remotes. `--reset-hooks-path`: `git config --local --unset-all core.hooksPath` / `--global …` then re-resolve paths (warns `local core.hooksPath has been unset.` / `global core.hooksPath has been unset.`).

### 13.3 Other commands

| Command | Flags | Behavior / messages |
|---|---|---|
| `uninstall` | `--force/-f` (remove *all* hook files), `--remove-configs`, `-v` | delete every file in hooks dir that contains the string `LEFTHOOK` (unless forced); for each deleted `<h>`, rename `<h>.old` back; remove `.git/info/lefthook.checksum`; strip AI hook entries; `--remove-configs` deletes `{lefthook,.lefthook,.config/lefthook,lefthook-local,.lefthook-local,.config/lefthook-local}.{yml,yaml,toml,json}` (**not** `.jsonc`); `RemoveAll(.git/info/lefthook-remotes)` (`internal/command/uninstall.go`) |
| `add <hook>` | `--dirs`/`--create-dirs`, `--force/-f`, `-v` | only standard git hooks (`skip adding, hook is unavailable: <h>`); clean + write shim (with **empty** template args — no rc/roots/lefthook path until next sync); `--dirs` → `mkdir -p <source_dir>/<hook>` and `<source_dir_local>/<hook>` |
| `dump` | `--format/-f yaml\|json\|toml` | §2.6; errors `couldn't load config: …`, `couldn't dump config: …` |
| `validate` | `-v` | JSON-schema-validate main and secondary (merged extends/remotes/local) **raw** maps separately; prints a tree `  <instance/path>: <errors joined ",">` (yellow/red); success `All good`; failures `validation failed for main config` / `validation failed for secondary config` |
| `check-install` | `-v` | exit 0 iff a main config exists **and** the checksum says synced; else exit 1 (no output) (`check_install.go`) |
| `version` | `--full/-f`, `--verbose/-v` | prints `2.2.0` or `2.2.0 <commit>` (`-dev` suffix for dev builds) |
| `self-update` | `--yes/-y`, `--force/-f`, `-v` | GET `https://api.github.com/repos/evilmartians/lefthook/releases/latest`; asset `lefthook_<ver>_<OS>_<arch>[.exe]`; verify sha256 from `lefthook_checksums.txt`; prompt y/n; rename current exe to backup, move new into place, roll back on failure. Excluded from package-manager builds (`no_self_update` build tag) |

Shell completion prints configured hook names and long flags (`internal/command/lefthook.go:176-213`).

---

## 14. Installation model

### 14.1 Paths and file modes

- Hooks dir = `git rev-parse --path-format=absolute --git-path hooks` (resolves `core.hooksPath` and worktree common dir). Created with `0755` if missing. Shim files written with mode `0755`.
- Identification of lefthook-owned files: **any line containing `LEFTHOOK`** (`lefthook.go:31, 96-119`).
- Existing non-lefthook hook → renamed to `<hook>.old` (error `can't rename <h> to <h>.old - file already exists` unless `--force`; info `Renamed <path> to <path>.old`). **lefthook never runs `.old` hooks** (contrast pre-commit "migration mode" running `.legacy` first).
- Custom (non-git) hook names: `cleanHook` still runs for them (removes a same-named file in hooks dir), but no shim is written unless `install_non_git_hooks: true` (`install.go:315-339`).

### 14.2 The hook shim (`internal/templates/hook.tmpl`, rendered by `templates.go:38-60`)

Essential logic (verbatim structure, template conditionals shown as comments):

```sh
#!/bin/sh

if [ "$LEFTHOOK_VERBOSE" = "1" -o "$LEFTHOOK_VERBOSE" = "true" ]; then
  set -x
fi

if [ "$LEFTHOOK" = "0" ]; then
  exit 0
fi

# {{if .Rc}}
[ -f <rc> ] && . <rc>
# {{end}}

call_lefthook()
{
  if test -n "$LEFTHOOK_BIN"
  then
    "$LEFTHOOK_BIN" "$@"
  elif test -n "<lefthook option>"          # only if `lefthook:` set
  then
    <lefthook option> "$@"
  elif lefthook<.exe> -h >/dev/null 2>&1
  then
    lefthook<.exe> "$@"
  elif lefthook.bat -h >/dev/null 2>&1       # Windows only (Ruby bundler wrapper)
  then
    lefthook.bat "$@"
  elif '<abs path of the installing binary>' -h >/dev/null 2>&1   # 2.0.6+
  then
    '<abs path of the installing binary>' "$@"
  else
    dir="$(git rev-parse --show-toplevel)"
    osArch=$(uname | tr '[:upper:]' '[:lower:]')
    cpuArch=$(uname -m | sed 's/aarch64/arm64/;s/x86_64/x64/')
    # then, in order, test -f + run:
    #  $dir/node_modules/lefthook-${osArch}-${cpuArch}/bin/lefthook
    #  $dir/node_modules/@evilmartians/lefthook/bin/lefthook-${osArch}-${cpuArch}/lefthook
    #  $dir/node_modules/@evilmartians/lefthook-installer/bin/lefthook
    #  $dir/node_modules/lefthook/bin/index.js
    #  the same four under $dir/<root>/ for every distinct job `root` in the config
    #  go tool lefthook | bundle exec lefthook | yarn lefthook | pnpm lefthook |
    #  swift package lefthook | mint run csjones/lefthook-plugin | uv run lefthook |
    #  mise exec -- lefthook | devbox run lefthook   (each probed with `-h`)
    echo "Can't find lefthook in PATH"
    # {{if .AssertLefthookInstalled}}
    echo "ERROR: Operation is aborted due to lefthook settings."
    echo "Make sure lefthook is available in your environment and re-try."
    echo "To skip these checks use --no-verify git argument or set LEFTHOOK=0 env variable."
    exit 1
    # {{end}}
  fi
}

call_lefthook run "<hook-name>" "$@"
```

Notes: the shim does **not** `exec` (one extra sh process stays alive); each fallback probe spawns a process (`-h`), costing tens to hundreds of ms when lefthook isn't on PATH; without `assert_lefthook_installed` a missing binary makes the hook **succeed silently** (prints only `Can't find lefthook in PATH`); `rc` and `root` values are interpolated unescaped; the installing binary path is single-quote-escaped. Stdin passes through to lefthook (pre-push refs).

### 14.3 `core.hooksPath` policy (`install.go:503-609`)

If `core.hooksPath` is set locally (and differs from `<root>/.git/hooks`) or globally, `install` (and auto-sync) refuses without `--force`/`--reset-hooks-path`, logging (error level):

```
core.hooksPath is set locally to '<local>'
core.hooksPath is set globally to '<global>'
Custom hooks paths are not supported by default.

Reset core.hooksPath automatically:
  lefthook install --reset-hooks-path

Remove it manually:
   git config --unset-all --local core.hooksPath
   git config --unset-all --global core.hooksPath

Force installation into current hooks path:
  lefthook install --force

  Target:
  <local or global path>
```
(only the applicable `is set …` / hint lines appear). With `--force`: warnings `core.hooksPath is set locally to '<p>'` … `Installing hooks anyway in '<p>'`. pre-commit: `Cowardly refusing to install hooks with \`core.hooksPath\` set.` (+ hint) — snp keeps pre-commit's behavior; consider lefthook's `--reset-hooks-path` as an extra flag.

### 14.4 Ghost hook

When the config defines **no** hooks (and no hook names were passed to `install`), `install` writes a `prepare-commit-msg` shim anyway (`config.GhostHookName`, `install.go:341-351`) so a later commit triggers `run`, which auto-syncs shims for hooks added meanwhile; `run` for an unconfigured `prepare-commit-msg` lowers logging to warnings. (Since 2.0.16 only when no hooks are configured.)

### 14.5 Checksum and auto-sync (`install.go:159-232, 401-489`; `templates.go:16, 72-74`)

- File `<git-info>/lefthook.checksum`, mode 0644, one line: `<md5-hex> <unix-mtime-of-main-config> <hook1,hook2>\n` (`%s %d %s\n`; third field = hook names passed to `install`, empty for "all").
- Parse regex `(\w+)\s+(\d+)(?:\s+([\w,-]+))?`.
- Synced iff stored mtime == current main-config mtime (fast path, **ignores local/extends/remote edits**), else iff stored md5 == md5(compact JSON dump of the freshly merged config).
- `run` (unless disabled) → not synced ⇒ `installHooks(cfg, storedHooks)` (same as install incl. hooksPath check; failure ⇒ warning `Skipping hook sync: <err>`); success prints `sync hooks: ✔️(…)`; failure prints `sync hooks: ❌`.

### 14.6 AI agent hooks (beta, `internal/command/install_ai.go`)

`ai: {claude|codex|cursor|copilot: {<provider event>: <lefthook hook name>}}`. `install` validates that each referenced hook exists (errors `ai.<provider>.<event> -> "<hook>"` sorted, then `ai hooks misconfigured`), then writes:
- `.claude/settings.json` / `.codex/hooks.json`: `{"hooks": {"<Event>": [{"hooks": [{"type": "command", "command": "<bin> run <hook>"}]}]}}` (preserving user entries; lefthook-managed entries recognized by the `… run …` command prefix and replaced);
- `.cursor/hooks.json`: `{"version": 1, "hooks": {"<event>": [{"command": "<bin> run <hook>"}]}}`;
- `.github/hooks/lefthook.json` (Copilot): same shape, file rewritten from scratch (always deleted first on install; removed on uninstall).
`<bin>` = trimmed `lefthook:` option or plain `lefthook` (never a machine-specific path).

### 14.7 Packaging

- npm `lefthook`: platform `optionalDependencies` (`lefthook-<os>-<arch>`), `bin/index.js` spawns the native binary (exit code collapsed to 0/1); `postinstall.js` runs `<exe> install` in `INIT_CWD` unless `CI` is enabled and `LEFTHOOK` is not (`isEnabled = v && v !== "0" && v !== "false"`), errors only warn. (2.1.13: no longer forces install.)
- PyPI wheel: `python -m lefthook` → `bin/lefthook-<os>-<arch>/lefthook[.exe]` (`amd64→x86_64`, `aarch64→arm64`).
- RubyGems, Go (`go install …/v2@v2.2.0`, `go get -tool`), Homebrew, winget, scoop, apt/rpm/apk/AUR, snap, mise, devbox, Swift plugin.

---

## 15. Environment variables

Read:

| Variable | Where | Semantics |
|---|---|---|
| `LEFTHOOK` | shim (`= "0"` only) and `run` (`"0"` or `"false"`) | disable; npm postinstall: `1`/`true` forces install even when `CI` set |
| `LEFTHOOK_VERBOSE` | binary (enabled = non-empty, not `0`/`false`/`off`); shim (`1`/`true` → `set -x`); self-update (`1`/`true`) | debug logs |
| `LEFTHOOK_CONFIG` | loader, install | override main config path (relative → repo root) |
| `LEFTHOOK_OUTPUT` | run | override `output` (`false` or comma list) |
| `LEFTHOOK_EXCLUDE` | loader | comma list (untrimmed) appended to every hook's `exclude_tags` (tags or job names) |
| `LEFTHOOK_BIN` | shim | explicit binary path (highest priority) |
| `CI` | run (`fail_on_changes: ci/non-ci`, `fail_on_changes_diff` default — existence only), npm postinstall (enabled check) | — |
| `NO_COLOR`, `CLICOLOR_FORCE`, `CLICOLOR`, `FORCE_COLOR` | logger | §12.3 |
| `GIT_INDEX_FILE` | Windows `sh` lookup | "inside a git hook" heuristic |
| `INIT_CWD` | npm postinstall | project dir |

Set:

| Variable | For | Value |
|---|---|---|
| `LEFTHOOK` | lefthook's own git subprocesses and skip `run:` checks (`system/command.go:41-58`) | `0` |
| `NO_COLOR` | jobs when colors disabled | `true` |
| `CLICOLOR_FORCE` | jobs when colors explicitly enabled and not already set | `1` |
| job `env` | jobs | `$VAR`-expanded |
| (removed) `GIT_DIR`, `GIT_INDEX_FILE` | remote clone/fetch/pull | unset |

---

## 16. Performance techniques

Adopt (with snp-specific improvements):

1. **Shim fast exit** for the kill switch without starting the binary (`LEFTHOOK=0` check in sh).
2. **Speculative concurrent git queries** while the config parses (`Cache.WarmUp`: staged files, staged incl. deleted, status) + **once-per-run caches** (`sync.OnceValues`).
3. **Lazy template evaluation**: a file list (`ls-files`, push diff, `files` command, MIME sniffing) is computed only if the job references it.
4. **Single process-wide git lock** to avoid `index.lock` contention — snp should lock only *index-writing* commands and run read-only ones concurrently with `GIT_OPTIONAL_LOCKS=0` (lefthook serializes everything).
5. **Config checksum** to avoid re-installing shims — snp: hash *all* config inputs (main, local, extends), not mtime of main only.
6. **Batched git calls** (`hash-object`, `add`, `checkout`, `diff` take many paths per invocation, split by command-line length).
7. **Text detection on the first 1024 bytes** only.
8. **PTY capture** so tools keep colors while output stays un-interleaved.

Avoid:

- Unbounded goroutine-per-job parallelism; completion-order summaries.
- Per-job file-type sniffing without caching (same file sniffed by many jobs).
- `strings.Split` of non-`-z` git output + `strconv.Unquote`.
- `git stash create` on every commit with partial staging (writes tree objects) unless opted in.
- Shim probing package managers (`yarn lefthook -h`, `bundle exec …`) — each probe can cost 100 ms+.
- Unbounded in-memory output buffers.

---

## 17. Quirks, bugs and doc drift found (pitfalls for snp and for migration fidelity)

1. **Shell injection via file names**: `"{staged_files}"` wraps shellescape'd names in double quotes after stripping single quotes ⇒ `$(…)`/backticks in a staged file name execute (`replacer.go:267-294`). Git args `{0}`/`{N}` and custom `templates` are never escaped; script jobs append raw git args.
2. `glob.MustCompile` panics on invalid patterns (glob, exclude, `ref:`) ⇒ crash at hook time (`filter.go:126, 159`; `skip_checker.go:89`).
3. glob is case-insensitive, exclude case-sensitive; multiple matching globs duplicate files.
4. `root` prefix match has no `/` boundary (`client` matches `client-old/…`); paths passed as `./…`.
5. `group.root` and `group` alongside `run`/`script` are silently ignored.
6. Docs: `src/*.js` "matches src/file.js only" — false with gobwas default; "interactive jobs run after non-interactive" — not implemented in 2.2.0 (no reordering exists); "`--all-files` ignored with `--file`" — they are unioned; tags "comma-separated" — not split; remote extends "relative to remote root" — actually relative to the config file's dir; `{cmd}` in `runner` — not replaced; local config "must use the same dot prefix" — not enforced.
7. `--fail-on-changes-diff` flag declared but not registered.
8. `lefthook validate` outside a repo returns exit 0 silently (`cmd/validate.go:27-29`).
9. Loader (extension-major) and installer (name-major) config lookup orders differ.
10. Auto-sync fast path compares only main-config mtime.
11. Remote directory names collide for same-basename URLs.
12. `SaveUnstagedDiff` batches pathspecs but each batch writes the same `--output` file ⇒ only the last batch survives when partially staged paths exceed ~130 KB of names.
13. `DropStash` drops **every** `lefthook auto backup` stash entry, including leftovers from earlier crashed runs that may be the only copy of user work.
14. Only SIGINT is handled; SIGTERM/SIGHUP during a commit leave files checked out and the patch in `.git/info`.
15. `setup` with a non-empty file template passes a nil filter (nil dereference).
16. `use_stdin` with `parallel` races on a non-thread-safe cached reader.
17. Scripts are `chmod 0751`'d in the working tree (mode change can show up as a diff with `core.fileMode=true`); same-named scripts in several source dirs all run.
18. `LEFTHOOK_CONFIG` pointing to a missing file, or no config at all, makes hooks **pass** with only a warning (fail-open); a missing binary without `assert_lefthook_installed` also passes.
19. `NO_COLOR=0` doesn't disable color; `LEFTHOOK_OUTPUT` with one unknown name drops all later names.
20. `--files-from-stdin` splits on NUL only (newline-separated input becomes one bogus path).
21. Windows: `files` commands are split on spaces and run without a shell; `sh -c` command line built without escaping `"`; `env` slice has empty leading entries.
22. `extends` duplicate detection rejects diamond includes; non-matching extends paths are silently ignored.
23. Skip `{ref, run}` items are OR'ed inside one map.
24. `stage_fixed` passes file names as pathspecs (glob magic not disabled) and uses `git add --force`.
25. Unknown config keys are silently ignored at runtime (typos never reported unless `validate` is run).

---

## 18. Adopt into snp

### 18.1 How extension keys behave in pre-commit (observed) and the resulting key policy

Observed with pre-commit 4.1.0 (`scratchpad/tmp-lefthook/extkeys`, `pre-commit run --all-files`) on a local hook carrying `stage_fixed`, `priority`, `env`, `x-snp`, a repo-level `x-group`, and root-level `snp`, `x-snp-anchors`:

```
[WARNING] Unexpected key(s) present on local: x-group
[WARNING] Unexpected key(s) present at root: snp, x-snp-anchors
[WARNING] Unexpected key(s) present on local => echo: env, priority, stage_fixed, x-snp
echo.....................................................................Passed
```
exit 0. `pre-commit validate-config` prints only the first two lines (exit 0) — the hook-level warning is emitted at hook instantiation (`pc:pre_commit/hook.py:50-58`, keys sorted, `", "`-joined), root/repo warnings by cfgv `WarnAdditionalKeys` (`pc:pre_commit/clientlib.py:358-373`). **Unknown keys never fail pre-commit**; what *does* fail is changing the type/values of existing keys (e.g. prek's `files: {glob: …}` — `files` must be a regex string; or a non-pre-commit value in `stages`).

Policy for snp:

- **P1 — plain keys.** Extension keys are plain snake_case keys on the hook entry (and a few at root), not namespaced. One pre-commit warning line per hook is the accepted cost; it costs nothing in snp or prek. Where prek already defines a key, reuse prek's **name and semantics** (`priority`, `priorities`, `env`, `groups`), so one config works across snp, prek and (degraded) pre-commit.
- **P2 — `x-` prefix.** Keys starting with `x-` are silently ignored at every level (prek parity, `prek:docs/reference/configuration.md` "Extension keys (`x-`)"). They are useful for YAML anchors.
- **P3 — unknown keys.** For keys that are neither pre-commit keys, snp extension keys, nor `x-*`, snp prints the **byte-identical pre-commit warnings** (same three formats). `snp validate-config` adds "did you mean `<key>`?" hints (Damerau–Levenshtein ≤ 2 against known keys) — only in that command, so `run` output stays identical to pre-commit.
- **P4 — never retype pre-commit keys.** snp-native configs must stay loadable by pre-commit. prek-only forms that break pre-commit are accepted for prek compatibility, and `snp validate-config --portable` lists them along with every extension key pre-commit would ignore.
- **P5 — personal overrides live in a separate overlay file** (§18.4 A3), which pre-commit never reads.

### 18.2 Ranked list

| # | lefthook feature | snp mapping (pre-commit-compatible) | Level | Rationale | Main risk |
|---|---|---|---|---|---|
| A1 | `parallel`, `piped`, `group`, `priority` | hook `priority` (prek semantics: lower first, equal ⇒ concurrent) + `priorities` aliases; bounded pool `-j/--jobs`, `SNP_CONCURRENT_HOOKS`; `piped` ⇒ `fail_fast` | **MUST** | biggest wall-clock win; prek-compatible; default stays sequential | file-modification attribution; nondeterministic output |
| A2 | `env` | hook `env` map (prek) | **MUST** | PATH/RAILS_ENV/RUSTDOCFLAGS needs are universal | `$VAR` expansion differs (lefthook expands) — Q3 |
| A3 | `lefthook-local.yml`, `{cmd}`, `exclude_tags` locally | overlay `.pre-commit-config.local.yaml` keyed by hook id/alias; `{entry}` placeholder | **MUST** | per-developer skip/wrap (docker, dip) without editing the shared file | silent divergence between devs ⇒ always announce overlay use |
| A4 | `stage_fixed` | hook `stage_fixed: true` (pre-commit stage only) with fix→verify | **MUST** | most-requested pre-commit feature (refused upstream) | staging wrong content; interplay with partial staging |
| A5 | guard: `apply --check` before staging, backup stash, worktree-safe patch names | internal guard redesign | **MUST** | safety | — |
| A6 | `LEFTHOOK=0` | `SNP=0`/`SNP=false` checked in shim before exec and in `hook-impl` | **MUST** | tools/CI need a bypass that works for all hook types | misuse (same as `--no-verify`) |
| A7 | `LEFTHOOK_BIN`, embedded installer path, `assert_lefthook_installed` | `SNP_BIN` override → embedded absolute path → `snp` on PATH → **fail closed** (pre-commit parity) | **MUST** | GUI git clients with odd PATH | none |
| S1 | `skip`/`only` (merge, rebase, merge-commit, ref, run) | hook-level and root-level `skip`/`only` | SHOULD | stop hooks during rebases/merges, branch-specific checks | `run:` conditions execute shell; evaluation cost |
| S2 | `tags`, `exclude_tags`, `LEFTHOOK_EXCLUDE`, `--tag`, `--job` | `groups` (prek) + `--group/--no-group` + `SNP_SKIP_GROUPS`; `snp run <id>...` (several ids) | SHOULD | selective runs | none |
| S3 | `timeout` | hook `timeout: "30s"` | SHOULD | a hung hook blocks commits forever | killing a tool mid-write ⇒ partial files (guard restores stash) |
| S4 | `fail_text` | hook `fail_text` | SHOULD | actionable hints | none |
| S5 | `output`, `LEFTHOOK_OUTPUT`, timing summary | `--summary`/`SNP_OUTPUT=summary`; root `output` | SHOULD | timing visibility | must not change default pre-commit output |
| S6 | `root` | hook `root: <dir>` | SHOULD | monorepos without full workspace mode | path rewriting rules; stage_fixed mapping |
| S7 | `use_stdin` | hook `use_stdin: true` | SHOULD | pre-push scripts reading ref lines | memory (cap) |
| S8 | `--files-from-stdin` | `snp run --files-from-stdin[=lines]` (NUL-separated by default) | SHOULD | CI pipelines, `git diff -z` piping | none |
| S9 | `dump`, `validate` (JSON schema) | `snp config dump` (merged incl. overlay), published JSON schema with extensions | SHOULD | debugging layered config; editor support | none |
| S10 | `min_version` | root `minimum_snp_version` | SHOULD | gate configs using new extensions | none |
| S11 | `fail_on_changes: ci/non-ci` | root/hook `fail_on_changes: always\|never\|ci\|non-ci` (default `always` = pre-commit) | SHOULD | `stage_fixed` locally + strict CI | wrong default would break compat |
| S12 | built-in git-lfs passthrough | keep pre-commit legacy chaining; migrator re-installs git-lfs hooks as legacy | SHOULD | migration must not silently drop LFS | none |
| S13 | concurrent git warm-up + per-run caches | internal | SHOULD | startup latency | none |
| C1 | `interactive` | hook `interactive: true` (runs exclusively, TTY passthrough) | COULD | commitizen-style prompts | TTY edge cases; Windows |
| C2 | `follow` | hook `follow: true` / `--follow` (stream output) | COULD | long test suites | interleaving in parallel groups |
| C3 | `extends` (local files) | root `extends: [path]` | COULD | sharing inside a monorepo | precedence confusion |
| C4 | `remotes` (+`refetch`) | root `extends: [{repo, rev, path, sha256?}]` with mandatory pinning, explicit `snp autoupdate` | COULD | org-wide shared config | supply chain; network at hook time |
| C5 | `templates` | overlay `{entry}` only | COULD | wrappers | — |
| C6 | `setup` | none — use an `always_run` hook with the lowest `priority` | WON'T (doc pattern) | duplicates hooks | — |
| C7 | auto-install/checksum sync | warn when config stages have no installed shim; `snp install --sync` | COULD | forgotten `install -t pre-push` | implicit writes to `.git/hooks` |
| C8 | `rc` | env `SNP_RC` sourced by shim | COULD | GUI clients (nvm/rbenv PATH) | sourcing arbitrary scripts |
| C9 | `ai:` agent hooks | `snp install --agent claude` writes `.claude/settings.json` entries calling `snp run --hook-stage manual --group <g>` | COULD | AI-agent workflows | scope creep |
| C10 | glob filters | accept prek's `files/exclude: {glob: …}`; snp-native globset semantics | COULD | readability | breaks pre-commit parsing (P4) |
| C11 | `check-install` | `snp check-install` (exit 0/1) | COULD | CI assertions | none |
| C12 | `self-update` | `snp self update` (sha256 + signature) | COULD | standalone installs | update channel security |
| W1 | gobwas semantics (case-insensitive, `*` crosses `/`) | only inside the migrator | WON'T | surprising | — |
| W2 | unbounded parallelism, completion-order summaries | bounded + config order | WON'T | — | — |
| W3 | template substitution into `sh -c` strings | argv passing only | WON'T | injection (§17 #1) | — |
| W4 | auto-`chmod` of scripts, running every same-named script copy | no | WON'T | surprises | — |
| W5 | fail-open on missing config/binary | pre-commit behavior (fail closed unless `PRE_COMMIT_ALLOW_NO_CONFIG=1`) | WON'T | safety | — |
| W6 | custom color palettes, spinner | no (pre-commit output parity) | WON'T | — | — |

### 18.3 Proposed extension schema (summary)

Hook entry (any repo type; for remote hooks these keys belong to the user's config, never the manifest):

| Key | Type | Default | Validation | Source |
|---|---|---|---|---|
| `priority` | u32 or alias string | implicit (sequential by position) | alias must exist in root `priorities` | prek |
| `env` | map<string,string> | {} | names non-empty, no `=`/NUL | prek |
| `groups` | list<string> | [] | non-empty, no whitespace, not starting with `@` | prek |
| `stage_fixed` | bool | false | warn if hook has no `pre-commit` stage | lefthook |
| `skip` / `only` | condition (§18.4 S1) | — | compiled at load; bad glob ⇒ config error | lefthook |
| `timeout` | duration string (Go-style `1h2m3.5s`, `500ms`) | none | > 0, ≤ 24h | lefthook |
| `fail_text` | string | — | single line recommended | lefthook |
| `root` | relative dir | — | normalized, no `..`, no absolute, must exist at run | lefthook |
| `use_stdin` | bool | false | — | lefthook |
| `interactive` | bool | false | — | lefthook |
| `follow` | bool | false | — | lefthook |
| `fail_on_changes` | `always`/`never`/`ci`/`non-ci` (+ bool aliases `true`/`false`) | root value | — | lefthook |

Root: `minimum_snp_version` (semver), `priorities` (prek), `fail_on_changes` (default `always`), `skip`/`only` (whole-run conditions), `output` (list), and COULD-level `extends`.

### 18.4 Detailed semantics for MUST / SHOULD items

**A1 — Parallel scheduling (`priority`)**
- Default unchanged: hooks without `priority` run strictly sequentially in config order (pre-commit semantics). Implicit priority = position-based, exactly as prek defines it (cross-check with the prek research doc; same config must schedule identically).
- Hooks are bucketed by priority value (ascending). A bucket is a barrier: the next bucket starts only after every hook in the current one finishes. Within a bucket, hooks run concurrently on a shared pool of `N = --jobs | SNP_CONCURRENT_HOOKS | available_parallelism()`. Batches (xargs partitions) of a non-`require_serial` hook draw from the same pool, so the total process count never exceeds N.
- `interactive: true` hooks always form a singleton bucket (exclusive TTY).
- `fail_fast` (root or hook): after a failure, hooks already running in the bucket finish (no kill) and their lines are printed. Later buckets are not started and print nothing. pre-commit simply `break`s out of the hook loop and prints no line for the remaining hooks, so snp matches that.
- Output: one status line per hook **in config order** (head-of-line buffering: print a hook's line and captured output as soon as it and all earlier hooks are done). Byte-identical format to sequential runs.
- Modification detection: pre-commit compares the whole-repo `git diff` before/after *each* hook. Do the same for sequential buckets. For buckets with >1 hook, take a pre-snapshot `(path → mtime_ns, size, ino, mode)` of each hook's input files plus the `git diff` text. After the bucket, a changed file that belongs to exactly one hook's input set is attributed to that hook ("files were modified by this hook"). Changes outside every input set, or in files shared by several hooks, mark **all** hooks in the bucket that could have caused them as failed with the same message (prek documents only group-level failure — snp can do better). Content check: confirm a stat difference with a content hash (`blake3`) to avoid false positives from touch-only tools.
- lefthook `piped: true` maps to `fail_fast: true` on the hooks (converter).

**A2 — `env`**
- Applied to the hook process after language-environment variables. The rule is "user env wins" (prek: "Values override the existing process environment (including variables such as PATH)"). Language backends may still force their own managed variables; document which ones.
- Values are literal strings by default (prek parity). lefthook expands `$VAR` — the migrator resolves this by emitting `sh -c` wrappers or by leaving `${VAR}` if snp adopts opt-in expansion (Q3).
- `env` also applies to `skip`/`only` `run:` conditions of the same hook.

**A3 — Local overlay**
- File: `<dir of main config>/.pre-commit-config.local.yaml` (also `.yml`), or the path in `SNP_LOCAL_CONFIG` (`0`/empty disables). Ignored with `--no-local-config`. Never read by pre-commit. Template output from `snp init` adds it to `.gitignore`.
- Schema (all optional):
  ```yaml
  hooks:                 # overrides, keyed by hook alias, else id
    pytest:
      skip: true         # or any condition
      entry: "docker compose run --rm app {entry}"   # {entry} = merged base entry
      args: ["-x"]       # replaces
      env: {PYTHONWARNINGS: ignore}   # merged, overlay wins
      priority: 10
      stage_fixed: true  # any hook-level key except id/alias/repo
  skip_groups: [slow]    # like lefthook exclude_tags
  fail_on_changes: never
  repos: []              # extra repos appended after the main config's repos
  ```
- An id matching several hooks applies to all of them. An unknown id/alias produces a warning `snp: local config overrides unknown hook '<id>'` (not an error, so branches can differ).
- Every run with an active overlay prints one line on stderr at verbose level, and `snp config dump` marks the overridden fields. Rationale: lefthook users love `lefthook-local.yml`, and pre-commit's only escape hatch is `SKIP=`.

**A4 — `stage_fixed`**
- Active only when the hook runs for the `pre-commit` git stage on the staged file set (git hook, or `snp run` without `--all-files/--files/--from-ref`). Otherwise it is ignored with a debug note. lefthook ties it to the hook name only.
- Per hook H with inputs I (staged, filtered), after H returns `rc`:
  1. `M` = files in I changed by H (stat + hash). `O` = other changed files (whole-diff compare).
  2. If `O ≠ ∅` → normal pre-commit failure (`files were modified by this hook`), nothing staged.
  3. If `M = ∅` → normal result from `rc`.
  4. If `M ≠ ∅` and `rc = 0` → **accepted** (lefthook semantics: success ⇒ stage).
  5. If `M ≠ ∅` and `rc ≠ 0` → **verify**: re-run H once on I (common pre-commit fixers exit 1 after fixing). Verify passes (`rc2 = 0` and no new modifications) → accepted. Otherwise → Failed, with the second run's output shown and nothing staged.
  6. Accepted ⇒ status `Passed`, plus a line `- files were modified by this hook and staged` (Q5). M is queued for staging, except files also modified by any non-`stage_fixed` hook.
- Staging happens once, after all hooks, inside the guard (A5):
  1. Check the unstaged patch with `git apply --check`.
  2. If OK: `git --literal-pathspecs add -- <M…>` (no `--force`; batch by arg length), then restore the unstaged patch.
  3. If not OK: stage nothing, roll back per pre-commit (`Stashed changes conflicted with hook auto-fixes... Rolling back fixes...`), and mark the accepted hooks as failed.
- A run that fails for other reasons still stages accepted fixes (lefthook parity). This makes the next commit attempt cheaper.

**A5 — Guard hardening (on top of pre-commit's `staged_files_only` semantics)**
1. Keep pre-commit's behavior: hide **all** unstaged changes via `git diff-index … <write-tree>`, handle intent-to-add files, keep the patch in the cache dir, and use the same messages.
2. Add an applicability check (`git apply --check`) before any staging. Keep the autocrlf retry.
3. Handle signals. SIGINT/SIGTERM/SIGHUP set a cancel flag, kill hook process groups (SIGTERM, then SIGKILL after 2 s), restore the patch, and exit 130/143/129.
4. On start, detect leftover patch files from a crashed run and print recovery instructions. Never delete them automatically.
5. Linked worktrees: the patch file name must include a worktree-unique component (lefthook #1582).
6. Optional `SNP_STASH_BACKUP=1`: `git stash store -m "snp auto backup"` before hiding, and drop exactly that SHA after restore. Never drop other entries (lefthook §17 #13).
7. All pathspec git calls: `--literal-pathspecs` and `--`, `-z` output, `GIT_OPTIONAL_LOCKS=0` for reads, one mutex for index writers.

**A6 — `SNP=0`**
- The shim starts with `case "${SNP-}" in 0|false) exit 0;; esac`.
- `snp hook-impl` re-checks it (direct invocations) and returns 0 with no output.
- `snp run` (manual) ignores it.
- The variable is documented next to `SKIP`.

**A7 — Shim binary discovery**
1. `exec "$SNP_BIN" hook-impl …` if `SNP_BIN` is set.
2. Otherwise the absolute path of the installing binary, embedded and single-quote-escaped, if it is executable.
3. Otherwise `snp` on `PATH`.
4. Otherwise print `` `snp` not found.  Did you forget to install it? (set SNP_BIN or reinstall hooks with `snp install`)`` and exit 1. This mirrors pre-commit's `` `pre-commit` not found.  Did you forget to activate your virtualenv? ``.
- Use `exec` (saves a process; signals reach snp directly).
- No package-manager probing.
- Pre-commit-compatible legacy-hook handling remains.

**S1 — `skip` / `only`**
- Grammar: lefthook's (§8.1) plus states `cherry-pick` (`CHERRY_PICK_HEAD`), `revert` (`REVERT_HEAD`), `bisect` (`BISECT_LOG`), and items `{env: NAME}` (set and non-empty) and `{ci: true|false}`.
- Inside one map, keys are **AND**ed. lefthook ORs them; the migrator splits such maps into separate list items to preserve OR.
- `ref` patterns are matched case-sensitively against the short branch name (detached HEAD ⇒ `""`), using `globset` with **default options**: `*`/`?` also match `/`, plus `[…]` and `{a,b}`. This is the same as lefthook's gobwas usage and close to `git branch --list <pattern>`, so migrated patterns keep their meaning.
- `run:` is executed via `sh -c` (Windows: Git's sh.exe, §9), with cwd = repo root, stdin null, hook `env` applied, a 30 s timeout, and results cached per distinct command per run.
- Levels:
  - root (applies to every hook, evaluated once)
  - hook
  - per-overlay
- Skipped hooks print pre-commit's `Skipped` line (yellow), same as `SKIP=`.

**S2 — Groups and selection**
- `groups` key, `--group/--require-group/--no-group` (prek names).
- `SNP_SKIP_GROUPS=a,b` (trimmed, comma-split), plus overlay `skip_groups`.
- `snp run id1 id2 …` (pre-commit accepts a single `hook` positional, so extra ids are an extension).
- When an explicit id/group selection matches nothing → error `No hook with id \`<x>\` in stage \`<stage>\``. This is pre-commit's message for a single id; extend it to multiple ids.

**S3 — `timeout`**
- Wall clock for the whole hook (all batches).
- On expiry: SIGTERM to every process group, SIGKILL after 2 s.
- Status `Failed`, with the captured output plus `- hook timed out after <d>`.
- Counts as a failure for `fail_fast`.

**S4 — `fail_text`**: appended to the failure block after `- exit code: N` (or after `- files were modified by this hook`) as `- <fail_text>`. It is printed only on failure.

**S5 — Summary/timings**
- Default output stays byte-identical to pre-commit.
- `--summary` / `SNP_OUTPUT=summary` adds a footer: total wall time, per-hook durations sorted by config order, and a count of passed/failed/skipped.
- `SNP_OUTPUT=quiet` prints only failures.
- Env overrides config. Unknown values warn once and are ignored individually (fixing lefthook §17 #19).

**S6 — `root`**
- File selection happens first, using the normal `files`/`exclude`/`types` on repo-relative paths (lefthook parity). Then keep only paths under `root/` (boundary-aware).
- The hook's cwd is `<repo>/<root>`, and filenames are passed relative to root (no `./` prefix; pre-commit passes bare relative paths).
- `stage_fixed` maps paths back.
- `language` environments are unaffected.
- A missing root dir → hook `Failed` with `- root directory '<root>' does not exist`.

**S7 — `use_stdin`**
- At hook-impl start, snp reads the git hook's stdin fully, capped at 64 MiB; beyond that it errors.
- pre-push parsing uses the same bytes.
- Every `use_stdin` hook (and each of its batches) gets its own pipe fed by a writer thread. Others get a closed stdin (pre-commit parity).

**S8 — `--files-from-stdin`**: NUL-separated by default (`git diff -z` friendly; one trailing NUL ignored). `--files-from-stdin=lines` reads newline-separated input. Paths are made repo-relative, de-duplicated, and keep their order. The input set is mutually exclusive with `--all-files/--files/--from-ref` (error).

**S11 — `fail_on_changes`**:
- `always` (default): pre-commit behavior.
- `never`: lefthook's default. Modifications don't fail the hook and stay unstaged.
- `ci` / `non-ci`: `always` iff `CI` is set (non-empty, not `0`/`false`) / unset. lefthook checks existence only; snp uses enabled semantics (Q8).
- The setting combines with `stage_fixed` (fixes staged locally, failed in CI).

**S12 — LFS**:
- `snp install` keeps pre-commit's legacy chaining (an existing git-lfs hook is moved to `<hook>.legacy` and runs first).
- `snp migrate lefthook` detects LFS by any of: `filter=lfs` in `.gitattributes`, `.lfsconfig`, `.lfs-required`. When `--install` is given, it runs `git lfs update --force` (re-creates git-lfs's own `post-checkout`/`post-commit`/`post-merge`/`pre-push` hooks over the removed lefthook shims) and then `snp install`, which moves them to `.legacy` and chains them. Without `--install` it prints those two commands.
- Native passthrough (lefthook-style) is COULD.

---

## 19. `snp migrate lefthook` — converter specification

### 19.1 CLI

```
snp migrate lefthook [--config <path>] [--output <path>|-] [--mode snp|portable]
                     [--local] [--fetch-remotes] [--install] [--force] [--strict]
```

| Flag | Default | Meaning |
|---|---|---|
| `--config` | lefthook lookup (§2.1, honoring `LEFTHOOK_CONFIG`) | source config |
| `--output` | `.pre-commit-config.yaml` in repo root; `-` = stdout | refuses to overwrite an existing file without `--force` |
| `--mode snp` | default | emit snp extension keys (§18.3); pre-commit will run the file with "Unexpected key(s)" warnings |
| `--mode portable` | — | emit only pre-commit 4.x keys. Emulate env/root/files via `sh -c` wrappers, drop the rest with comments |
| `--local` | off | also convert `lefthook-local.*` into `.pre-commit-config.local.yaml` (§19.9) |
| `--fetch-remotes` | off | shallow-clone `remotes` into a temp dir (lefthook's clone/fetch commands, §2.4) and inline them. Default: one `# snp-migrate: TODO remote …` comment per remote |
| `--install` | off | after writing: remove lefthook shims (files in hooks dir containing `LEFTHOOK`; restore `<hook>.old` if present), delete `.git/info/lefthook.checksum` and `.git/info/lefthook-remotes/`, run the LFS fix (§18.4 S12), then `snp install -t <each emitted stage>` |
| `--strict` | off | exit 2 if any feature could not be converted faithfully |

Exit codes: 0 = written (warnings allowed); 1 = parse/IO error, nothing written; 2 = `--strict` and lossy.

### 19.2 Pipeline

1. Locate and parse the main config. Parsing details:
   - YAML 1.2 boolean rules: only `true/false` are booleans, matching go-yaml v3.
   - Supported formats: TOML, JSON, JSONC (comments + trailing commas).
   - Error messages carry `path:line:col`.
2. Resolve `extends` exactly per §2.3/§2.5:
   - relative to the repo root
   - recursive, with nested files overriding their parent
   - glob = `filepath.Match` semantics
   - the duplicate-path error is reproduced as a warning, not fatal
   - `remotes` only with `--fetch-remotes`
   - the local config only with `--local`, and into the overlay
3. Apply weak typing (§2.2): scalar→list for list fields, number→string, `"true"`→bool, and Go-duration strings kept verbatim for `timeout`.
4. For each hook in `AvailableHooks` order, then custom hooks alphabetically:
   1. build the job list per §6.1 (jobs, then commands sorted, then scripts sorted);
   2. flatten groups into scheduling buckets (§19.4);
   3. convert each leaf (§19.3, §19.5);
   4. evaluate inherited scope per §6.3 (glob concat, exclude append, env merge, root/files override, tags concat).
5. Emit a YAML header comment, `default_install_hook_types` (the pre-commit hook types used, in pre-commit's `HOOK_TYPES` order), optional root keys, and a single `repo: local` with all hooks. Emit `minimum_pre_commit_version: '3.2.0'` only in portable mode when stage names require it (modern names `pre-commit`, `pre-push` need ≥ 3.2.0).
6. Print the report (§19.10).

### 19.3 Field mapping

| lefthook | Output | Notes |
|---|---|---|
| hook name ∈ pre-commit `HOOK_TYPES` (`commit-msg post-checkout post-commit post-merge post-rewrite pre-commit pre-merge-commit pre-push pre-rebase prepare-commit-msg`) | `stages: [<hook>]` | added to `default_install_hook_types` |
| custom hook name (`test`, `lint`…) | `stages: [manual]` + `groups: [<hook>]` (snp) | invoke with `snp run --hook-stage manual --group <hook>`. Portable mode: `stages: [manual]`, with the group noted in `name` |
| other git hooks (`applypatch-msg pre-applypatch post-applypatch pre-receive update proc-receive post-receive post-update reference-transaction push-to-checkout pre-auto-gc sendemail-validate fsmonitor-watchman p4-* post-index-change`) | as custom + warning `git hook '<h>' is not supported by pre-commit; converted to manual stage` | Q6 |
| job `name` / command key / script file name | `id: <slug>`, `name: <printable name>` | slug = lowercase ASCII alnum runs joined by `-`; collisions within a stage get `-2`, `-3`, …; an empty slug becomes `<hook>-job-<n>`. Unnamed jobs: `id: <hook>-job-<n>` (n = 1-based position), `name` = `run` (truncated to 60 chars + `…`) |
| (all) | `language: system` | accepted by pre-commit 2.x–4.x (4.4+ aliases it to `unsupported`) and prek |
| `run` | `entry`, `args`, `pass_filenames`, `always_run`, `require_serial` per §19.5 | — |
| `script` + `runner` | `entry: <runner> <source_dir>/<hook>/<script>` (repo-relative; first existing source dir per §7.4) | when the script exists in several source dirs, warn: lefthook ran all copies |
| `script` without `runner` | `language: script`, `entry: <source_dir>/<hook>/<script>` | warn if not executable (lefthook chmods it; pre-commit fails) |
| script `args` | templates → §19.5 wrapper; else `args: <shlex split>` | with no `args`, lefthook appended raw git args. pre-commit passes the message file to commit-msg/prepare-commit-msg hooks itself; other stages lose positional args (warning) |
| `glob` (+ inherited) | `files: '<regex>'` (§19.6) | omitted when empty |
| `exclude` (hook + ancestors + job) | `exclude: '<regex>'` | case-sensitive |
| `root` | snp: `root: <dir>`; portable: wrapper (§19.5 W-root) + `files` constrained to `^<root>/` | the lefthook prefix bug (no `/` boundary) is fixed; noted in the report |
| `file_types` | `types` / `types_or` / `exclude_types` (§19.7) | — |
| `files` (job/group/hook) | §19.5 rule R5 | — |
| `env` (+ inherited) | snp: `env:`; portable: wrapper `NAME="<value>"` assignments before `exec` | lefthook `$VAR` expansion is preserved by emitting values inside double quotes in the wrapper. snp mode with literal env (Q3): values containing `$` are emitted as a wrapper instead |
| `tags` (+ inherited) | `groups:` (snp) | portable: dropped |
| `skip` / `only` (hook + group + job) | snp `skip`/`only` lists; a hook-level condition is copied onto every job of that hook | `{ref, run}` maps are split into two list items (lefthook ORs them, snp ANDs). Portable: dropped + warning |
| `fail_text` | snp `fail_text` | — |
| `timeout` | snp `timeout` (same Go-duration string) | — |
| `stage_fixed` | snp `stage_fixed: true` (only on `pre-commit` stage jobs; elsewhere dropped with a note) | — |
| `interactive` | snp `interactive: true` + `require_serial: true` | portable: dropped + warning |
| `use_stdin` | snp `use_stdin: true` | portable: dropped + warning |
| ordering, `priority`, `parallel`, `piped`, `group` | order of emission + snp `priority` + `fail_fast` (§19.4) | — |
| hook `follow` | `verbose: true` on every job (+ snp `follow: true`) | — |
| hook `setup` | warning. Optional `--setup-as-hook` emits a first hook `id: <hook>-setup`, `entry: sh -c '<s1>; <s2>; true' --`, `always_run: true`, `pass_filenames: false`, lowest priority | lefthook only warns when setup fails, hence `; true` |
| hook `fail_on_changes` / default | §19.8 | — |
| hook `exclude_tags` | snp: overlay `skip_groups` or a comment | — |
| `templates` | inlined at conversion time (raw, like lefthook) | — |
| `min_version`, `colors`, `no_tty`, `output`, `assert_lefthook_installed`, `no_auto_install`, `skip_lfs`, `install_non_git_hooks`, `glob_matcher` (consumed by §19.6), `lefthook`, `rc`, `ai`, `$schema`, `source_dir*` (consumed), `remotes` | dropped | listed in the report. `rc` → suggest `SNP_RC`. `ai` → suggest C9. `skip_lfs: false` with LFS detected → S12 |

### 19.4 Ordering and parallelism → buckets

Model each lefthook hook as a tree: `Seq(children, piped)` for a non-parallel hook or group, `Par(children)` for a parallel one, and `Leaf(job)`. Compute bucket indices with:

```
schedule(Leaf, b)      -> assign bucket b to the leaf; return b + 1
schedule(Seq(cs), b)   -> for c in cs: b = schedule(c, b); return b
schedule(Par(cs), b)   -> return max(schedule(c, b) for c in cs)   // all start at b
```

- Emit hooks per stage sorted by `(bucket, original DFS order)`.
- If the stage contains no `Par` node, omit `priority` entirely (implicit sequential order = same semantics). Otherwise emit `priority: <bucket * 10>` on **every** hook of that stage (explicit everywhere avoids mixing implicit and explicit priorities).
- This is conservative: it never runs anything concurrently that lefthook ran sequentially, and it keeps group-internal order. Example: `Par(Seq(A, B), C)` becomes buckets A=0, B=1, C=0, so C overlaps A but not B. lefthook would overlap C with both; report `parallelism reduced`.
- `piped: true` (hook or group) → `fail_fast: true` on each member. When the piped node is not the root, warn: `pre-commit fail_fast stops the whole run, lefthook piped only stopped its group`.
- Commands/scripts `priority` semantics (0 = last) are already applied in §6.1 ordering before bucketing.
- Interactive leaves keep their position. snp makes them exclusive at run time (§18.4 A1). lefthook's docs claim they move to the end, but the code doesn't, so keep the position.

### 19.5 `run` → `entry` rules

Definitions:
- **main template** of a stage: `{staged_files}` for `pre-commit`, `{push_files}` for `pre-push`. When the job uses `{all_files}` or `{files}` see R4/R5. Other stages have no main template.
- **simple** command: after removing the template, it contains none of ``| & ; < > ( ) $ ` \ " ' * ? [ ] # ~ { }``, no newline, and no leading `NAME=` assignment. Simple commands are emitted as `entry` words (pre-commit `shlex.split`s `entry`).
- **wrapper** `W(script)` = `sh -c '<script>' --`, emitted with POSIX single-quote escaping (`'` → `'\''`) so pre-commit's `shlex.split` yields `["sh", "-c", script, "--"]`. Filenames then become `"$@"` (and `$0 = --`).
- Template substitutions inside a wrapper script:
  - main template (bare, `"…"`, or `'…'`) → `"$@"`
  - `{0}` → see table below, space-joined
  - `{N}` → see table below
  - custom templates → raw text
  - `{lefthook_job_name}` → POSIX-quoted name

Rules, first match wins:

| Rule | Condition | Output |
|---|---|---|
| R1 | no templates; simple | `entry: <run>`, `pass_filenames: false`; `always_run: true` if the stage ∈ {`post-checkout`, `post-commit`, `post-merge`, `post-rewrite`, `pre-rebase`, `manual`/custom} (pre-commit passes no files there, `pc:pre_commit/commands/run.py:253-260`) |
| R1' | no templates; not simple | `entry: W(run)`, rest as R1 |
| R2 | exactly one main-template occurrence (any quoting) at the very end, prefix simple, no other templates | `entry: <prefix>`, `pass_filenames: true`, `require_serial: true` (lefthook made one call per command-line-sized chunk; pre-commit would otherwise parallelize chunks across CPUs) |
| R3 | main template elsewhere / repeated / prefix not simple / other templates present | `entry: W(script)`, `pass_filenames: true`, `require_serial: true` |
| R4 | `{all_files}` | snp mode: COULD-ext `file_source: all` (Q9) + R2/R3 treating `{all_files}` as the main template + `always_run: true`. Portable: `entry: W("git ls-files -z \| xargs -0 <prefix>")` (or the R3 script fed via `xargs -0 sh -c '<script>' --`), `pass_filenames: false`, `always_run: true`, warning `glob/exclude no longer filter {all_files}` |
| R5 | `{files}` and/or job/hook `files:` command | `pass_filenames: false`, `always_run: true` (lefthook ran the files command on every hook invocation), `entry: W("set -f; IFS='<newline>'; set -- $(<files cmd> <filter>); [ $# -gt 0 ] \|\| exit 0; <script with {files}→\"$@\">")`. `<filter>` = `\| grep -iE '<ERE of glob>'` and `\| grep -vE '<ERE of exclude>'` when present (§19.6 ERE form). Root filtering via `\| grep '^<root>/'` + `cd`. A `files` command without `{files}` in `run` still gates execution (§7.4 step 4): the same prelude, then `<run>`. Report: `best-effort: file list computed in shell` |
| R6 | positional git args | per table below; inside a wrapper |
| R7 | `--mode portable` and `env`/`root` present | prefix the wrapper script with `cd '<root>' \|\| exit 1;` (and rewrite `"$@"` paths: `for f; do set -- "$@" "${f#<root>/}"; shift; done` before the command), and `NAME="<value>"; export NAME;` for each env var |

Positional git arguments → pre-commit equivalents (`pc:pre_commit/commands/hook_impl.py:214-236`, `pc:pre_commit/commands/run.py:370-417`):

| Stage | lefthook `{1}` | `{2}` | `{3}` |
|---|---|---|---|
| `commit-msg` | `"$1"` (message file is passed as the only filename ⇒ `pass_filenames: true`) | — | — |
| `prepare-commit-msg` | `"$1"` (filename, `pass_filenames: true`) | `"$PRE_COMMIT_COMMIT_MSG_SOURCE"` | `"$PRE_COMMIT_COMMIT_OBJECT_NAME"` |
| `pre-push` | `"$PRE_COMMIT_REMOTE_NAME"` | `"$PRE_COMMIT_REMOTE_URL"` | — |
| `post-checkout` | `"$PRE_COMMIT_FROM_REF"` | `"$PRE_COMMIT_TO_REF"` | `"$PRE_COMMIT_CHECKOUT_TYPE"` |
| `post-merge` | `"$PRE_COMMIT_IS_SQUASH_MERGE"` | — | — |
| `post-rewrite` | `"$PRE_COMMIT_REWRITE_COMMAND"` | — | — |
| `pre-rebase` | `"$PRE_COMMIT_PRE_REBASE_UPSTREAM"` | `"$PRE_COMMIT_PRE_REBASE_BRANCH"` | — |
| other | warning `no pre-commit equivalent for {N}`; replaced by empty string | | |

`{0}` = the stage's available values space-joined, in order. Note that lefthook substituted these **unquoted**; the converter quotes them. That intentionally changes behavior for values with spaces.

### 19.6 Glob translation

gobwas (default `glob_matcher`) → Python-`re`-compatible regex body (also valid for Rust `regex`):

| gobwas token | regex |
|---|---|
| `\x` | escaped `x` |
| run of `*` (one or more) | `.*` (no separator ⇒ crosses `/`) |
| `?` | `.` |
| `[abc]`, `[a-z]` | same class; regex-special chars inside escaped |
| `[!abc]` | `[^abc]` |
| `{a,b,…}` (nestable) | `(?:a\|b\|…)` |
| any other char | `re.escape` |

- `glob` list → `files: '^(?i:(?:<g1>)|(?:<g2>))$'`. Use a scoped `(?i:…)` because lefthook lowercases both sides, and Python ≥ 3.11 rejects global inline flags not at the start.
- `exclude` list → `exclude: '^(?:<e1>|<e2>)$'` (case-sensitive).
- doublestar (`glob_matcher: doublestar`):
  - `**/` → `(?:.*/)?`
  - a trailing `/**` → `(?:/.*)?`
  - a standalone `**` → `.*`
  - `*` → `[^/]*`
  - `?` → `[^/]`
  - `[!…]`/`[^…]` → `[^…]`
  - braces as above
  - glob is still case-insensitive
- ERE form (for R5 shell filters): the same mapping with `(a|b)` instead of `(?:a|b)`; case-insensitivity via `grep -i`.
- Root in portable mode with a glob: `files: '^(?=<root>/)(?i:…)$'` (lookahead — Python only). In snp mode, use the `root` key instead, so no lookahead is needed.
- Test vectors (must pass, from `filter/filter_test.go`; `skip_checker_test.go` for refs):
  - `*.rb` ⇒ {`folder/subfolder/0.rb` ✓, `2.RB` ✓, `1.txt` ✗}
  - `**/*.rb` ⇒ {`0.rb` ✗, `folder/1.rb` ✓}
  - `*.rb?` ⇒ {`1.rBs` ✓, `2.rbv` ✓, `folder/0.rb` ✗}
  - `*.{a,b,cn}` ⇒ {`f.a`, `f.b`, `f.cn` ✓; `f.c` ✗}
  - doublestar `**/*.rb` ⇒ {`0.rb`, `folder/1.rb`, `folder/subfolder/2.rb` ✓}
  - exclude `*.txt` removes `dir/a.txt`; exclude `a.txt` keeps `dir/a.txt`
  - property test: translator regex ≡ a reference gobwas-semantics matcher on random paths/patterns

### 19.7 `file_types` → identify tags (file-identify crate)

| lefthook | Output |
|---|---|
| `text` | `types: [text]` |
| `binary` | `types: [binary]` |
| `executable` | `types: [executable]` |
| `not executable` | `exclude_types: [executable]` |
| `symlink` | `types: [symlink]` |
| `not symlink` | `exclude_types: [symlink]` |
| `text/x-python` | `types_or: [python]` |
| `text/x-sh`, `text/x-shellscript` | `types_or: [shell]` |
| `text/x-perl` / `text/x-lua` / `text/x-php` | `types_or: [perl]` / `[lua]` / `[php]` |
| `text/html` / `text/xml` / `text/javascript` / `application/json` | `types_or: [html]` / `[xml]` / `[javascript]` / `[json]` |
| other MIME | warning `no identify tag for MIME '<m>'`, dropped |

- Combination semantics match: lefthook ANDs simple types and ORs MIME types, and pre-commit has `types` (all), `types_or` (any), and `exclude_types` (none).
- Detection differs slightly. identify classifies by extension, then shebang, then a content heuristic, while lefthook content-sniffs with mimetype. The report notes `file type detection differs (identify vs mimetype)`.

### 19.8 `fail_on_changes` defaults

- lefthook's default is `never`: a job that rewrites files passes and the fixes stay unstaged. pre-commit/snp default `always`.
- To preserve behavior, snp mode emits a root `fail_on_changes: never` when every lefthook hook used the default or `never`. Otherwise it emits per-hook values mapped from each lefthook hook:
  - `true`/`1`/`always` → `always`
  - `ci` → `ci`
  - `non-ci` → `non-ci`
  - unset/`never`/`false`/`0` → `never`
- The report recommends removing the key (pre-commit semantics) and adding `stage_fixed: true` to fixers. The fixer heuristic looks for `--fix`, `--write`, `-w`, `fmt`, `format`, `-i`, `--autofix`.
- Portable mode cannot express `never`. It warns `pre-commit fails hooks that modify files; lefthook did not`.

### 19.9 Local config → overlay (`--local`)

- Job present in main (same name) → `hooks: {<id>: {<changed keys only>}}`.
- `{cmd}` → `{entry}` when the main job mapped to rule R1/R2. Otherwise the converter emits the fully expanded `entry` wrapper.
- `skip: true` → `skip: true`.
- Changed `env` → `env` (overlay map-merges).
- Jobs only in local → overlay `repos: [{repo: local, hooks: [...]}]`.
- `exclude_tags` → `skip_groups`.
- `source_dir_local` scripts → repo-relative `entry` paths.
- Local `templates` are inlined into the affected entries.
- Write `.pre-commit-config.local.yaml` and suggest adding it to `.gitignore`.

### 19.10 Report format

Each issue is printed to stderr as `snp-migrate: <level>: <hook>[/<job>]: <message>`, where level is `warning` or `lossy`. The same text goes as a `# snp-migrate: …` comment above the affected hook. The final line is:

`Converted <J> jobs from <H> lefthook hooks into <N> hooks (<W> warnings, <L> lossy conversions).`

### 19.11 Worked example

Input `lefthook.yml`:

```yaml
pre-commit:
  parallel: true
  jobs:
    - name: eslint
      glob: "*.{js,ts}"
      run: yarn eslint --fix {staged_files}
      stage_fixed: true
    - name: rubocop
      root: backend/
      glob: "*.rb"
      exclude: [config/routes.rb]
      run: bundle exec rubocop -A --force-exclusion -- {staged_files}
      env: {RUBOCOP_OPTS: "--parallel"}
      tags: [backend, style]
commit-msg:
  commands:
    lint-msg:
      run: npx commitlint --edit {1}
      skip: [merge, rebase]
pre-push:
  commands:
    test:
      run: yarn test
      fail_text: "Run `yarn test` locally"
      timeout: 5m
```

Output (`--mode snp`):

```yaml
# Generated by `snp migrate lefthook` from lefthook.yml
fail_on_changes: never   # snp-migrate: lefthook default; pre-commit semantics would fail modified files
default_install_hook_types: [commit-msg, pre-commit, pre-push]
repos:
  - repo: local
    hooks:
      - id: eslint
        name: eslint
        language: system
        entry: yarn eslint --fix
        files: '^(?i:(?:.*\.(?:js|ts)))$'
        stages: [pre-commit]
        require_serial: true
        priority: 0
        stage_fixed: true
      # snp-migrate: warning: pre-commit/rubocop: exclude 'config/routes.rb' is matched from the repo root (lefthook semantics) and never matches under root 'backend/'
      - id: rubocop
        name: rubocop
        language: system
        entry: bundle exec rubocop -A --force-exclusion --
        files: '^(?i:(?:.*\.rb))$'
        exclude: '^(?:config/routes\.rb)$'
        stages: [pre-commit]
        require_serial: true
        priority: 0
        root: backend
        env: {RUBOCOP_OPTS: --parallel}
        groups: [backend, style]
      - id: lint-msg
        name: lint-msg
        language: system
        entry: npx commitlint --edit
        stages: [commit-msg]
        require_serial: true
        skip: [merge, rebase]
      - id: test
        name: test
        language: system
        entry: yarn test
        pass_filenames: false
        stages: [pre-push]
        fail_text: Run `yarn test` locally
        timeout: 5m
```

Notes:
- `lint-msg` uses rule R2 with the `commit-msg` message file as the single filename. `pass_filenames` defaults to true, so it is omitted.
- `test` has no template. In pre-push, lefthook skipped it when no files were pushed, and pre-commit likewise skips hooks without `always_run` when the push diff is empty, so behavior matches.
- `priority: 0` appears because the pre-commit stage contains a `Par` node.

---

## 20. Implementation notes for snp (Rust 1.98, edition 2024)

**Config model and validation**
1. Parse into a span-preserving tree first, then into typed structs.
   - The YAML parser choice is cross-cutting (see the parser research). pre-commit itself uses PyYAML/YAML 1.1. The lefthook migrator must use **YAML 1.2 booleans** (only `true`/`false`) to mirror go-yaml v3.
   - Unknown keys go into an `extra: BTreeMap<String, Spanned<Value>>` per level so snp can emit pre-commit-identical warnings (§18.1) with `file:line:col` in `validate-config`.
2. Type the extension keys in a dedicated `SnpHookExt` struct flattened into the hook (`#[serde(flatten)]` is slow and loses spans. Prefer a hand-written visitor or a two-pass "known keys" partition).
   - Compile everything at load time (regexes, globs, `skip`/`only` conditions, durations) and return `ConfigError`s with spans.
   - **No `MustCompile`-style panics** (§17 #2).
3. Durations: write a small parser compatible with Go `time.ParseDuration`. Grammar `[+-]?(\d*\.?\d+(ns|us|µs|μs|ms|s|m|h))+`; reject negatives and zero for `timeout`. Don't depend on `humantime` semantics, which differ (`"1h30m"` vs `"1h 30m"`, `"1y"`).
4. For the migrator, implement weak typing explicitly:
   - `fn str_list(v) -> Vec<String>`: string → `[s]`, number/bool → `[to_string]`, list → elementwise
   - `fn weak_bool`
   - `fn weak_string`

**Matching**
5. Python-`re` compatibility for `files`/`exclude` belongs to the regex research.
   - Generated migrator regexes stay within the common subset: `(?i:…)`, `(?:…)`, classes, `.*`, anchors. Lookahead is portable-mode only.
   - Unit-test the gobwas→regex translator with the §19.6 vectors plus a property test against a reference gobwas-semantics matcher (write it in the test module: `*`/`**` = any sequence, `?` = any char, braces, classes).
6. `ref` patterns: `globset::GlobBuilder::new(p).literal_separator(false)` (the default), case-sensitive. Build once per config.

**Process execution**
7. Spawning: `std::process::Command`.
   - Unix: `CommandExt::process_group(0)` (safe, stable) for every hook process. Kill groups with `rustix::process::kill_process_group(pgid, Signal::Term)`, then `Kill` after a grace period. This needs no `unsafe` in snp code. Keep `#![forbid(unsafe_code)]` in every snp crate (dependencies such as rustix/portable-pty may use unsafe internally).
   - PTY (color passthrough, pre-commit parity when `--color=always`): `portable-pty` on unix only. lefthook's PTY-only-the-leader kill problem is avoided by putting the PTY child in its own session/group and killing the group.
8. Concurrency: a bounded worker pool, either:
   - `std::thread::scope` plus a counting semaphore (`std::sync::Condvar`-based), or
   - a tokio `current_thread`/`multi_thread` runtime with `JoinSet` + `Semaphore`.
   Hook processes are blocking I/O, so threads are fine and keep startup minimal. Every xargs batch acquires a permit, so `N` bounds total processes (lefthook is unbounded).
9. Stdin: in `hook-impl`, `read_to_end` with a 64 MiB cap into `Arc<[u8]>`. Each `use_stdin` child gets its own `ChildStdin` fed by a writer thread (avoids deadlock when the child doesn't read). Other children get `Stdio::null()` — pre-commit uses a closed pipe, which is equivalent EOF behavior.
10. Output capture: a per-hook bounded buffer (e.g. 8 MiB in memory, then spill to `tempfile`), flushed in config order. lefthook's unbounded `bytes.Buffer` is the anti-pattern here.
11. Signals: `signal-hook` (or `tokio::signal`) for SIGINT/SIGTERM/SIGHUP → cancellation token → kill groups → guard restore → exit `128 + signo`.
    - The guard is an RAII value whose explicit `restore()` is called on every path.
    - `Drop` is a best-effort fallback that logs. Don't rely on `Drop` during panics; use `panic = "abort"` only if restore is done before.
    - Leftover-patch detection runs on the next start.

**Git interaction**
12. Use the **git CLI** for index-mutating steps (`apply`, `checkout`, `add`, `stash`) for fidelity with user hooks/filters/LFS smudge.
    - Read-only queries (`diff --name-only -z`, `ls-files -z`, `status --porcelain=v1 -z`) may run concurrently with `GIT_OPTIONAL_LOCKS=0`.
    - Hold a single `Mutex<()>` only around writers (lefthook serializes everything).
    - Consider `gix` for read-only listing later, but only behind parity tests.
13. Always use `-z` and byte paths (`bstr::BString`/`Vec<u8>`; `OsString` on the boundary). Never `TrimSpace` paths or unquote C-quoted output (§7.1). Pass `--literal-pathspecs` (or `GIT_LITERAL_PATHSPECS=1`) plus `--` for every pathspec git call. A file named `[file].js` must not glob (§10.3).
14. Warm-up: at `hook-impl` start, spawn the staged-files, status, and `write-tree`/diff-index queries in parallel with config parsing (lefthook `Cache.WarmUp`). Join before scheduling.
15. Command-line limits: compute like pre-commit's xargs (ARG_MAX via `rustix`/`libc` sysconf minus env size; Windows 32 767 UTF-16 units). Do not use lefthook's fixed 130 000/260 000/7 000 constants. Batch every multi-path git call by the same budget (lefthook bug §17 #12: never reuse one `--output` file across batches).
16. Modification attribution (A1): stat snapshots as `(dev, ino, size, mtime_ns, ctime_ns, mode)` from `std::fs::symlink_metadata` + `std::os::unix::fs::MetadataExt`. Confirm with `blake3` content hashes only for changed stats. Fall back to pre-commit's whole `git diff --no-ext-diff --no-textconv --ignore-submodules` byte comparison for sequential buckets, which is exactly pre-commit's algorithm (`pc:pre_commit/commands/run.py:274-279`, `:290-296`).

**Shim and install**
17. Shim template. pre-commit's shim (`pc:pre_commit/resources/hook-tmpl`) is **bash** (`#!/usr/bin/env bash`, arrays `ARGS=(hook-impl --config=… --hook-type=…)`, then `--hook-dir "$HERE" -- "$@"`, `exec`), with fallback `command -v pre-commit`. lefthook's is POSIX `/bin/sh`.
    - Prefer POSIX sh for snp: Alpine/BusyBox images often lack bash, and Git for Windows ships sh. Keep pre-commit's argument shape (`hook-impl --config=<cfg> --hook-type=<type> --hook-dir "$HERE" -- "$@"`) and its identifying header comments, so pre-commit-aware tooling and `pre-commit uninstall` keep recognizing it. The final choice belongs to the install research doc.
    - Order of checks: `SNP=0`, then `SNP_BIN`, then the embedded single-quote-escaped absolute path, then PATH `snp`, then the fail-closed message.
    - `set -e`-free. Escape all embedded strings with a single tested `sh_quote()` (lefthook interpolates `rc`/roots raw, §14.2).
18. Lefthook interop on install:
    - Recognize lefthook shims (any line containing `LEFTHOOK`; also `call_lefthook`) as **foreign managers**.
    - Default `snp install` treats them like any foreign hook (move to `.legacy`, pre-commit behavior), which would run lefthook too.
    - `snp migrate lefthook --install` removes them instead (§19.1).
    - Warn when `.git/info/lefthook.checksum` exists: `lefthook appears to be installed in this repository; run 'snp migrate lefthook' or 'lefthook uninstall'`.
19. Config change detection (C7, optional): hash (xxh3/blake3) the raw bytes of main config + overlay + local extends, not the mtime-only fast path (§17 #10).

**Errors and messages**
20. One `thiserror` enum per subsystem. All user-facing strings live in one module (`messages.rs`) with `const`/`format!` helpers so tests can assert exact text. Keep pre-commit strings byte-identical, and give snp-only messages a stable prefix (`snp:`).
21. Never interpolate file names or git args into shell strings. When a wrapper is unavoidable (`sh -c` in migrated configs), pass data as positional parameters after `--` (§19.5).

**Testing** (see `10-test-strategy.md`)
22. Port the relevant lefthook integration fixtures as snp scenario tests:
    - `restore_unstaged.txt`, `restore_unstaged_on_conflict*.txt` (no patch/stash leftovers)
    - `stage_fixed.txt` / `stage_fixed_505.txt` (`[file].js` names)
    - `worktree_partially_staged_issue_1580.txt`
    - `run_interrupt.txt` (SIGINT restores), plus a SIGTERM variant lefthook fails
    - `fail_on_changes_recover_previous_change.txt`
    - `timeout.txt`
    - `files_from_stdin.txt`
    - `cli_run_only.txt` (→ groups)
    - `skip_merge_commit.txt`, `skip_run.txt`
    - for the migrator: `job_merging.txt` and `many_extends_levels.txt` as golden inputs.
23. Migrator golden tests: input lefthook config → expected `.pre-commit-config.yaml`. Then execute the output with the real pre-commit 4.1.0/4.6.2 oracle in a temp repo (portable mode must run with zero errors; snp mode must run with only "Unexpected key(s)" warnings).

---

## 21. Open questions

1. **Extension key naming.** Plain keys (P1, prek-style) produce one pre-commit warning per hook per run. Alternative: a single namespaced key (`snp: {stage_fixed: true, …}`) — one warning per hook either way, but uglier. Plain keys are recommended; needs a project decision. Should snp also accept `x-snp-*` spellings to silence pre-commit? (pre-commit warns on `x-` keys too, so no gain.)
2. **Overlay file name/location**: `.pre-commit-config.local.yaml` (proposed) vs `snp-local.yaml` vs a user-level `~/.config/snp/overrides.yaml` keyed by repo. Should the overlay apply in CI (`CI` set)? Proposed: yes, but announced.
3. **`env` value expansion**: lefthook expands `$VAR` (`os.ExpandEnv`). prek docs don't mention expansion (check prek source in the prek research doc). Options: literal (prek parity, proposed default), opt-in `${VAR}` expansion, or always expand. Must match prek if prek users share configs.
4. **Implicit priority rule** for hooks without `priority` when others have one. This must be identical to prek's scheduler; take it from the prek research doc. The migrator avoids the question by emitting explicit priorities for whole stages.
5. **`stage_fixed` UX**: is `Passed` + an extra `- files were modified by this hook and staged` line acceptable, or should snp print a distinct status word (e.g. `Fixed`) that breaks byte-parity of output only for extension users? Should `pre-merge-commit` be eligible too? Is the "verify re-run when rc≠0" cost acceptable for slow fixers? Possibly configurable: `stage_fixed: verify|trust`.
6. **Non-pre-commit git hooks** (`reference-transaction`, `pre-auto-gc`, `post-index-change`, `sendemail-validate`, `pre-receive`, `p4-*`, `applypatch-msg`…): pre-commit hard-rejects them in `stages`. Should snp support them via an extension key (e.g. `snp_stages: [reference-transaction]`) and `snp install -t <any>`? Proposed: not in v1; the migrator maps them to `manual` + group.
7. **Remote/shared config (`extends` with `repo`/`rev`)**: worth the supply-chain risk? If yes: mandatory full-SHA pinning or `sha256` of the file, fetched only by `snp install`/`autoupdate`, never during hook execution. Precedence also needs a decision: lefthook lets extends override the main file. Proposed for snp: the main file overrides extends.
8. **`CI` detection** for `fail_on_changes: ci`: lefthook uses existence (`CI=` empty or `CI=false` count as CI). Proposed: enabled semantics (non-empty, not `0`/`false`) — but migrating users might rely on existence.
9. **`{all_files}` / `{files}` mapping**: introduce snp extensions `file_source: all` and `files_cmd: <sh>` (faithful, simple), or keep only shell-wrapper emulation (portable but filters are approximated in ERE/grep)? Proposed: `file_source: all` as COULD; no `files_cmd`.
10. **Parallel output streaming**: strict config-order head-of-line buffering (proposed, pre-commit-like) vs lefthook-style completion order with live progress. Also: should `follow`/`verbose` hooks in a concurrent bucket be line-prefixed?
11. **Interactive hooks on Windows** (no `/dev/tty`; `CONIN$` via `windows-sys` requires unsafe in a dependency). Should v1 restrict `interactive` to unix?
12. **Windows `sh` dependency** of migrated `sh -c` entries. Inside git hooks, Git for Windows puts `sh` on PATH, but `snp run` from cmd.exe may not have it. Should snp resolve `sh` like lefthook (`Program Files\Git\bin\sh.exe`, then `git.exe`'s `..\..\bin\sh.exe`) when an `entry` starts with `sh`?
13. **Timeout scope**: total hook wall time (proposed) vs per batch (lefthook effectively applies per job = all chunks).
14. **`SNP=0` vs security policy**: some teams want hooks to be mandatory. Provide `SNP_DISABLE_ALLOWED=0`? (`--no-verify` exists anyway.)
15. **LFS**: is pre-commit-style legacy chaining sufficient, or should snp also offer lefthook-style native `git lfs <hook>` passthrough when no legacy hook exists but the repo requires LFS (`.lfs-required`)?
16. **AI agent hooks (`ai:`)**: in scope for snp at all? If so, under `snp install --agent` with a root extension key, or a separate command?
