# 06 — prek (j178/prek): architecture, extensions, pitfalls, and lessons for snp

> Topic owner: research/06. Input for engineers implementing **snp** ("You Shell Not Pass").
> Subject: prek **0.5.5** (commit `19609fea4275d116285da041d48957f449423044`, 2026-10-05), an existing Rust
> reimplementation of pre-commit. Study only; **no prek code may be copied**. Everything below is a
> behavioral/architectural description written for clean-room reimplementation.
>
> Citation conventions:
> * Bare paths (e.g. `crates/prek/src/run.rs:109`) are relative to `refs/prek/`.
> * `pc:` prefix = `refs/pre-commit/` (pre-commit 4.6.x source). Observed pre-commit behaviour was checked
>   against the installed pre-commit **4.1.0** binary in a throwaway repo (`scratchpad/tmp-prek/`).
> * `#NNNN` = GitHub issue/PR `https://github.com/j178/prek/issues/NNNN`.
> * "**snp:**" marks a concrete recommendation.

---

## 0. Executive summary (read this first)

1. **Shape.** prek is a single binary crate (`crates/prek`, ~55k LOC src + ~30k LOC tests) plus three helper
   crates: `prek-consts` (env-var names, config filenames), `prek-identify` (file tagging; bitset `TagSet`),
   `prek-pty` (vendored `pty-process`, contains `unsafe`). It runs on a **single-threaded tokio runtime**
   (`new_current_thread`, `crates/prek/src/main.rs:505`), uses OS processes for parallelism, `rayon` +
   `spawn_blocking` for CPU-bound builtin hooks, and `FuturesUnordered` + a semaphore for scheduling.
2. **It is not a byte-exact drop-in.** Exit codes (config/operational errors exit **2**, pre-commit exits **1**),
   message wording, stream choice (stash messages on stderr vs pre-commit's stdout), colors, output
   indentation, filename shuffle order, YAML acceptance, regex dialect, `entry` splitting, autoupdate tag
   choice and several builtin hooks all differ from pre-commit. snp must treat these as compatibility bugs.
3. **Workspace mode is implicit.** Any nested `prek.toml`/`.pre-commit-config.yaml`/`.yml` under the
   workspace root silently becomes an extra project whose hooks run (#1739, #1044, #1061). Running from a
   subdirectory that has its own config changes the root. **snp:** default to exact pre-commit semantics
   (git-root config only); make multi-project mode explicit opt-in.
4. **Data-safety incidents** in the staged-file stash: index corruption / lost unstaged edits on conflicted
   restore (#2801, #2142, #1889), and **`GIT_INDEX_FILE`/`GIT_DIR` leaking into installer subprocesses**
   corrupting the user's index in linked worktrees (#2505, #1354). These are the most important lessons.
5. **Performance wins worth copying:** native builtin hooks + automatic "fast path" for
   `pre-commit/pre-commit-hooks`; skipping `git diff` entirely when every hook in a group reports whether it
   modified files; diff baseline reuse (clean baseline after stash = empty diff); cloning only repos whose
   hooks are selected; installing environments only for hooks that will actually run (have matching
   files); parallel clones/installs; one run-scoped file index + per-file tag cache; priority groups.
6. **Weaknesses snp can beat:** multiplicative concurrency (hooks × batches, #2272); eager tag computation
   for every file; diff snapshots buffered fully in memory; panics on overlong filenames (#1337) and in the
   progress UI (#1625, #2289); `unsafe` (pty, `sysconf`, `std::env::set_var`,
   `OsString::from_encoded_bytes_unchecked`); 64-bit non-crypto hashes as on-disk keys; `DefaultHasher`
   (not stable across Rust versions) for the workspace cache key; regex match errors silently treated as
   "no match"; nested-config parse errors silently skipped (only `tracing::error!`, invisible by default).
7. **Extensions** (all prek-only): `prek.toml`, `repo: builtin`, glob `files`/`exclude`, `priority` +
   `priorities` aliases, `groups` + `--group/--require-group/--no-group`, `env`/`default_env`, `shell`,
   `pass_filenames: <int>`, `language_version` map form, `include_deleted`, `hide_status`, `orphan`,
   `minimum_prek_version`, `update` table, `{hook_repo}` placeholder, `x-` keys, extra languages
   (bun, deno, dotnet, julia, mise, php), `exec`, `list`, `util …`, `cache …`, `self update`, `--dry-run`,
   `--last-commit`, `--directory`, `--glob`, `--no-fail-fast`, global user config `~/.config/prek/prek.toml`.

---

## 1. Repository and crate layout

### 1.1 Workspace (`Cargo.toml`)

| Item | Value | Source |
|---|---|---|
| Workspace members | `crates/*` (resolver 3) | `Cargo.toml:1-3` |
| Edition / MSRV | 2024 / `rust-version = "1.97"`; toolchain pinned to `1.99` | `Cargo.toml:5-7`, `rust-toolchain.toml` |
| Release profile | `lto = "fat"`, `codegen-units = 1`, `strip = true`, **`panic = "abort"`** | `Cargo.toml:192-198` |
| Extra profiles | `no-debug` (opt 1, no debuginfo), `profiling` (release + full debug, no LTO, 16 CGU) | `Cargo.toml:186-205` |
| Lints | `clippy::pedantic` warn; `print_stdout`, `print_stderr`, `dbg_macro`, `exit`, `get_unwrap` warn; `dead_code` allowed | `Cargo.toml:128-176` |
| Disallowed methods | `std::env::var`, `std::env::var_os` (forces reads through `EnvVars`) | `clippy.toml` |
| Features (prek crate) | `default = ["docker"]`; `self-update` (axoupdater); `profiler` (pprof); `schemars` (JSON schema gen) | `crates/prek/Cargo.toml:14-27` |

Note `panic = "abort"`: any panic (e.g. #1337, #1625) kills the process *without unwinding*, so `Drop`-based
stash restoration does **not** run. **snp:** keep `panic = "unwind"` in release *or* guarantee zero panics on
paths that hold repository mutations; prefer both (no panics + unwind + explicit restore guard).

### 1.2 Crates

| Crate | Purpose | Notes |
|---|---|---|
| `prek` | binary + all logic | ~55k LOC src |
| `prek-consts` | env-var name constants, config filenames, `prepend_paths`, bool parsing | `crates/prek-consts/src/lib.rs`, `env_vars.rs` |
| `prek-identify` | identify port: generated tables (`gen.py` from Python `identify`), `TagSet` = `[u64; 6]` bitset of 324 tags, `phf` maps | `crates/prek-identify/src/lib.rs:39-170`, `tags.rs` |
| `prek-pty` | vendored `pty-process` (async PTY); uses `unsafe` for fd conversion | `crates/prek-pty/src/pty.rs:30,137,309,321`, `sys.rs:26,73,83,128,150` |

### 1.3 Module map of `crates/prek/src` (LOC from `wc -l`, src only)

| Module | LOC | Responsibility |
|---|---|---|
| `main.rs` | 532 | runtime, logging, Ctrl-C, command dispatch, error printing |
| `cli/mod.rs` | 1654 | clap definitions, `ExitStatus`, hidden aliases |
| `cli/run/run.rs` | 1688 | run pipeline, scheduler, result rendering |
| `cli/run/filter.rs` | 911 | file selection, project file index, tag cache, filters |
| `cli/run/selector.rs` | 1167 | hook/project selectors, SKIP parsing, group filters |
| `cli/run/stash.rs` | 307 | unstaged-change stash/restore |
| `cli/run/diff.rs` | 68 | worktree diff tracker for modification detection |
| `cli/run/reporter.rs` | 1322 | progress UI, live output preview |
| `cli/run/install.rs` | 326 | env install partitioning + install cache |
| `cli/hook_impl.rs` | 411 | git-hook entry point (`hook-impl`), legacy hook, pre-push parsing |
| `cli/install.rs` | 547 | `install`/`uninstall`/`init-template-dir`, shim template |
| `cli/update/*` | ~2.5k | `update`/`autoupdate` (tags, cooldown, freeze, impostor check) |
| `cli/cache_gc.rs` | 834 | mark & sweep GC |
| `config/{mod,hook,repo,pattern,priority,update}.rs` | ~3k | serde config model |
| `workspace.rs` | 928 | project discovery, workspace cache, hook init |
| `hook.rs` | 1578 | `HookSpec`, `Hook`, `InstallInfo`, env requirement matching |
| `hook_entry.rs` | 319 | `entry` → argv (shlex), `shell` adapters, `{hook_repo}` |
| `languages/*` | ~12k | 25 language backends |
| `hooks/{builtin_hooks,pre_commit_hooks,meta_hooks}` | ~8k | native hooks |
| `git.rs` | 1510 | all git invocations |
| `process.rs` | 860 | `Cmd` wrapper (tokio::process), merged-pipe & PTY capture |
| `run.rs` | 482 | concurrency knobs, `ARG_MAX` partitioning, batch runner |
| `store.rs` | 420 | cache layout, cloning, config tracking |
| `fs.rs` | 753 | `CWD`, file locks, Windows rename retry, path helpers |
| `terminal.rs` | 331 | color detection, ANSI sanitizer, Windows VT fix |
| `printer.rs`, `warnings.rs` | small | output channels, `warning:` dedupe |
| `resource_limit.rs` | small | raise `RLIMIT_NOFILE` soft→hard on Unix |
| `http.rs`, `archive.rs`, `checksum.rs` | ~1k | toolchain downloads (reqwest+rustls), tar/zip, sha256 |

---

## 2. Architecture in detail

### 2.1 Process lifecycle (`crates/prek/src/main.rs`)

1. `CompleteEnv::with_factory(Cli::command).completer("prek").complete()` — dynamic shell completion
   short-circuits before anything else (`main.rs:487-489`).
2. `ctrlc::set_handler` sets `INTERRUPTED` and calls `exit_interrupted()` **from the handler thread**:
   runs registered cleanup closures (stash restore) under a global mutex, then `process::exit(130)`
   (Windows: `0xC000013A`) (`main.rs:462-495`). Cleanup registry: `crates/prek/src/cleanup.rs:1-17`
   (`Mutex<Vec<Box<dyn Fn()+Send>>>`, `.lock().unwrap()`).
3. `Cli::try_parse()`; on error `err.exit()` (clap's own exit code 2) (`main.rs:497-500`).
4. Runtime: `tokio::runtime::Builder::new_current_thread().enable_all()` (`main.rs:505-508`);
   `runtime.shutdown_background()` after `block_on`.
5. `run()` (`main.rs:155-460`): enable ANSI on Windows, set global color choice, open `Store`
   (`$PREK_HOME`), set up `tracing` (stderr layer filtered by `-v` count/`RUST_LOG`; file layer always at
   `prek=trace` into `$PREK_HOME/prek.log`, **truncated each run**, ANSI stripped) (`main.rs:100-153`),
   choose `Printer`, enable/disable warnings (`-q` disables), raise open-file limit,
   `git::init_git_work_tree()` **before** `--cd`, then `set_current_dir(--cd)`, dispatch.
6. Error printing: `error: <msg>` then `  caused by: <cause>` per chain element, exit 2
   (`main.rs:521-530`).

Exit codes (`crates/prek/src/cli/mod.rs:59-86`, documented `docs/reference/cli.md`):

| Code | prek meaning | pre-commit equivalent |
|---|---|---|
| 0 | success | 0 |
| 1 | hook failed / validation failed / "expected" failure | 1 |
| 2 | config error, operational error, clap usage error | **pre-commit: 1** for config/operational errors (`An error has occurred: InvalidConfigError: …`, verified with 4.1.0); clap/argparse usage error is 2 in both |
| 130 | interrupted (Windows `0xC000013A`) | 130 |
| N | `prek exec` propagates child code | n/a |

**snp:** match pre-commit (config/operational errors → 1, with the `An error has occurred: …` /
`Check the log at …` format); keep a distinct code only behind an explicit opt-in.

### 2.2 Configuration loading

* Filenames & precedence: `CONFIG_FILENAMES = [prek.toml, .pre-commit-config.yaml, .pre-commit-config.yml]`
  (`crates/prek-consts/src/lib.rs:8-13`). If several exist in one directory, the first wins and prek warns
  `Multiple configuration files found (`a`, `b`); using `<path>`` (`crates/prek/src/workspace.rs:269-290`).
  pre-commit itself only ever reads `--config` (default `.pre-commit-config.yaml`).
* Parsing (`crates/prek/src/config/mod.rs:218-231`): extension `.toml` (case-insensitive) → `toml` crate
  (TOML **1.1**, e.g. multi-line inline tables used in prek's own `prek.toml`); anything else →
  `serde_saphyr::from_str`. Errors: `Failed to parse `<path>`` + source (saphyr gives line/column + snippet).
* Post-parse: validate priority aliases (`Priority alias `{alias}` referenced by hook `{hook}` is not
  declared in `priorities``), resolve relative local `repo:` paths against the **config file directory**
  (not CWD; fixed in #1065) (`config/mod.rs:107-137`).
* Unknown keys: collected via `#[serde(flatten)] _unused_keys: BTreeMap<String, serde_json::Value>` at root,
  repo and **hook** level; keys starting with `x-` ignored silently; root keys `minimum_pre_commit_version`
  and `ci` silently ignored (`config/mod.rs:170-216`, `settings.rs:18-60`). Message:
  `warning: Ignored unexpected keys in `<path>`: `a`, `b`` (≤3) or a bullet list (≥4).
  pre-commit warns only for root and repo level (`[WARNING] Unexpected key(s) present at root: foo`;
  verified: hook-level unknown keys are silent in pre-commit).
* Mutable rev warning (`config/mod.rs:240-268`): a remote `rev` is "mutable" if it has no `.` and is not all
  hex; warning text begins `The following repositories use mutable `rev` values (branches or moving tags):`.
  pre-commit's equivalent heuristic/wording differs (`pc:pre_commit/clientlib.py` `_WarnMutableRev`).
* Not implemented in prek: pre-commit's "is a regex, not a glob — matching '/*' probably isn't what you want
  here" warnings and deprecated-stage-name warnings (`pc:pre_commit/clientlib.py:103,155,181,320,338`);
  prek silently accepts `commit`/`push`/`merge-commit` aliases (`config/hook.rs:112-117`).
* `minimum_prek_version` validated during deserialization against the running version; failure text:
  `` `prek` version `X` or newer is required, but version `Y` is installed. <hint> ``
  (`config/mod.rs:285-316`). `minimum_pre_commit_version` is **ignored** entirely.
* Data model: `Config` (`config/mod.rs:56-93`), `Repo` enum {Remote, Local, Meta, Builtin}, `RemoteHook`
  (all-optional overrides), `LocalHook`/`ManifestHook` (require id/name/entry/language), `MetaHook`,
  `BuiltinHook` (`config/hook.rs:653-951`), shared `HookOptions` (`config/hook.rs:469-528`).
  `Stages` is a `u16` bitset; empty list deserializes to empty (later replaced by defaults)
  (`config/hook.rs:154-235`). `PassFilenames` = `All | None | Limited(NonZeroUsize)`; `0`, negatives and
  strings rejected (`config/hook.rs:236-296`, tests `config/mod.rs:1286-1330`).
* `Language` is a closed enum (25 languages; aliases `unsupported`→`system`,
  `unsupported_script`→`script`) (`config/hook.rs:33-61`). Unknown language → parse error.

YAML behaviour differences vs pre-commit (PyYAML `SafeLoader`, YAML 1.1) — verified with pre-commit 4.1.0:

| Input | pre-commit | prek | snp must |
|---|---|---|---|
| `rev: 1.0` | **error** `=====> Expected string got float` (exit 1) | accepted as string `"1.0"` (`config/mod.rs:1271-1283`) | error like pre-commit |
| `always_run: yes` / `pass_filenames: no` | YAML 1.1 bools accepted | parser-dependent | accept YAML 1.1 bool set |
| duplicate mapping keys in config | silently last-wins | serde "duplicate field" error in manifests (#1158) | last-wins (optionally warn) |
| merge keys `<<: *a`, nested merges | supported | supported after fixes (#1019) | support |
| `!!binary` non-UTF-8 in check-yaml | ok | failed (#1102, #1104) | n/a for config; builtin must accept |
| very alias-heavy / comment-heavy YAML | ok | rejected by saphyr budgets: `AliasAnchorRatio` (#1838), "too many consecutive comments" (#2501) | no artificial budgets for trusted local files, or very generous ones |
| config file whose content is a path string (Windows checked-out symlink) | (see #858) | `invalid type: string …, expected struct Config` | match pre-commit |

### 2.3 Workspace / project discovery (`crates/prek/src/workspace.rs`)

* **Root finding** (`workspace.rs:622-642`): with `-c/--config`, workspace root = git root (single config
  mode). Otherwise walk ancestors of CWD (after `--cd`) up to the git root and stop at the **first** dir
  containing any config filename → that is the workspace root (can be a subdir of the git root).
* **Fresh discovery** (`workspace.rs:744-840`): `ignore::WalkBuilder::new(root).follow_links(false)
  .add_custom_ignore_filename(".prekignore").build_parallel()`; honors `.gitignore`, `.git/info/exclude`,
  global excludes, `.prekignore` (gitignore syntax), skips hidden dirs (ignore's default), Cookiecutter dirs
  (`{{…cookiecutter…}}`), and git submodules (from `.gitmodules`). Every directory with a config becomes a
  `Project`. A project whose config fails to parse is **skipped with `tracing::error!` only** (invisible
  without `-v`) unless it was explicitly selected (`workspace.rs:803-818`).
* **Ordering**: projects sorted by depth descending, then relative path (`workspace.rs:842-857`);
  `ProjectDepthGroups` iterates same-depth slices (`cli/run/run.rs:612-650`).
* **Workspace cache** (`workspace.rs:457-602`): JSON at `$PREK_HOME/cache/prek/workspace/<hash>` storing
  config paths with mtime+size; valid ≤ 1 h (`MAX_CACHE_AGE = 3600`); does **not** detect newly added
  configs or `.prekignore` changes → users need `--refresh` (documented, `docs/faq.md`). Key = std
  `DefaultHasher` of the root path (unstable across Rust releases; same class of bug as #1994).
* File ownership: each file is visible to every ancestor project (relative path stripped), unless an
  `orphan: true` project consumes it (`cli/run/filter.rs:363-436`, trie `ProjectPathNode`).
* Each project's hooks run with CWD = project dir; filenames are project-relative.
* Staged-config check: all *discovered* config files must be staged (`workspace.rs:903-927`), message
  ``Configuration file `<path>` is not staged. Stage it with `git add` and try again``
  (pre-commit: `Your pre-commit configuration is unstaged.\n`git add <cfg>` to fix this.`).

### 2.4 Hook model and merge order (`crates/prek/src/hook.rs`)

`HookSpec::from_remote(manifest, config)` (`hook.rs:65-91`): start from manifest hook; config overrides
`name`, `entry`, `language` (sets `language_overridden=true`, which disables the fast path), `priority`,
`groups`; then `HookOptions::update` overlays every set option. Then `with_project_defaults`
(`hook.rs:93-127`):

1. `default_language_version[language]` merged field-by-field into `language_version` (map form).
2. If `stages` absent **or empty** → `default_stages` or ALL stages.
3. `env` = `default_env` ∪ hook `env` (hook wins).

`validate` (`hook.rs:129-170`) errors:
`Hook specified `additional_dependencies: {deps}` but the language `{language}` does not install an environment`,
`… does not support installing dependencies for now`,
`Hook specified `language_version: {v}` but the language `{language}` does not support toolchain installation for now`,
`Hook specified `shell` but {repo} hooks do not support shell execution`,
`Hook specified `shell` but the language `{language}` does not support shell execution: {reason}`.

`Hook` runtime struct (`hook.rs:359-393`): `Arc<Project>`, `Arc<Repo>`, `idx`, id, name, `entry: HookEntry`,
language, alias, files/exclude (`FilePattern`), `include_deleted`, `types/types_or/exclude_types: TagSet`,
`additional_dependencies`, `args`, `env: FxHashMap`, `always_run`, `fail_fast`, `pass_filenames`,
description, `language_request`, `log_file`, `require_serial`, `stages: Stages`, `verbose`,
`minimum_prek_version`, `priority: u32` (implicit = config-wide index), `groups: BTreeSet<String>`.
`HookKey { project_idx, hook_idx }` identifies hooks across a workspace.

`entry` handling (`crates/prek/src/hook_entry.rs`):
* `HookEntry::Argv` → `shlex::split(entry)`; empty → `Failed to parse entry: entry is empty`; unparseable →
  `Failed to parse entry `…` as commands` (`hook_entry.rs:171-186`). Then `{hook_repo}` expanded **per argv
  element** (after splitting) to the cached checkout (remote) or project dir (local)
  (`hook_entry.rs:13-30`). Then `resolve_command` (`languages/mod.rs:675-740`): `which_in(cmd, PATH', cwd)`;
  explicit relative paths resolved against hook CWD; shebang parsed (`prek_identify::parse_shebang`) and
  interpreter resolved via PATH on **all** platforms; on Windows an absolute-looking missing interpreter
  (`/bin/sh`) falls back to basename lookup (Git-for-Windows behaviour).
* `language: script`: first token joined with repo path (`hook_entry.rs:151-162`).
* `HookEntry::Shell` (prek-only `shell:` key): entry written to a temp script, executed via adapter:

  | `shell` | argv | args/filenames visible as |
  |---|---|---|
  | `bash` | `bash --noprofile --norc -eo pipefail <script>` | `"$@"` |
  | `sh` | `sh -e <script>` | `"$@"` |
  | `pwsh` | `pwsh -NoProfile -NonInteractive -File <script>` (+`-ExecutionPolicy Bypass` on Windows, `.ps1`) | `$args` |
  | `powershell` | `powershell -NoProfile -NonInteractive -File <script>` | `$args` |
  | `cmd` | `cmd /D /E:ON /V:OFF /S /C CALL <script>` (`.cmd`) | `%*` |

  (`docs/reference/configuration.md:804-878`, `docs/proposals/hook-shell.md`). Unsupported for docker,
  docker_image, dart, fail, julia, rust, pygrep, r, meta, builtin.

### 2.5 Language backends (`crates/prek/src/languages/mod.rs`)

Trait `LanguageBackend` (`#[async_trait(?Send)]`, `languages/mod.rs:54-155`): `install(store, hook,
install_cwd, reporter) -> InstalledHook`, `check_health(info)`, `execution_environment`,
`prepare_hook_entry`, `prepare_execution_environment`, `run(store, hook, filenames, reporter) -> (i32,
Vec<u8>)` (default impl: resolve entry, `run_by_batch`, append `args` then batch filenames, stdin null,
capture via PTY sink). Dispatch: `Language::backend()` returns `&'static dyn LanguageBackend`
(`languages/mod.rs:301-329`).

Capability matrix (`languages/mod.rs:331-555`):

| Capability | Languages |
|---|---|
| `can_modify_files() == false` (no diff tracking needed) | `fail`, `pygrep` |
| installs an env | all except `docker_image`, `fail`, `script`, `system` |
| supports `language_version` (managed toolchains) | bun, deno, dotnet, golang, mise, node, python, ruby, rust |
| supports `additional_dependencies` | all env languages except docker, swift, pygrep (and non-env ones) |
| `prek exec` unsupported | docker, docker_image, fail, julia, pygrep, meta, builtin |

Notable per-language behaviour (from `docs/reference/language-support.md`):
* python: `uv venv` + `uv pip install --directory <repo> . <deps>`; uv found on PATH / next to prek /
  in store, else installed from Astral CDN → PyPI (+mirrors) → pip (`PREK_UV_SOURCE`); uv ≥ 0.7.0 required
  (`languages/python/uv.rs:24-26`); requires-python from repo `pyproject.toml` may set the version; PEP 723
  inline metadata supported for local scripts without `additional_dependencies`. Strips `UV_PYTHON`,
  `UV_SYSTEM_PYTHON`, `UV_MANAGED_PYTHON`, `UV_NO_MANAGED_PYTHON`, `PYTHONHOME`, sets `VIRTUAL_ENV`
  (`languages/python/python.rs:191-331`). Other `UV_*` vars leak in (#1299: `UV_RESOLUTION`).
* pygrep: **runs an embedded Python script** (`languages/pygrep/script.py`) with a found/managed Python 3.13
  to keep Python `re` semantics; filenames fed on stdin **newline-separated** (breaks filenames containing
  `\n`), args parsed: `-i/--ignore-case`, `--multiline`, `--negate`, anything else → `Unknown argument`
  (`languages/pygrep/pygrep.rs:19-50,160-230`).
* docker/docker_image: runtime auto-detect (docker/podman/Apple `container`), `--init` unless
  `PREK_DOCKER_NO_INIT`, mounts project at `/src`, non-root uid on Linux (uses `unsafe { libc::geteuid() }`,
  `languages/docker.rs:353-354`).
* rust: `cargo install --bins --locked`; `cli:` deps syntax; rustup `minimal` profile (`PREK_RUST_PROFILE`).
* node: npm install from a `git+file://` URL of the cached clone (#2394) — the source of #2505.

### 2.6 Store layout, locking, environment reuse (`crates/prek/src/store.rs`, `hook.rs`)

`$PREK_HOME` (default `~/.cache/prek` on Linux **and macOS** via `etcetera` XDG strategy,
`%LOCALAPPDATA%\prek` on Windows; `~` expanded) (`store.rs:76-96`):

```
$PREK_HOME/
  README                      # created once (create_new)
  .lock                       # whole-store cross-process lock (std File::lock)
  prek.log                    # trace log of the last run (truncated per run)
  config-tracking.json        # set of config paths ever used (for GC)
  repos/<hex(seahash(source,rev))>/   + .prek-repo.json {repo, rev}
  hooks/<language>-<20 random chars>/  + .prek-hook.json (InstallInfo)
  tools/{uv,python,node,go,ruby,rustup,bun,dotnet,deno,mise}/<version>/
  cache/{uv,go,python,cargo,deno,npm,coursier,prek}/  (prek/workspace/<hash> = workspace cache)
  patches/<millis>-<pid>.patch       # stash patches, dir chmod 0700, GC keeps 30 days
  scratch/                    # temp clones/downloads (same FS → atomic rename)
```

* Repo key: `seahash(source) ++ seahash(rev)` → 64-bit hex (`store.rs:299-304`). Clone into
  `scratch/` temp dir then `rename_with_retry` into `repos/<key>`; marker file written **after** rename; a
  repo is considered present iff the marker exists (`store.rs:156-178`). Clones run concurrently
  (`buffer_unordered(INTERNAL_CONCURRENCY = CPU count)`).
* Clone algorithm (`git.rs:657-870`): `git -c init.defaultObjectFormat= init --template= <dir>`,
  `git remote add origin <url>`, then **shallow**: `git -c protocol.version=2 fetch origin <rev> --depth=1`
  + `git checkout FETCH_HEAD`, falling back to **full**: `git fetch origin --tags` + `git checkout <rev>`;
  submodules updated only if `.gitmodules` exists or a gitlink (mode 160000) is in the index. All with
  `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`, repo-local GIT_* vars removed, and
  `PREK_INTERNAL__SKIP_POST_CHECKOUT=1`. If stderr looks like an auth failure
  (`terminal prompts disabled`, `could not read username`, …, `git.rs:700-725`) and not in CI (`CI` unset),
  retry once with prompts enabled after printing
  `Authentication may be required to clone repository `<repo>`. Retrying with terminal prompts enabled.`
* Hook environments are **decoupled from repos**: `InstallInfo` JSON marker (`hook.rs:707-815`) =
  `{schema_version: 1, language, language_version (semver), repo: {url, rev}?, dependencies[], env_path,
  toolchain, extra{}}`. A hook reuses an env iff `schema_version` matches, same language, same repo identity
  (or both local), **identical dependency sequence** (order-sensitive; #607 showed superset reuse was a bug),
  and `language_request.satisfied_by(info)` (`hook.rs:625-634`). Local hooks with identical deps share envs
  across projects and repos. Toolchains in `tools/` are shared across all envs.
* Env lookup: on first need per run, scan **all** `hooks/*/.prek-hook.json` concurrently and health-check
  lazily (`cli/run/install.rs:161-320`). Missing envs are installed in partitions (same language family,
  repo, deps sequence) — partitions in parallel, sequential inside a partition so later hooks reuse the env
  just created (`cli/run/install.rs:20-160`). New env dirs are created as `tempfile::TempDir` and only
  `persist`ed after success (failed installs auto-cleaned).
* Locking: `LockedFile::acquire(.lock)` — `try_lock()`, else blocking `lock()` on `spawn_blocking`, with a
  1 s timer to print a "waiting for lock" warning (suppressed for in-process contention)
  (`fs.rs:167-290`). Held during repo init and during env install (`cli/run/run.rs:159-172,431`).
* GC (`cli/cache_gc.rs`): mark repos/envs/tools reachable from **tracked configs that still exist**
  (parse configs, no cloning), keep the `prek` cache bucket, sweep `repos/`, `hooks/`, `tools/<bucket>`,
  `tools/<bucket>/<version>`, `cache/<bucket>`, `scratch/`, patches older than 30 days. Supports `--dry-run`
  and verbose per-item details.

### 2.7 Git integration (`crates/prek/src/git.rs`)

* `GIT` resolved once with `which`; on macOS `/usr/bin/git` is resolved through `xcrun --find git` to skip
  the shim on every call (`git.rs:47-65`).
* Every git call goes through `git_cmd()` which prepends hidden `-c core.useBuiltinFSMonitor=false` and,
  if needed, `GIT_WORK_TREE` (`git.rs:183-192`).
* `GIT_DIR` set without `GIT_WORK_TREE` (git does this for some hooks): the original CWD is captured at
  startup *before* `--cd` and passed as `GIT_WORK_TREE` only to git commands and to hooks whose CWD is not
  the work-tree root — never via process-wide env mutation (`git.rs:66-182`; see pre-commit issue 2295).
* Repo discovery: one `git rev-parse --absolute-git-dir --git-common-dir --git-path hooks --show-toplevel`
  with `LC_ALL=C`; detects `fatal: not a git repository (or any` → `Not in a Git repository. Change to a Git
  repository, or run `git init` to create one.`; falls back to one call per field when paths contain
  newlines; root canonicalized with `dunce` (`git.rs:581-655`).
* `GIT_REPO_LOCAL_ENVS` removed for operations on *other* repos (clone/update/install):
  `GIT_ALTERNATE_OBJECT_DIRECTORIES, GIT_CONFIG, GIT_OBJECT_DIRECTORY, GIT_DIR, GIT_WORK_TREE,
  GIT_IMPLICIT_WORK_TREE, GIT_GRAFT_FILE, GIT_INDEX_FILE, GIT_NO_REPLACE_OBJECTS, GIT_REPLACE_REF_BASE,
  GIT_PREFIX, GIT_INTERNAL_SUPER_PREFIX, GIT_SHALLOW_FILE, GIT_COMMON_DIR`; `GIT_CONFIG_PARAMETERS`,
  `GIT_CONFIG_COUNT/KEY_*/VALUE_*` deliberately kept (`git.rs:119-138`).

File-listing commands used by `prek run` (all `-z`, paths kept as raw bytes on Unix):

| Purpose | Command | Notes |
|---|---|---|
| staged (default) | `git diff --raw --no-renames --no-relative -z [--diff-filter=d] --cached` (+hidden `--no-ext-diff`) | raw format lets deleted entries carry their old mode for `include_deleted` (`git.rs:310-352,397-408`) |
| from/to refs | same raw diff with `<old>...<new>`, fallback `<old>..<new>` | (`git.rs:260-286`) |
| all files | `git --literal-pathspecs ls-files -z -- <workspace_root>` | (`git.rs:356-376`); pre-commit: `git ls-files -z --deduplicate` (`pc:pre_commit/git.py:154-155`) |
| merge conflict | `git write-tree` then `git diff --name-only --no-relative -z -m <tree> HEAD MERGE_HEAD` ∪ paths from `MERGE_MSG` lines starting with `\t` or `#\t` | (`git.rs:486-536`) |
| worktree status | `git diff --name-status -z --no-renames --no-relative --no-ext-diff --ignore-submodules` (`U` → unmerged, `A` → intent-to-add) | one call replaces several (`git.rs:417-461`) |
| modification snapshot | `git diff --full-index --no-ext-diff --no-textconv --ignore-submodules -- <project path>`, **non-zero exit tolerated** (shallow/blobless CI, #2208) | (`git.rs:538-570`) |
| LFS check | `git check-attr filter -z --stdin` with concurrent write/read (deadlock fixed, #730) | (`git.rs:981-1041`) |
| hooksPath | `git config --includes [--local \| --worktree] --null --get core.hooksPath`; `core.hooksPath=` (empty) rejected: `Git resolved hooks directory to the current directory (`.`). Unset `core.hooksPath` or set it to a real directory path.` | (`git.rs:378-395,873-910`) |

### 2.8 The `prek run` pipeline (`crates/prek/src/cli/run/run.rs:77-332`)

1. If stage is `post-checkout` and `PREK_INTERNAL__SKIP_POST_CHECKOUT` set → exit 0 (prevents recursion from
   prek's own checkouts; pre-commit uses `_PRE_COMMIT_SKIP_POST_CHECKOUT`).
2. If the file selection requires a clean worktree (default staged mode or `--from-ref/--to-ref`):
   concurrently resolve git root and `worktree_status`; unmerged → error
   `Found unresolved merge conflicts. Resolve the conflicts, stage the files with `git add`, and try again`
   (pre-commit: `Unmerged files.  Resolve before committing.`, exit 1, based on non-empty
   `git ls-files --unmerged`, checked only when stashing, i.e. not `--all-files`/`--files`;
   `pc:pre_commit/commands/run.py:325-349`). Both detect unmerged index entries; wording and exit path differ.
3. Load user settings (`~/.config/prek/prek.toml`), find workspace root, parse selectors and group filters,
   discover workspace, check configs are staged.
4. Under the store lock: track configs, `init_hooks` — **only repos referenced by selected hooks are cloned**
   (`HookInitFilters`, `workspace.rs:58-170`), manifests read, specs merged.
5. Apply selectors (`Selected` / `Skipped` (shown as Skipped) / `NotSelected`), then group filters; report
   unused selectors as warnings. Empty → `error: No hooks found after filtering with the given selectors`
   (+ `--refresh` hint) exit 1.
6. Infer stage & input mode (`run.rs:334-360`): explicit `--stage` wins; with group filters and no stage →
   files mode and any stage; otherwise `pre-commit`, except if the user named only hook ids and none of them
   can run at `pre-commit` → `manual` (mirrors pre-commit's "run named manual hook" behaviour). Hooks not in
   the stage are dropped; empty → exit **0** silently (fixed to match pre-commit in #1300).
7. Stash unstaged changes if any unstaged path is under the **workspace root** (§2.9).
8. `set_env_vars` — sets `PRE_COMMIT=1` and the `PRE_COMMIT_*` stage variables **process-wide with
   `unsafe { std::env::set_var }`** (`run.rs:367-418`).
9. Input-mode shortcut: if all selected hooks are `always_run && pass_filenames: false` and the selection is
   default/all → don't list files at all.
10. Collect input (`cli/run/filter.rs:545-590`), strip workspace prefix, `set_current_dir(workspace root)`.
11. Build `RunFileIndex` once: per-project file lists + `FileTagCache` computed **eagerly for every file in
    parallel with rayon** (`filter.rs:152-176,363-436`).
12. `ensure_hooks_installed`: only hooks that need an env **and** have at least one matching file (or
    `always_run`) are installed (`run.rs:420-522`). pre-commit installs every non-skipped hook of the stage.
13. `run_hooks` (`run.rs:524-603`): for each same-depth project slice (deepest first), run projects
    concurrently; inside a project, sort hooks by `(priority, idx)`, split into priority groups, run each
    group's hooks concurrently (semaphore `PREK_CONCURRENT_HOOKS`), each hook's batches concurrently
    (`PREK_CONCURRENT_BATCHES` or 1 if `require_serial`).
14. Modification detection per group (`run.rs:849-927`, `cli/run/diff.rs`): builtin/fast-path/meta hooks
    report `FileChanges::{Unchanged, Modified}` precisely; external hooks report `Unknown`. A `git diff`
    snapshot is taken **only** if the group has ≥1 hook that `requires_diff_tracking`
    (`hooks/mod.rs:91-98`); the baseline after a stash is known-empty (no "before" diff). If any hook in a
    group reports `Modified`, no diff is run and the baseline is invalidated.
15. Group result: if files were modified and exactly one hook executed → that hook is marked Failed with
    `- files were modified by this hook`; if several executed → the whole group fails with a tree:

    ```
    Files were modified by following hooks...................................Failed
      ┌ Modifies File........................................................Passed
      │ Prints Output........................................................Passed
      └ No Output............................................................Passed
    ```
16. `fail_fast` (CLI `--fail-fast/--no-fail-fast` > config `fail_fast` > false) or a failing hook with
    `fail_fast: true` stops after the **current priority group** of that project and after the current depth
    level (other projects at the same depth already ran).
17. `--show-diff-on-failure` and any modification → (in CI an extra hint) `All changes made by hooks:` then
    `git --no-pager diff --no-ext-diff --color=always|never -- <workspace root>` (`run.rs:1177-1230`).
18. Restore stash; if restoration had to revert hook changes the run becomes Failure.

### 2.9 Staged-file stashing (`crates/prek/src/cli/run/stash.rs`)

Algorithm (all git commands run with CWD = git root):

1. Inputs: `intent_to_add` paths from the worktree status, retained only if under the workspace root.
2. Register a Ctrl-C cleanup closure **before** mutating anything (shared `Arc<Mutex<Option<Pending>>>`;
   `None` = restoration already attempted) (`stash.rs:272-306`).
3. Under the mutex: `git rm --cached -- <intent-to-add…>`; `tree = git write-tree`;
   `git diff-index --binary --exit-code --ignore-submodules --no-color --no-ext-diff --no-textconv
   --no-relative <tree> -- <workspace_root>`: exit 0 → clean; exit 1 with empty stdout → treat as clean
   (CRLF-only differences); other → error `Failed to save unstaged changes:\n<stderr>` (`stash.rs:45-80`).
4. Write patch to `$PREK_HOME/patches/<unix-millis>-<pid>.patch` prefixed with
   `# prek pre-hook index tree: <tree>`; print (stderr, yellow bold)
   ``Unstaged changes detected. Temporarily saving them to `<path>` ``.
   pre-commit prints on **stdout**: `[WARNING] Unstaged files detected.` /
   `[INFO] Stashing unstaged files to <path>.` (verified).
5. `git -c submodule.recurse=0 checkout -- <workspace_root>` with `PREK_INTERNAL__SKIP_POST_CHECKOUT=1`.
6. After hooks: `git apply --whitespace=nowarn <patch>`. On failure: print (stderr, red bold)
   `Hook changes conflicted with the saved unstaged changes. Reverting the hook changes`, then
   **0.5.5 fix (#2801)**: `git reset --quiet <tree> -- <root>` (restore the *captured* index, not the live
   one), `git checkout -- <root>`, `git apply` again; failure →
   ``Failed to restore unstaged changes.\nYour changes are saved in `<patch>`.\nPre-hook index tree: <tree>``
   (`stash.rs:113-177`). Then `git add --intent-to-add -- <paths>` is attempted even if the patch step
   failed. Success message: ``Restored unstaged changes from `<path>` ``
   (pre-commit: `[INFO] Restored changes from <path>.`;
   conflict: `Stashed changes conflicted with hook auto-fixes... Rolling back fixes...`).
7. `Drop` and the Ctrl-C cleanup both call the same restore-once function.

History of failures (lessons):
* #2801 (0.5.3): rollback used `git checkout -- <root>` against the **live** index; a hook that ran `git add`
  (or index drift) made the retried apply fail; staged + unstaged edits of 98 files vanished while
  `git status` looked clean. pre-commit has the same structural weakness (`git checkout -- .` without
  tree-ish, `pc:pre_commit/staged_files_only.py:85-95`).
* #2142: intent-to-add file contents lost after conflicted restore. #1889: index corrupted after rollback,
  new files committed empty. #385/#486: `git commit -a` / `index.lock` interactions.
* **snp:** see §7 item 9 and §8 "Stash" (verify-after-restore, never lose the patch, restore from captured tree).

### 2.10 Batching ("xargs") (`crates/prek/src/run.rs`)

* Limits: Unix `ARG_MAX` via **`unsafe { libc::sysconf(_SC_ARG_MAX) }`** minus one page minus 2048,
  clamped to `[4096, 1 MiB]` (changelog: earlier capped at `1<<19`); Windows `32768 − 2048`; `.cmd`/`.bat`
  entries `8192 − 1024` (`run.rs:84-123,159-169`). On Unix also subtract the **whole environment size**
  (`8 + len(k) + 1 + len(v) + 1` per var, with hook `env` overrides) and the argv of `entry` + `args`.
  pre-commit clamps to `[2**12, 2**17]` and subtracts env size (`pc:pre_commit/xargs.py:49-58`).
* If `entry + args` alone exceed the limit → error
  `Command line length (N bytes) exceeds platform limit (M bytes).` + hint.
* Partition size: `pass_filenames: n` → `n` per batch; otherwise `max(4, ceil(files / concurrency))` where
  `concurrency = 1` if `require_serial` else `PREK_CONCURRENT_BATCHES` (same formula as pre-commit
  `pc:pre_commit/xargs.py:86`). Greedy fill by length; zero files → one empty batch.
* **Panics** if a single filename doesn't fit (`run.rs:226-234`, "is too long to fit in command line", #1337 —
  triggered on CPython because of the env-size accounting).
* Batches run via `stream::iter(partitions).map(run).buffered(concurrency)`; outputs concatenated in batch
  order; exit codes combined with **bitwise OR** (`run.rs:252-255`). pre-commit keeps the code with the
  largest `abs()` (`pc:pre_commit/xargs.py:179-181`) — OR can fabricate codes (1|2 = 3).
* Filenames shuffled deterministically before batching with `fastrand::Rng::with_seed(1_542_676_187)`
  (`cli/run/run.rs:1376-1384`). pre-commit uses the same seed with **CPython's `random.Random`
  (MT19937, `seed(..., version=1)`)** (`pc:pre_commit/lang_base.py:22,146-153`) → **different order**.

### 2.11 Process spawning and output capture (`crates/prek/src/process.rs`)

* `Cmd` wraps `tokio::process::Command` with: hidden-arg ranges (not shown in logs), a file-arg boundary
  (file list elided in logs/errors), `check(bool)` for status checking, task-local env injection
  (`with_command_env`) so concurrent installs get isolated env (`process.rs:46-60,128-240`).
* Hook output capture: **one OS pipe shared by stdout and stderr** (`std::io::pipe()`, writer cloned) so
  interleaving matches the child's write order (#2382 fixed stdout-then-stderr ordering), stdin
  `/dev/null` (#1154: hooks hung waiting on a TTY stdin), 4 KiB read loop streaming chunks into a sink for
  the live preview (`process.rs:278-321`).
* If color is enabled (non-Windows) the hook runs under a **PTY** (both stdout/stderr = slave, stdin null),
  not a session leader (avoids termenv OSC probe hangs; changelog 0.3.2 "Do not make the child a session leader"), EIO = EOF on Linux, drain
  after exit (`process.rs:323-414`). Same policy as pre-commit (PTY only when color).
* Displayed output is passed through `sanitize_output` (`terminal.rs:130-138`): bytes → `from_utf8_lossy`
  → ANSI parser keeping only SGR, resolving `\r` overwrites and `EL`, dropping OSC/DCS/cursor moves.
  pre-commit writes the raw bytes. Output lines are **indented by two spaces**
  (`cli/run/run.rs:1150-1173`); pre-commit prints them unindented after a blank line.

### 2.12 Output and printer

`Printer` (`crates/prek/src/printer.rs`): `Silent` (`-qq`/`PREK_QUIET=2`: nothing), `Quiet` (`-q`: only
"important" stdout, i.e. failures), `Default`, `Verbose` (`-vv+`: no progress bar), `NoProgress`.
Warnings: `warning: <msg>` (yellow bold) on stderr, dedupable (`warnings.rs`).

Status line (`cli/run/run.rs:1446-1500`): `columns = max(79, max_name_width + 3 + width("(no files to
check)Skipped"))`; line = `prefix + name + dots + suffix + label`, dots = `columns − used` (saturating;
an underflow panic existed before, #1625). Equivalent width to pre-commit's
`max(name+3+len(NO_FILES)+1+len(SKIPPED), 80) − 1` (`pc:pre_commit/commands/run.py:238-250,46-57`).
Width uses `unicode-width` (pre-commit uses East-Asian-width W/F = 2).

| Element | prek | pre-commit |
|---|---|---|
| Passed / Failed / Skipped label | `Style::new().green()/red()/cyan().reversed()` (`cli/run/mod.rs:21-24`) | background colors `\033[42m` / `\033[41m` / turquoise `\033[46;30m` (no files) and yellow `\033[43;30m` (SKIP) |
| dots | colored | uncolored |
| SKIP'd hook | `Skipped` | `Skipped` |
| no files | `(no files to check)Skipped` | same |
| dry run | `Dry Run` label; output `` `<hook>` would be run on N files:`` + `- <file>` lines | n/a (no dry-run) |
| details | `- hook id: X`, `- hook alias: Y` (if ≠ id), `- description: <first line, trailing '.' removed>`, `- duration: 0.01s` (verbose), `- exit code: N`, `- files were modified by this hook` | `- hook id`, `- duration` (verbose), `- exit code`, `- files were modified by this hook` |
| hook output | blank separator line, then each line indented 2 spaces, sanitized | blank line, raw bytes |
| `log_file` | output written (append) to file relative to **config dir** and **not** printed | written (append, `'ab'`) **and** printed (`pc:pre_commit/output.py:14-29`) |
| projects | header `✓ <name>`/`× <name>` (`<workspace>` for root) when >1 project or non-root | n/a |
| progress | spinner/bars + live 1-line output preview on stderr (indicatif) | none |
| `hide_status` | hides reports by final status (passed/failed/skipped) | n/a |

Progress UI caused garbled output in non-TTY GUI integrations (#1551 Neovim/Fugitive, #718 Zed hang,
#1237/#950 PowerShell) and panics inside indicatif (#1625, #2289).

### 2.13 Git hook shims (`crates/prek/src/cli/install.rs`, `cli/hook_impl.rs`)

Shim template (`install.rs:370-386`, `/bin/sh`):

```sh
#!/bin/sh
# File generated by prek: https://github.com/j178/prek
# ID: 182c10f181da4464a3eec51b83331688
HERE="$(cd "$(dirname "$0")" && pwd)"
PREK=[PREK_PATH]            # absolute current_exe(), quoted
if [ ! -x "$PREK" ]; then PREK="prek"; fi
exec "$PREK" hook-impl --hook-dir "$HERE" --script-version 4 [ARGS] -- "$@"
```

`[ARGS]`: include selectors, CLI `--skip` selectors (env SKIP ignored with a warning),
`--hook-type=<t>`, `--config="<path>"` (only if given) **or** `--cd="<project rel path>"` (workspace not at
git root), `--skip-on-missing-config`. Mode: `0o755` adjusted by `core.sharedRepository`
(`git.rs:912-979`). Ownership detection only by the prek hash (`PRIOR_HASHES` empty, `install.rs:388-400`) — **pre-commit's
scripts (current hash `138fd403232d2ddd5efb44317e38bf03`, priors `4d9958c90bc262f47553e2c073f14cfe`,
`d8ee923c46731b42cd95cc869add4062`, `49fd668cb42069aa1b6048464be5d395`, `79f09a650522a87b0da915d0d983b2de`,
`e358c9dae00eac5d06b38dfdb1e33a8c`; `pc:pre_commit/commands/install_uninstall.py:23-30`) are not recognized**, so `prek install` over a
pre-commit install moves pre-commit's script to `<hook>.legacy` and runs it in "migration mode" →
both tools run (#1620, #1621; docs tell users to `pre-commit uninstall` first).

`core.hooksPath` policy (`install.rs:41-95`, decision at `install.rs:58`): repo-local/worktree-local value → honored; only global/system
→ refuse with a multi-line `Refusing to install hooks because `core.hooksPath` is configured outside this
repository.` message unless `--force` (installs into `<common-dir>/hooks`) or `--git-dir`. pre-commit refuses
whenever `core.hooksPath` is set at any scope.

`hook-impl` (`hook_impl.rs`):
* reads stdin fully **only** for `pre-push`; runs `<hook>.legacy` first if executable (Windows: always),
  with `PREK_RUNNING_LEGACY=1` and the same stdin; recursion guard error
  `prek's Git shim is installed in migration mode\nrun `prek install -f --hook-type {hook_type}` to reinstall the shim`.
* warns if `--script-version` ≠ 4.
* missing config: if allowed (`--skip-on-missing-config` or `PREK_ALLOW_NO_CONFIG`/`PRE_COMMIT_ALLOW_NO_CONFIG`)
  return legacy code; else error + three hints (`- To temporarily silence this, run `PREK_ALLOW_NO_CONFIG=1 git ...``…).
* argument count validation per hook type: commit-msg 1; post-checkout 3; pre-commit 0; post-commit 0;
  pre-merge-commit 0; post-merge 1; post-rewrite 1; pre-push 2; pre-rebase 1–2; prepare-commit-msg 1–3.
* pre-push stdin parsing (`hook_impl.rs:301-370`): for each line `local_ref local_sha remote_ref
  remote_sha`: skip deletes (all-zero local sha); if remote sha exists locally and is an ancestor → range
  `remote..local`; else `git rev-list <local> --topo-order --reverse --not --remotes=<remote>`; first
  ancestor is a root commit → `--all-files`; else from = parent of first new commit. **Only the first
  pushable ref is used** (#2842, inherited from pre-commit `_pre_push_ns`, `pc:pre_commit/commands/hook_impl.py:120`).
* Git ≥ 2.54 config-based hooks: `hook-impl` without `--hook-dir` skips legacy lookup.
* Final status: hook failure code wins, else legacy code.

### 2.14 `update` (autoupdate) design (`crates/prek/src/cli/update/`)

* Each distinct repo fetched once (temp repo, `--filter=blob:none`, HEAD + tags), in parallel (`-j`).
* Tag selection: **newest tag by creation timestamp** (annotated) / commit timestamp (lightweight), ties
  broken by semver; cooldown window (`--cooldown-days`, 0–255) never downgrades; include/exclude tag globs
  global and per repo; multiple tags on one commit → prefer version-like, then Levenshtein-closest to the
  current rev (`repository.rs:153-370`). pre-commit uses `git describe FETCH_HEAD --tags --abbrev=0` (or `--exact`)
  (`pc:pre_commit/commands/autoupdate.py:46`) — prek's timestamp ordering caused a "rollback" (#1533).
* `--freeze` writes SHA + `# frozen: <tag>` comment; verifies pinned SHAs exist in fetched refs using
  `git --no-lazy-fetch cat-file -e` (**impostor-commit** protection: fork-only SHAs on GitHub)
  (`repository.rs:54-110`).
* Validates that all configured hook ids still exist in the new manifest before rewriting.
* Rewrites YAML preserving quote style (`yaml.rs:10-21`, quotes numeric-looking revs, #1935) and TOML via
  `toml_edit`. `--dry-run`, `--exit-code`, `--check` (= both), `--bleeding-edge`, `--repo`,
  `--exclude-repo`.

---

## 3. Performance and memory techniques (what made prek fast)

| # | Technique | Where | Effect / note |
|---|---|---|---|
| P1 | Native Rust implementations of 22 `pre-commit-hooks` hooks + 7 extra builtins; **automatic fast path** when `repo == "https://github.com/pre-commit/pre-commit-hooks"` exactly, id known, `language` not overridden, `PREK_NO_FAST_PATH` unset | `hooks/mod.rs:73-89`, `hooks/pre_commit_hooks/mod.rs:126-225` | benchmark: 1,534 ms → 105 ms (14.6×) on 13 hooks / 960 files (`docs/benchmark.md`) |
| P2 | Builtin hooks report precise `FileChanges` → **no `git diff`** for groups made only of such hooks | `hooks/mod.rs:23-71,91-98`; `cli/run/run.rs:868-905` | pre-commit runs 1 + N diffs (N executed hooks) |
| P3 | Clean baseline after stash = empty diff (no "before" diff); diff snapshot reused as next baseline | `cli/run/diff.rs:18-62` | halves diffs for external hooks |
| P4 | Only clone repos of selected hooks; only install envs for hooks with matching files | `workspace.rs:58-170`; `cli/run/run.rs:470-522` | pre-commit clones all repos, installs all stage hooks |
| P5 | Parallel clones and env installs (partitioned to keep reuse) | `store.rs:156-260`; `cli/run/install.rs:20-160` | |
| P6 | `uv` for venv + pip; managed toolchains shared across envs; Python interpreter probe with `-I -S` | `languages/python/*` | install 3-4× faster (#872 comment) |
| P7 | Run-scoped `RunFileIndex` + `FileTagCache` (rayon) shared by env selection and execution | `cli/run/filter.rs:152-176,363-436` | each file identified once |
| P8 | `TagSet` bitset (`[u64;6]`) → `types`/`types_or`/`exclude_types` checks are word ANDs | `crates/prek-identify/src/lib.rs:39-170` | vs `HashSet<&str>` |
| P9 | Priority groups (same-priority hooks concurrently) and same-depth projects concurrently | `cli/run/run.rs:524-960` | benchmark: 105 → 94 → 80 ms |
| P10 | Builtin per-file checks: one `spawn_blocking` per batch + `rayon` `par_iter`, output merged in input order | `hooks/pre_commit_hooks/mod.rs:90-120` | |
| P11 | Combined git queries: one `rev-parse` for 4 paths; one `diff --name-status` for unmerged/unstaged/ITA; overlap discovery with status | `git.rs:581-655,417-461`; `cli/run/run.rs:121-140` | 0.5.4/0.5.5 changelog |
| P12 | macOS: resolve `/usr/bin/git` shim via `xcrun` once | `git.rs:47-65` | avoids per-call xcrun |
| P13 | `-c core.useBuiltinFSMonitor=false` on every git call | `git.rs:183-192` | avoids fsmonitor daemon startup |
| P14 | Workspace discovery cache (1 h) | `workspace.rs:457-602` | cost: stale discovery, `--refresh` |
| P15 | Raise `RLIMIT_NOFILE` soft limit to hard limit | `resource_limit.rs` | prevents EMFILE under concurrency |
| P16 | Release build: fat LTO, 1 CGU, strip, abort | `Cargo.toml:192-198` | |
| P17 | Skip file listing entirely when all hooks are `always_run && pass_filenames:false` | `cli/run/run.rs:228-241` | |
| P18 | Health checks lazy + cached per run (`query_python_info_cached`, shared version probes) | changelog 0.5.5 | |

Memory techniques: borrowed views (`HookEnvRequirementRef`, `ProjectFile<'a>` holding `&Path` + index),
`Arc<Project>/Arc<Repo>` sharing, `Stages` u16 bitset, `TagSet` bitset, `FxHashMap` everywhere,
`Box<Parser>` to shrink progress structs, `strip_prefix_in_place` to avoid reallocating paths
(`cli/run/filter.rs:586-612`, uses `unsafe`). Weak spots: whole-file reads in builtins, full diff patches
held in memory (`Vec<u8>`), every hook's full output buffered, eager tag computation for all files,
`Vec<FileEntry>` of `PathBuf` (one heap alloc per path).

---

## 4. Extensions beyond pre-commit (prek-only), with syntax

| Extension | Syntax (YAML / TOML) | Semantics | Source |
|---|---|---|---|
| `prek.toml` config | top-level same keys; `[[repos]]`, `hooks = [{…}]` or `[[repos.hooks]]` | TOML 1.1 accepted; preferred over YAML in same dir | `docs/configuration.md`, `prek.toml` |
| User global config | `~/.config/prek/prek.toml` (`$XDG_CONFIG_HOME`), `%APPDATA%\prek\prek.toml` | only `hide_status`, `update.*` | `docs/reference/configuration.md:5-57` |
| `repo: builtin` | `- repo: builtin` / `hooks: [{id: trailing-whitespace}]` | no clone/env; `entry` forbidden; `language` must be `system` | `config/hook.rs:888-951` |
| glob filters | `files: {glob: "src/**/*.rs"}` or `{glob: [a, b]}`; same for `exclude`, top-level and hook | `globset` defaults (`*` **does** cross `/` because `literal_separator` is not set); `glob: []` matches nothing | `config/pattern.rs:11-235` |
| `priority` + `priorities` | `priorities: {format: 0, checks: 10}`; hook `priority: checks` or `priority: 10` | lower first; equal → concurrent; implicit = index in flattened config | `config/priority.rs`, `docs/proposals/concurrency.md` |
| `groups` | `groups: [lint, ci]` (not from manifests; names non-empty, no whitespace, no leading `@`) | `--group` (OR), `--require-group` (AND), `--no-group` (exclude wins), `@ungrouped` | `docs/proposals/hook-groups.md` |
| `env`, `default_env` | `env: {RUSTDOCFLAGS: -Dwarnings}` | applied at env creation and run; hook > default; manifest `env` merged, config wins; passed into containers with `-e` | `docs/reference/configuration.md:927-995` |
| `shell` | `shell:` one of `bash`, `sh`, `pwsh`, `powershell`, `cmd` | entry treated as script source (§2.4) | `docs/proposals/hook-shell.md` |
| `pass_filenames: <n>` | `pass_filenames: 10` | ≤ n files per invocation | `config/hook.rs:236-296` |
| `language_version` map | `{request: "20", preference: only-managed}`; preferences `only-managed`, `managed`, `system`, `only-system`; semver ranges `>=3.9, <3.13` | toolchain selection & download | `config/hook.rs:301-460` |
| `include_deleted` | `include_deleted: true` | deleted paths (and rename sources) passed; tags from name+git mode | `docs/reference/configuration.md:1134-1200` |
| `hide_status` | `hide_status: [passed, skipped]` | report filtering only | `config/mod.rs:39-81` |
| `orphan` | `orphan: true` | files of this project hidden from ancestor projects | `cli/run/filter.rs:218-283` |
| `minimum_prek_version` | top-level and hook-level string | error if older | `config/mod.rs:285-316` |
| `update` table | `update: {cooldown_days, freeze, include_tags, exclude_tags, repos: {url: {...}}}` | defaults for `prek update` | `config/update.rs` |
| `{hook_repo}` placeholder | `entry: "{hook_repo}/scripts/x.sh"` | expanded per argv element in direct entries/`script` | `hook_entry.rs:13-30` |
| `x-` keys | any level | silently ignored (anchor holders) | `settings.rs:23-26` |
| `alias` shown in output | (pre-commit has `alias`) | `- hook alias:` detail line | `cli/run/run.rs:1110-1116` |
| workspace mode | nested config files, `.prekignore` | §2.3 | `docs/reference/workspace.md` |
| selectors | `hook`, `dir/`, `dir:hook`, `:id:with:colons`, `.` ; `--skip`; `PREK_SKIP` > `SKIP` (not merged) | §2.3 | `cli/run/selector.rs` |
| extra languages | bun, deno, dotnet, julia, mise, php | | `docs/reference/language-support.md` |
| CLI | `init`, `prepare-hooks` (=`install-hooks`), `exec`, `list` (`--output-format json`), `update` (=`autoupdate`), `cache {dir,gc,clean,size}`, `util {identify,list-builtins,init-template-dir,yaml-to-toml,generate-shell-completion}`, `self update`; run flags `--dry-run`, `--last-commit`, `--directory/-d`, `--glob`, `--no-fail-fast`, `--hide-status/--no-hide-status`, `--group…`; globals `-C/--cd`, `--refresh`, `--no-progress`, `--log-file`, `-q/-qq` | | `docs/reference/cli.md` |
| `--files` accepts multiple values per flag | `--files a b c` | (pre-commit: `--files` is `nargs='*'` too) | #825 |
| builtin-only args | `--check` on fixers; `mixed-line-ending --fix=cr`; `check-jsonc --allow-trailing-commas`; `check-yaml --disallow-unknown-tags` | | `docs/reference/built-in-hooks.md` |

Not implemented: `pre-commit hazmat` (4.5.0), `migrate-config` (replaced by `util yaml-to-toml`),
`help <cmd>` subcommand.

Environment variables (`crates/prek-consts/src/env_vars.rs:72-216`,
`docs/reference/environment-variables.md`): `PREK_HOME`, `PREK_COLOR`, `PREK_QUIET`, `PREK_SKIP`,
`PREK_ALLOW_NO_CONFIG` (fallback `PRE_COMMIT_ALLOW_NO_CONFIG`), `PREK_NO_CONCURRENCY` (fallback
`PRE_COMMIT_NO_CONCURRENCY`), `PREK_CONCURRENT_HOOKS`, `PREK_CONCURRENT_BATCHES` (legacy
`PREK_MAX_CONCURRENCY`), `PREK_NO_FAST_PATH`, `PREK_UV_SOURCE`, `PREK_NATIVE_TLS`,
`PREK_DOWNLOAD_CHECKSUM_POLICY` (`warn-missing|required|disabled`), `PREK_CONTAINER_RUNTIME`,
`PREK_DOCKER_NO_INIT`, `PREK_RUBY_MIRROR`, `PREK_CONDA_INSTALLER`, `PREK_RUST_PROFILE`,
`PREK_USE_CARGO_BINSTALL`, `SKIP`, `CI`, `GITHUB_TOKEN`, proxies, `SSL_CERT_*`, `RUST_LOG`.
**Not honored:** `PRE_COMMIT_HOME`, `PRE_COMMIT_COLOR`, `PRE_COMMIT_RUNNING_LEGACY` (uses
`PREK_RUNNING_LEGACY`), `_PRE_COMMIT_SKIP_POST_CHECKOUT` (uses `PREK_INTERNAL__SKIP_POST_CHECKOUT`),
`XDG_CACHE_HOME` for the store (etcetera does honor it on Linux). Exported to hooks: `PRE_COMMIT=1`,
`PRE_COMMIT_FROM_REF/TO_REF/ORIGIN/SOURCE`, `PRE_COMMIT_COMMIT_MSG_SOURCE`, `PRE_COMMIT_COMMIT_OBJECT_NAME`,
`PRE_COMMIT_PRE_REBASE_UPSTREAM/BRANCH`, `PRE_COMMIT_LOCAL_BRANCH/REMOTE_BRANCH`,
`PRE_COMMIT_REMOTE_NAME/REMOTE_URL`, `PRE_COMMIT_CHECKOUT_TYPE`, `PRE_COMMIT_SQUASH_MERGE`,
`PRE_COMMIT_REWRITE_COMMAND`. Booleans parse `y|yes|t|true|on|1` / `n|no|f|false|off|0` (case-insensitive);
most flags are "set = on" regardless of value (`is_set`).

---

## 5. Documented incompatibilities and pitfalls (docs + issue tracker)

### 5.1 Behavioural divergences (cross-checked)

| Area | prek | pre-commit | Evidence |
|---|---|---|---|
| Exit code for config/operational errors | 2 | 1 | `cli/mod.rs:76-86`; verified pre-commit 4.1.0 |
| Error format | `error: …` / `  caused by: …` | `An error has occurred: <Type>: …` + `Check the log at <home>/pre-commit.log` | verified |
| Config error detail | saphyr/serde message with snippet | cfgv breadcrumbs `==> File …`, `==> At Config()`, `==> At key: repos`, `=====> Missing required key: name` | verified |
| Unknown hook id | `error: No hooks found after filtering with the given selectors` (+ warnings) | `No hook with id `x` in stage `pre-commit`` (exit 1) | verified |
| Stash messages | stderr, different wording | stdout `[WARNING]/[INFO]` | §2.9 |
| Running from a subdir with its own config | that config becomes workspace root | pre-commit always uses `<git root>/.pre-commit-config.yaml` | §2.3, #1061, #1739 |
| Nested configs | become projects automatically | ignored | #1739, #1044 |
| `.pre-commit-config.yml`, `prek.toml` | read | not read | |
| File identification | extension wins over filename prefix (`makefile.png` → png only); `.jsonc` tagged `jsonc`; `.cts/.mts` tagged `ts`; earlier missed `.pylintrc` → `pylintrc, ini` (#2398) | `identify` | `docs/diff.md:20-24`, `crates/prek-identify/gen.py` |
| `log_file` | not echoed to terminal; relative to config dir | echoed and appended; relative to CWD (= git root) | §2.12 |
| Batch exit code | bitwise OR | max by abs | §2.10 |
| Filename order | fastrand shuffle, always | MT19937 shuffle, **skipped when `require_serial: true`** | §2.10 |
| Env installation scope | only hooks that will run | all non-skipped hooks of the stage | §2.8 |
| `additional_dependencies` | passed as-is now (was deduped/reordered via `FxHashSet`, #1602) | as-is (can contain pip flags like `--index-url`) | #1602 |
| local-repo install CWD | was the user repo; now an empty temp dir | pseudo repo in store with placeholder `setup.py` | #1603 |
| Autoupdate tag choice | newest by timestamp | `git describe --tags --abbrev=0` | #1533 |
| `core.hooksPath` | honored if repo/worktree scope | refuse if set anywhere | §2.13 |
| Existing pre-commit shim | treated as foreign → `.legacy` + double run | n/a | #1620, #1621 |
| Diff scope for modification detection | `git diff --full-index … -- <project dir>` | whole repo `git diff --no-ext-diff --no-textconv --ignore-submodules` | `git.rs:538-570` vs `pc:pre_commit/commands/run.py:274-279` |
| meta hooks `files` | glob over all three config names | regex for `.pre-commit-config.yaml` | `hooks/meta_hooks.rs:60-90` |
| Fast-path hooks | Rust reimplementation regardless of pinned `rev` | the pinned Python code | §5.4 |
| Hook output bytes | lossy UTF-8 + ANSI sanitized + indented | raw | §2.11 |
| Merge-conflict precheck | `U` entries from `git diff --name-status` (same semantics) | `git ls-files --unmerged` non-empty (`pc:pre_commit/commands/run.py:325-349`); different message | §2.8 |
| `minimum_pre_commit_version` | ignored | enforced | `config/mod.rs:170` |
| `--files` with a missing path | `warning: This file does not exist and will be ignored: `<f>`` (`cli/run/filter.rs:619-632`) | silently dropped (`Classifier` keeps only `os.path.lexists`, `pc:pre_commit/commands/run.py:73-75`) | |
| Tag computation | eager for all candidate files (rayon) | lazy, `functools.cache` per file (`pc:pre_commit/commands/run.py:77-78`) | §2.8 |
| sensible-regex / deprecated stage warnings | absent | present | §2.2 |

### 5.2 YAML

* History: serde_yaml → serde-saphyr (0.2.15) → reverted to serde_yaml (0.2.17, #1102/#1104/#1111) →
  serde-saphyr again (later) with `granit-parser` for check-yaml; fixes for merge keys (#1019), flow
  indentation (0.5.3), unknown tags (0.5.1), `.nan` (#2544), comment-heavy files (#2501), alias budgets
  (#1838). Lesson: Rust YAML crates are *stricter* than PyYAML/ruamel and their DoS budgets reject real
  files. A drop-in must reproduce **PyYAML SafeLoader (YAML 1.1)** for configs/manifests and
  **ruamel.yaml** semantics for the `check-yaml` builtin. PyYAML implicit typing, verified with the
  PyYAML inside pre-commit 4.1.0's venv:

  | scalar | PyYAML `safe_load` |
  |---|---|
  | `yes` `Yes` `YES` `on` `true` `TRUE` | `True` (also `no/off/false` → `False`, 3 casings each) |
  | `y`, `Y`, `tRue` | strings |
  | `0x1F` / `017` / `0b101` / `1_000` / `+12` / `1:20` | 31 / 15 (octal) / 5 / 1000 / 12 / 80 (sexagesimal) |
  | `0o17` | **string** (YAML 1.2 octal not recognized) |
  | `1.0`, `.inf`, `.NaN` | float |
  | `~`, `null`, empty | `None` |
  | `2024-01-01` | `datetime.date` (→ cfgv type error for string fields) |
  | duplicate keys `a: 1\na: 2` | `{'a': 2}` (last wins, silent) |

### 5.3 Regex: Python `re` vs Rust (measured, `scratchpad/rxtest`, fancy-regex 0.19.2 / regex 1.13.1 vs CPython 3.12)

prek compiles `files`/`exclude` with `fancy_regex::RegexBuilder::bytes_mode(BytesMode::UnicodeBytes)` and
matches against path bytes; **a match-time error (backtrack limit) is treated as "no match"**
(`config/pattern.rs:193-208`). Builtin `deny-pattern`/`require-pattern` use fancy-regex `RegexSet`
(lookaround since 0.5.4, #2731). pygrep runs real Python (§2.5).

| Pattern / input | Python `re.search` | `regex` crate | `fancy-regex` |
|---|---|---|---|
| `(?x)^a[ ]b` / `a b` | True | **compile error** (whitespace ignored inside class) | True |
| `^a{,3}$` / `aa` | True | **error** | True |
| `foo\Z` / `foo` | True | **error** | True |
| `(?<=foo)bar`, `^(?!.*test).*\.py$` | True | **error** (no look-around) | True |
| `(?P<n>a)(?P=n)`, `(?P<n>a)\1` / `aa` | True | **error** | True |
| `(?>a)`, `a++` | True (3.11+) | error / True | True |
| `foo$` / `foo\n` | **True** (`$` matches before final `\n`) | False | **False** |
| `^[[:alpha:]]+$` / `abc` | **False** (nested-set literal + FutureWarning) | True | **True** |
| `a(?i)b` | **error** "global flags not at the start" (3.11+) | True | **True** |
| `\e` | **error** bad escape | error | compiles (no error) |
| `\N{LATIN CAPITAL LETTER A}` / `A` | True | error | **False** |
| `(?a)\w` / `é` | False (ASCII mode) | error | **error** |
| `\d` / `٣` | True (str pattern) | True | True |

pygrep specifics (`pc:pre_commit/languages/pygrep.py:22-123`): **bytes** pattern (`pattern.encode()`) →
ASCII-only `\w\d\s\b` and ASCII case folding; files opened `rb`; by-line iteration yields lines **including**
the trailing `\n`; `--multiline` adds `re.MULTILINE | re.DOTALL`; `-i` adds `IGNORECASE`.

**snp:** implement a Python-`re` → fancy-regex translator (parse Python syntax, emit fancy-regex):
`$` (non-MULTILINE) → `(?=\n?\z)`; `\Z` → `\z`; `{,n}` → `{0,n}`; `[[:` → literal `\[\[:`; `\N{name}` →
codepoint; `(?a)`/bytes patterns → `(?-u)` ASCII classes; validate Python-only errors (global flag
position, bad escapes like `\e`, `(?L)` on str) and reject exactly as Python would; keep a hard
match-time error path (never silently "no match"). Use plain `regex` when the translated pattern needs no
fancy features (fancy-regex already delegates).

### 5.4 Fast path / builtin parity bugs (from issues)

| Issue | Hook | Divergence |
|---|---|---|
| #837 | trailing-whitespace | ran on binary files (manifest `types: [text]` ignored early on) |
| #309 | end-of-file-fixer | replaced final CRLF with LF |
| #2604, #1032, #2674 | check-yaml | accepted unknown tags (`!reference`), different YAML 1.1/1.2 handling; now needs `--disallow-unknown-tags` to match |
| #2824 | check-json | accepted empty files; trailing data |
| #2825 | check-added-large-files | rounded size down (1023 bytes over limit passed) |
| #2365 | requirements-txt-fixer | sort order with versions |
| #2207 | pretty-format-json | CRLF not preserved (fast path still disabled for it) |
| #2213, #928 | shebang checks | Windows exec bit; prek consults git index mode |
| #730 | check-added-large-files | hang (stdin/stdout pipe deadlock with `git check-attr`) |
| #1235, #1130 | shebang checks / prettier | Windows command-line length (8191 for cmd.exe) |
| #2687 | check-hooks-apply | misreported builtin hooks |

Fast-path detection ignores `rev` (`hooks/pre_commit_hooks/mod.rs:218-221`, `// TODO: compare rev`) and
matches only the exact URL string (no `.git` suffix, no `git@github.com:` form). It still clones the repo
and installs the Python venv even though the venv is never used (`docs/built-in-hooks.md` note;
`hook.needs_install_env()` is true for remote python hooks).

### 5.5 Git environment leakage (critical)

* #2505: during `git commit` in a **linked worktree**, git exports absolute `GIT_DIR`/`GIT_INDEX_FILE`;
  prek passed them to npm, whose internal `git clone` wrote the hook repo tree into the **user's index**.
* #1354: `GIT_DIR` leaked into `uv pip install` → setuptools_scm saw the wrong repo.
* #1028: user `init.defaultObjectFormat=sha256` broke prek's `git init` for clones (now
  `-c init.defaultObjectFormat=`).
* pre-commit strips git env for its own git calls (`no_git_env()`), and prek now sanitizes clone/update
  and some installers (changelog 0.4.13 "Isolate dependency installs from inherited Git repository state").

pre-commit's `no_git_env()` (`pc:pre_commit/git.py:27-48`) is an **allowlist**: it drops every `GIT_*`
variable except `GIT_EXEC_PATH, GIT_SSH, GIT_SSH_COMMAND, GIT_SSL_CAINFO, GIT_SSL_NO_VERIFY,
GIT_CONFIG_COUNT, GIT_CONFIG_KEY_*, GIT_CONFIG_VALUE_*, GIT_HTTP_PROXY_AUTHMETHOD, GIT_ALLOW_PROTOCOL,
GIT_ASKPASS`; it is applied to its git clone/fetch helpers (`git.py:190,197`, `store.py:200`) and the
golang installer (`languages/golang.py:145`) — but not to e.g. pip/npm installs. prek uses a **denylist**
(§2.7), which is weaker against unknown variables (e.g. `GIT_QUARANTINE_PATH`, `GIT_NAMESPACE`).

**snp:** every subprocess that is *not* the hook itself (clones, installers, toolchain probes, npm, uv,
cargo, go, gem…) must run with pre-commit's allowlist filter (plus `GIT_CONFIG_PARAMETERS`, which git uses
for `-c` passthrough — decide explicitly) and an isolated CWD; hooks themselves get the user env unchanged
(pre-commit parity) plus `PRE_COMMIT*` vars.

### 5.6 Windows pitfalls

* `Command::new("npm")` doesn't find `npm.cmd` — need PATHEXT-aware lookup (`which`) (#1382).
* Case-insensitive path prefix comparisons (`--cd` recorded with user's casing, #1850); mapped/network
  drives vs `git rev-parse --show-toplevel` (#1875, #2113) — compare with `same_file`/canonicalization,
  never string prefixes.
* Rename failures due to AV scanners → retry with backoff (~10 s total) (`fs.rs:54-95`).
* Console VT mode reset by child processes breaks colors (#1237, #950) → re-enable VT before each flush.
* Cross-volume temp dirs break `rename` (#505, #883) → keep scratch inside the store.
* `cmd.exe` 8191-char limit for `.cmd/.bat` (#1130, #1235).
* Every file treated as executable on Windows for compatibility (changelog 0.2.13).
* Ctrl-C exit code `0xC000013A`.
* Antivirus false positive on release binary (#2363); corporate security stacks blocking hook install (#2755).

### 5.7 Concurrency and scheduling pitfalls

* #2272: total processes = hooks-in-group × batches-per-hook (each up to CPU count) → 2× oversubscription.
  prek split knobs (`PREK_CONCURRENT_HOOKS`, `PREK_CONCURRENT_BATCHES`) but kept the product.
* Same-priority hooks that modify the same files → undefined results; modifications can't be attributed
  to one hook (group failure tree).
* `require_serial` only serializes batches of that hook, not exclusivity.
* `fail_fast` within a parallel group waits for the group.

### 5.8 Panics / crashes reported

`run.rs:231` overlong filename panic (#1337, by design "instead of silently dropping files"); capacity
overflow in dots formatting (#1625); indicatif `PoisonError` / `multi.rs` panics (#1625, #2289);
`parse_shebang` empty vec (#925); `Fix panic when parse config failed` (0.2.5); hang under Zed (#718,
defunct git child). With `panic = "abort"` none of these restore the stash.

### 5.9 Other notable issues

* #1300 (fixed): no hooks for requested stage → error; must be silent exit 0.
* #1154 (fixed): stdin must be `/dev/null` for hooks.
* #2382 (fixed): stdout/stderr interleaving must be preserved (single pipe).
* #2127 (fixed): "files modified" false positive when a hook rewrote identical content (stat-only change)
  → compare content diffs, not `git diff --quiet` with stale stat info.
* #2208 (fixed): `git diff` fails in blobless/shallow CI checkouts → tolerate.
* #2048/#2042 (fixed): commit-msg file must bypass project ownership filtering but still honor hook
  filters; message file passed relative to project dir.
* #1739/#1055: users confused by cross-project file visibility.
* #2842 (open): pre-push runs hooks only for first ref (inherited from pre-commit).
* #1173: auth-failing clone retried in a loop-like manner (UX).

---

## 6. Crate choices (versions as of 2026-10-07 from crates.io)

### 6.1 Worth reusing

| Crate | prek uses | Latest | Use in snp for | Notes |
|---|---|---|---|---|
| `clap` (+`derive`,`env`,`wrap_help`) | 4.6.0 | 4.6.7 | CLI | pre-commit's argparse accepts unique long-option prefixes (verified: `pre-commit run --all` == `--all-files`); enable clap `infer_long_args(true)` for parity — prek does not |
| `clap_complete` (`unstable-dynamic`) | 4.6.0 | — | completion | |
| `anstream` / `anstyle` / `anstyle-query` | 1.0 | 1.0.0 | color + Windows VT | honors `NO_COLOR`, `CLICOLOR(_FORCE)` |
| `tokio` (`rt`,`process`,`io-util`,`fs`,`time`,`sync`,`macros`,`net`) | 1.47.1 | 1.53.2 | process orchestration | current-thread runtime is enough |
| `fancy-regex` | 0.19.0 | 0.19.2 | Python-compatible regex backend | wrap with translator (§5.3) |
| `regex` | 1.11 | 1.13.1 | fast path for simple patterns | |
| `globset` | 0.4.18 | 0.4.20 | glob extension, `--glob` | set `literal_separator(true)` deliberately |
| `ignore` | 0.4.23 | 0.4.33 | optional workspace discovery | |
| `serde`, `serde_json` | 1.x | 1.0.151 | state files, JSON output | |
| `toml` / `toml_edit` | 1.0.1 / 0.25.1 | 1.1.6 / 0.25.15 | native TOML config (if any), round-trip edits | TOML 1.1 support |
| `fs-err` | 3.3.0 | 3.3.2 | path-annotated IO errors | |
| `tempfile` | 3.25 | 3.27.0 | temp dirs/files | |
| `thiserror` 2 / `anyhow` 1 | | 2.0.21 / 1.0.104 | errors | |
| `which` | 8.0.0 | 8.0.6 | PATH/PATHEXT lookup | |
| `rustc-hash` | 2.1.1 | 2.1.3 | in-memory maps | not for persistent keys |
| `memchr`, `bstr`, `simdutf8`, `aho-corasick` | | 2.8.3 / 1.13.1 / 0.1.5 / 1.1.5 | builtin hooks scanning | |
| `rayon` | 1.11 | 1.12.0 | CPU-bound builtins | |
| `semver` | 1.0 | — | versions | |
| `similar` | 3.0 | 3.2.0 | diffs in `update`/dry-run | |
| `tracing` + `tracing-subscriber` | 0.1 / 0.3 | 0.1.44 | logs | |
| `same-file`, `dunce` | | | Windows-safe path identity/canonicalize | |
| `rustix` (`pty`,`process`,`fs`,`termios`) | 1.0.8 | 1.1.5 | **safe** PTY (`openpt/grantpt/unlockpt/ptsname`), rlimits, fcntl | lets snp avoid `unsafe` |
| `shlex` | 2.0.0 | 2.0.1 | **do not use for `entry`** (see 6.2) | |
| `phf` | 0.14 | — | static tag tables | |
| `serde-saphyr` / `granit-parser` | 1.2.0 / 1.2.1 | 1.3.0 / 1.3.0 | candidate YAML event parser with spans | must add PyYAML 1.1 resolver layer and disable budgets |
| `cfgv` (user crate) | — | 0.2.0 | pre-commit-exact validation & error breadcrumbs | |
| `file-identify` (user crate) | — | 0.4.0 | tags | add bitset `TagSet` + generated tag ids like prek-identify |
| `blake3` / `xxhash-rust` | — | 1.8.7 / 0.8.19 | stable on-disk keys / fast content hashes | replaces seahash/DefaultHasher |

### 6.2 Avoid or handle with care

| Crate / API | Why |
|---|---|
| `serde_yaml`, `serde_yml` | deprecated/unmaintained (prek abandoned serde_yaml) |
| `regex` alone for config patterns | rejects lookaround, `\Z`, `{,n}`, `(?P=…)`, backrefs (table §5.3) |
| `shlex` 2.0.1 for `entry` | **measured**: treats `#` as comment (`a # b` → `["a"]`; Python → `['a','#','b']`); in double quotes `\$` → `$` (Python keeps `\$`). Write a Python-`shlex.split` (POSIX, comments=False) clone |
| `std::collections::hash_map::DefaultHasher` for anything on disk | unspecified across Rust releases (#1994) |
| `seahash` 64-bit for repo keys | not collision-resistant; prefer 128/256-bit (blake3) |
| `libc::sysconf`, `std::env::set_var`, `OsString::from_encoded_bytes_unchecked` | `unsafe`; avoidable (constant ARG_MAX clamp like pre-commit; explicit per-command env; `OsStrExt::from_bytes` on Unix / owned `Vec<u8>` path type) |
| vendored `pty-process` with `unsafe` | rustix's safe pty API + `tokio::io::unix::AsyncFd<OwnedFd>` suffice |
| `async-trait` with `dyn` backends | native `async fn` + enum dispatch avoids boxing/`?Send` gymnastics |
| `indicatif` progress by default | GUI-git garbling, panics; off by default, TTY-only opt-in |
| `ctrlc` handler doing git work on its thread | race with main thread; use a signal → cancellation token → orderly restore on the main task; second Ctrl-C = abort with restore |
| `axoupdater` self-update | supply-chain surface; package managers update binaries |
| `reqwest` + TLS stack in the core binary | big; feature-gate toolchain downloads |

---

## 7. What snp should do better (concrete list)

Correctness / drop-in fidelity
1. **Default = pre-commit semantics**: config only from `--config` or `<git root>/.pre-commit-config.yaml`
   (CWD-independent), no implicit workspace mode, no `.yml`/`prek.toml` discovery unless opted in
   (e.g. `snp.toml`/CLI flag/`SNP_WORKSPACE=1`). Nested configs never auto-run.
2. Exit codes, messages, streams and colors byte-identical to pre-commit (golden tests against pre-commit
   4.6.x output); extras (progress, alias/description lines, project headers) only behind opt-in flags.
3. Reproduce pre-commit's filename shuffle exactly (MT19937 `random.Random(1542676187)` +
   `random.shuffle`, which uses `_randbelow` via `getrandbits`) and batch exit-code reduction (max |code|).
4. Python-compatible `shlex.split` and Python-`re` translation layer (§5.3); bytes/ASCII mode for pygrep;
   implement pygrep natively (no Python needed) with exact line-with-newline semantics.
5. PyYAML-1.1-faithful config loading + cfgv-rs validation with identical breadcrumb errors and warnings
   (unknown keys at root/repo level only, sensible-regex warnings, deprecated stage names, mutable rev,
   `minimum_pre_commit_version` honored against a declared compatibility version).
6. Recognize pre-commit's shim hashes (current `138fd403232d2ddd5efb44317e38bf03` + priors) as "ours" so
   install/uninstall/migration (`.legacy`) behave exactly like pre-commit and never double-run.
7. Fast path: opt-in or gated by verified `rev` ranges per hook, URL normalization (`.git`, trailing `/`,
   ssh form), differential test-suite against the real hooks; never install the unused venv for fast-path
   hooks (lazy fallback).
8. `include_deleted`, `--dry-run`, etc. are fine as extensions but must not change defaults.

Safety
9. Stash: capture `tree` (write-tree) **and** keep the patch; on restore conflict restore index from the
   captured tree (`git read-tree`/`reset <tree> -- .`), checkout that tree, apply patch; **verify** the
   resulting `git diff-index --binary <tree>` equals the saved patch; on any mismatch stop with an explicit
   recovery message and keep the patch forever (never GC un-restored patches). Register restoration
   before mutating; restore on SIGINT/SIGTERM/SIGHUP and on panic (unwind).
10. Env isolation for all non-hook subprocesses (remove repo-local `GIT_*`, set `GIT_TERMINAL_PROMPT=0`
    for clones, `LC_ALL=C` for parsed git output, `-c init.defaultObjectFormat=`/`--template=` for
    `git init`), installs in an empty temp CWD.
11. Zero `unsafe`, zero panics (`#![forbid(unsafe_code)]`, clippy `unwrap_used/expect_used/panic/indexing_slicing`
    deny in non-test code), overlong filename → clear error naming the file (or run it alone and report).
12. Persistent keys via blake3 (repo dir = blake3(normalized url ‖ \0 ‖ rev)); atomic writes (temp + rename
    + fsync dir) for markers/state; markers written before rename into place, or "complete" marker last.
13. Regex match errors are hard errors with the pattern and file, not "no match".
14. Nested/selected config parse errors always reported (user-visible warning or error).

Performance
15. **Global job-slot pool**: one semaphore bounding total concurrent child processes (default = CPUs),
    shared by hooks and batches (fixes #2272); optional GNU make jobserver client.
16. Lazy, tiered file tagging: filename/extension tags (no I/O) → `lstat` tags (file/symlink/executable)
    → content tags (`text`/`binary`, shebang) computed only if some hook's `types/types_or/exclude_types`
    needs them; memoize per file (`OnceLock` per index).
17. Stream diffs into a hasher (blake3) instead of buffering whole patches; compare hashes for
    modification detection; keep pre-commit's whole-repo diff scope.
18. Content-addressed env index (requirement hash → env dir) for O(1) lookup, falling back to a scan only
    for range requests; persist health-check results with toolchain mtime.
19. Avoid `prek`'s eager full-repo `ls-files` + tagging when only `always_run`/`pass_filenames:false`
    hooks run (copy P17) and when hook filters can be pre-evaluated on paths only.
20. Paths as `bstr`/`Vec<u8>` in an arena (one allocation for the NUL-separated git output, `&[u8]` slices
    per file) instead of one `PathBuf` per file.

UX (lefthook-inspired, opt-in)
21. Progress UI only when stderr is a real TTY and `TERM != dumb`, disabled under `CI`, never in
    GUI-git contexts; single-line summary mode.
22. Optional parallel groups (prek `priority` syntax compatible, so prek configs keep working), `groups`,
    `env`, glob filters, `exec`, `list`, `--dry-run` — all off-by-default extensions; read prek extension
    keys in YAML so prek users can switch to snp.

---

## 8. Implementation notes for snp (Rust-oriented)

Architecture
* Crates: `snp-cli` (bin), `snp-core` (config model, scheduler, run pipeline), `snp-git` (all git
  invocations, typed outputs), `snp-store`, `snp-lang` (one module per language; `enum Language` with
  `match` dispatch, no `dyn`), `snp-hooks` (builtins), reuse `cfgv` and `file-identify`.
* Runtime: tokio current-thread is sufficient (prek proves it); CPU-bound work via `spawn_blocking` +
  rayon; non-`Send` futures OK (use `Rc` + `tokio::sync::Semaphore` or `LocalSet`). Keep all repo-mutating
  steps (stash/restore) synchronous and guarded.
* Model config with a span-carrying YAML node tree (event parser) → typed structs via cfgv-rs schema, so
  errors can be both pre-commit-exact (breadcrumbs) and optionally pretty (spans).
* Hook identity: `HookKey { project: u32, idx: u32 }`; stages as `u16` bitset; tags as bitset.
* Commands: a `GitCmd` builder that always adds `-c core.useBuiltinFSMonitor=false` (verify pre-commit
  doesn't need it for parity; it's invisible), NUL-delimited outputs parsed from bytes, `LC_ALL=C` when
  parsing stderr.

Processes
* Single pipe for stdout+stderr (`std::io::pipe()` stable since 1.87), stdin `Stdio::null()`, PTY only when
  color is on and not Windows; drain after exit; treat Linux `EIO` on PTY master as EOF.
* Pass hook env explicitly: `cmd.envs(base_env).env("PRE_COMMIT","1")…` — never mutate the process env
  (edition-2024 `set_var` is `unsafe` and racy with concurrent spawns).
* ARG_MAX: replicate pre-commit exactly: `clamp(SC_ARG_MAX - 2048 - env_size, 2**12, 2**17)`; since the
  upper clamp is 128 KiB and mainstream Unixes report `ARG_MAX ≥ 128 KiB` (Linux ≥ 128 KiB, macOS 1 MiB;
  verify others) a constant `2**17 - 2048 - env_size` floored at `2**12` is equivalent without
  `sysconf` (or read it safely via `getconf ARG_MAX` once and cache); Windows `2**15 - 2048`; `.cmd/.bat`: `8192 - len(cmd_exe) - len(" /c ") - 1024`
  (`pc:pre_commit/xargs.py:49-58,160-166`). Also respect Linux `MAX_ARG_STRLEN` (128 KiB per single arg).
* Windows command resolution: PATHEXT via `which`; shebang emulation (pre-commit `parse_shebang`).
* Filename shuffle parity (pre-commit `_shuffled`, `pc:pre_commit/lang_base.py:146-153`): implement
  MT19937 with CPython seeding (`init_by_array([1542676187])`, int seeds ignore `version`), and
  `random.shuffle`: `for i in (1..n).rev() { j = randbelow(i+1); swap(i, j) }` with
  `randbelow(n) { k = bit_length(n); loop { r = getrandbits(k); if r < n { return r } } }` and
  `getrandbits(k≤32) = genrand_uint32() >> (32-k)`. Test vectors (CPython 3.12, verified):
  first three `getrandbits(32)` = `2472326011, 2061721674, 2625518588`;
  `shuffle(range(10))` → `[4, 0, 5, 1, 8, 6, 2, 3, 7, 9]`;
  `shuffle(['a.py','b.py','c.py','d.py','e.py'])` → `['b.py','a.py','c.py','d.py','e.py']`.
  Shuffle happens per hook on the filtered filename list before partitioning, and **only when
  `require_serial` is false** (`pc:pre_commit/lang_base.py:156-172`); prek shuffles unconditionally
  (`cli/run/run.rs:1600`).

Stash (exact pre-commit algorithm + hardening)
* Intent-to-add detection in pre-commit: `git diff --no-ext-diff --ignore-submodules --diff-filter=A
  --name-only -z` (worktree vs index) → `git rm --cached -- <paths>` (via xargs) before, and
  `git add --intent-to-add -- <paths>` after (`pc:pre_commit/git.py:146-151`,
  `pc:pre_commit/staged_files_only.py:36-46`); message `[WARNING] Unstaged intent-to-add files detected.`
* `tree=$(git write-tree)`; `git diff-index --ignore-submodules --binary --exit-code --no-color --no-ext-diff
  <tree> --`; write patch to `<store>/patch<epoch>-<pid>`; `git -c submodule.recurse=0 checkout -- .`;
  run; `git apply --whitespace=nowarn <patch>`; on failure roll back to `<tree>` and re-apply; verify.
* Hold restoration state in a guard registered with the signal handler before the first mutation.

Scheduling
* Pre-commit mode: strictly sequential hooks; batches parallel up to `cpu_count()` (1 if `require_serial`
  or `PRE_COMMIT_NO_CONCURRENCY` is set; **2 if `TRAVIS` is set**) (`pc:pre_commit/lang_base.py:135-143`).
* Extension mode: priority groups; one global semaphore for all child processes.
* Modification detection: builtins report precisely; external hooks → hash of
  `git diff --no-ext-diff --no-textconv --ignore-submodules` (whole repo, untracked files excluded, as in
  pre-commit) before/after; skip the "before" diff when the previous state is known (right after the
  stash the unstaged diff is empty; after a group that reported its own modifications the baseline is
  unknown and must be re-captured).

Store
* Decide between pre-commit-compatible store (`$PRE_COMMIT_HOME` / `$XDG_CACHE_HOME/pre-commit`, `db.db`
  sqlite schema, `repo<random>` dirs) vs own store; prek chose its own (`~/.cache/prek`). If snp shares
  pre-commit's store, envs created by Python pre-commit (virtualenv layout) and by snp must be mutually
  readable — likely impractical; recommend own store `~/.cache/snp` + honoring `SNP_HOME`, and reading
  `PRE_COMMIT_HOME` only for `clean`/`gc` compatibility commands (open question).
* Use `std::fs::File::lock`/`try_lock` (stable) for the store lock; print a waiting message after 1 s.

Testing
* Differential harness: run pre-commit 4.6.x and snp on generated repos/configs, compare stdout bytes,
  exit codes, git state (index tree, worktree diff) — especially for stash conflict scenarios from
  #2801/#2142/#1889 and the linked-worktree env scenario from #2505.
* Property tests for shlex, regex translation, xargs partitioning (never drop/duplicate a file, never
  exceed limit), YAML resolver (PyYAML corpus).

---

## 9. Open questions

1. **Workspace mode**: should snp support prek-style nested projects at all, and if so, how is it enabled
   (CLI flag, top-level key in root config, presence of `snp.toml`)? Should it honor `.prekignore`?
2. **Config formats**: read `prek.toml` / `snp.toml` natively? Accept prek-only YAML keys (`priority`,
   `groups`, `env`, glob `files`, `repo: builtin`) silently, with a warning, or only in extension mode?
3. **Fast path policy**: default-on (prek) vs opt-in vs rev-gated? Divergences (#2604, #2824, #2825) show
   default-on breaks the "exact drop-in" promise.
4. **Store sharing** with Python pre-commit (`PRE_COMMIT_HOME`, `db.db`) vs separate store; GC semantics.
5. **pygrep**: fully native with Python-`re` emulation, or fall back to a system Python for patterns the
   translator can't prove equivalent?
6. **Python envs**: use `uv` (fast, prek's choice; but `UV_*` leakage #1299 and requires-python
   inference) or `python -m venv`/`virtualenv` exactly like pre-commit for parity? Default
   `language_version` for python without pre-commit's own interpreter.
7. **`minimum_pre_commit_version`**: which pre-commit version does snp claim (e.g. "4.6.2-compatible")?
8. **Exit code 2** for snp-internal errors: never, or only with an opt-in flag?
9. **Shim**: write a pre-commit-identical shim (bash script with `INSTALL_PYTHON` fallback) so switching
   back to Python pre-commit works, or an snp shim (`/bin/sh`, absolute path + PATH fallback like prek)?
   Recognize prek's shim hash `182c10f181da4464a3eec51b83331688` too?
10. **Pre-push multi-ref** (#2842): keep pre-commit's first-ref behaviour (parity) or run per ref
    (better)? Probably parity by default + opt-in.
11. **globset `literal_separator`**: if snp accepts prek's `glob:` syntax, must it replicate prek's
    `*`-crosses-`/` semantics for compatibility with existing prek configs?
12. Diff scope: keep pre-commit's whole-repo `git diff` for modification detection in pre-commit mode, but
    allow prek-style per-project scoping (cheaper in monorepos) in workspace mode? (Merge-conflict
    precheck parity is already confirmed: both check only when stashing, i.e. not `--all-files`/`--files`.)
13. Whether to honor `PREK_*` env vars as aliases (helps users migrating from prek) — e.g. `PREK_SKIP`.
