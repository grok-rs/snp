# 04a: pre-commit language backends, part 1 (core languages)

This spec covers **python, node, golang, rust, ruby, docker, docker_image, unsupported (`system`), unsupported_script (`script`), fail, pygrep**. It also covers the shared infrastructure these backends depend on: `lang_base.py`, `prefix.py`, `envcontext.py`, `parse_shebang.py`, the language-related parts of `repository.py`, `xargs.py` and `util.py`.

* The other languages and the Rust `Language` trait design are in `04b-languages-extra.md`. Where the two documents overlap (the infrastructure in §1), they must agree; if they disagree, treat it as a bug in one of them.
* It also evaluates using `uv` for Python environments (§11) and specifies a native Rust `pygrep` (§12).

**Contents:**

| § | Topic |
|---|---|
| 0 | Provenance |
| 1 | Shared infrastructure |
| 2 | python |
| 3 | node |
| 4 | golang |
| 5 | rust |
| 6 | ruby |
| 7 | docker and docker_image |
| 8 | unsupported, unsupported_script, fail, pygrep (reference) |
| 9 | Platform matrix |
| 10 | Environment variables |
| 11 | Python via uv |
| 12 | Native pygrep |
| 13 | Ideas from prek and lefthook |
| 14 | Implementation notes for snp |
| 15 | Open questions |

---

## 0. Provenance and verification

* **Reference source:** `pre-commit` main at `c4292384` (`setup.cfg:3` gives `version = 4.6.2`). Paths are cited relative to `refs/pre-commit/`, for example `pre_commit/lang_base.py:135`.
* **Live observation:** the installed `pre-commit 4.1.0` (pipx venv, Python 3.12.3, virtualenv 20.29.2, nodeenv 1.9.1) was run in throwaway repos under `scratchpad/tmp-lang/`. Two techniques were used:
  * A fake `docker` shim placed first on `PATH` logged the exact argv and cwd.
  * An `envdump.py` hook printed the environment that each language exposes to hooks.

  Results marked **(observed)** come from these runs.
* **Differences between 4.1.0 and 4.6.2** in the in-scope code (from `diff` against the installed package):

| Area | 4.1.0 (installed) | 4.6.2 (target) | Since |
|---|---|---|---|
| `lang_base.hook_cmd` | `(*shlex.split(entry), *args)` | Also rewrites a leading `pre-commit hazmat` | 4.5.0 |
| python `get_default_version` | Tries `sys.executable` first, then `pythonX.Y` | Tries `pythonX.Y`, then `pythonX`, then `sys.executable`, then the py launcher | 4.2.0 |
| node `install_environment` | `npm install --include=dev --include=prod --ignore-prepublish --no-progress --no-save` in the prefix, then `npm pack`, then `npm install -g <tgz> *deps`, then `rm -rf node_modules` and the tgz | `npm install --allow-git=root --install-links -g git+file://<prefix> *deps`, and errors if there is no package | 4.6.1 / 4.6.2 |
| local template (`store._LOCAL_RESOURCES`) | Includes `package.json` and `.npmignore` | Neither file | 4.6.1 |
| `empty_template_setup.py` | `setup(name=..., version='0.0.0')` | adds `py_modules=[]` | 4.5.1 |
| docker container detection | `/proc/1/cgroup` contains `docker`, then the `cpuset` line basename | `/proc/1/mountinfo` regex (cgroups v2 and podman) | 4.4.0 |
| docker rootless | None | `docker system info` check; no `-u` when rootless | 4.3.0 |
| language names | `system`, `script` | `unsupported`, `unsupported_script`; `system` and `script` are translated aliases | 4.4.0 |
| `rbenv.tar.gz` / `ruby-build.tar.gz` | older | rbenv `1.3.2`, ruby-build `20251023` | 4.4.0 |

* Everything else in scope is **semantically identical** between the two versions. `prefix.py`, `envcontext.py`, `parse_shebang.py`, `repository.py`, `rust.py`, `ruby.py`, `docker_image.py`, `fail.py` and `pygrep.py` are byte-identical. `xargs.py` differs only in imports; `golang.py` only in `[2:]` vs `.removeprefix('go')`.
* **Target 4.6.2 semantics.** The 4.1.0 observations still hold, except where the table above lists a difference.

---

## 1. Shared infrastructure used by the part-1 languages

### 1.1 The `Language` protocol (`pre_commit/lang_base.py:27-60`)

| Member | Signature | Meaning | Generic implementation (`lang_base.py`) |
|---|---|---|---|
| `ENVIRONMENT_DIR` | `str \| None` | Prefix of the env dir name. `None` means no environment: no install, and the hook runs with prefix = CWD for local hooks. | – |
| `get_default_version()` | `() -> str` | Replaces `'default'` (`C.DEFAULT`, `constants.py:13`) | `basic_get_default_version` returns `'default'` (`:114`) |
| `health_check(prefix, version)` | `-> str \| None` | `None` means healthy. A string means unhealthy, which triggers a silent reinstall. | `basic_health_check` returns `None` (`:118`) |
| `install_environment(prefix, version, deps)` | `-> None` | Creates the env. Raises on failure. | `no_install` raises `AssertionError('This language is not installable')` (`:122-127`) |
| `in_env(prefix, version)` | context manager | Mutates `os.environ` for the duration of `run_hook` | `no_env`, which does nothing (`:130-132`) |
| `run_hook(prefix, entry, args, file_args, *, is_local, require_serial, color)` | `-> (int, bytes)` | Runs the hook and returns the exit code plus combined stdout/stderr | `basic_run_hook` (`:181-196`) |

* `is_local` is `hook.src == 'local'` (`commands/run.py:197`). None of the part-1 languages read it; only `r` and `julia` do.

### 1.2 Registry, aliases, and the part-1 matrix

* **Registry:** `all_languages.py:27-50` lists 21 languages. Config validation translates `system` to `unsupported` and `script` to `unsupported_script` before it checks the language with `check_one_of(language_names)`:
  * `clientlib.py:194-198` defines the translation.
  * It is used for manifest hooks (`clientlib.py:243`), config overrides (`:456`) and local hooks (`LOCAL_HOOK_DICT` reuses the manifest items).
  * The translated name is **stored**. Error messages therefore say `unsupported`, not `system`, in 4.6.2. 4.1.0 said `system`.
* **Removed names:** `python_venv` was removed in 4.0.0 (`CHANGELOG.md:156`). It now fails validation like any unknown language.

| language | ENVIRONMENT_DIR | default version | health_check | install | in_env | run_hook |
|---|---|---|---|---|---|---|
| python | `py_env` | `pythonX.Y` detection (§2.2) | pyvenv.cfg + version probe | virtualenv + `pip install .` | VIRTUAL_ENV, PATH, … | basic |
| node | `node_env` | `system` if `node`+`npm` "exist" (not win32) else `default` | `node --version` | nodeenv + `npm install -g` | NODE_*, npm_config_*, PATH | basic |
| golang | `golangenv` | `system` if `go` "exists" else `default` | none | download go (non-system) + `go install` | PATH (+GOROOT, GOTOOLCHAIN) | basic |
| rust | `rustenv` | `system` if `cargo --version` succeeds else `default` | none | rustup toolchain (non-system) + `cargo install` | PATH (+RUSTUP_TOOLCHAIN) | basic |
| ruby | `rbenv` | `system` if `ruby`+`gem` "exist" else `default` | none | rbenv/ruby-build (non-system) + `gem build`/`gem install` | GEM_HOME, PATH, … | basic |
| docker | `docker` | `default` | none | `docker build --pull` + mkdir | none | `docker run --entrypoint` |
| docker_image | `None` | `default` | none | not installable | none | `docker run <entry>` |
| unsupported (`system`) | `None` | `default` | none | not installable | none | basic |
| unsupported_script (`script`) | `None` | `default` | none | not installable | none | basic, with `entry[0]` made prefix-relative |
| fail | `None` | `default` | none | not installable | none | prints entry + files, returns 1 |
| pygrep | `None` | `default` | none | not installable | none | `sys.executable -m pre_commit.languages.pygrep` |

("exist" means `lang_base.exe_exists`, see §1.7.)

### 1.3 Lifecycle: from config to process (ordering matters)

1. **Merge and version resolution** (`repository.py:109-144`, `_hook`):
   1. Start from the manifest dict (remote) or the local dict and `dict.update` each overriding dict in order.
   2. If `ret['language_version'] == 'default'`, set it to `root_config['default_language_version'][lang]`. That map has one optional key per language, each defaulting to `'default'` (`clientlib.py:491-494`).
   3. If it is still `'default'`, set it to `languages[lang].get_default_version()`. Each backend caches the result for the process lifetime (`functools.lru_cache(maxsize=1)`).
   4. If `languages[lang].ENVIRONMENT_DIR is None`:
      * If `language_version != 'default'`, log an `ERROR` and `exit(1)` with exactly:
        ``The hook `{id}` specifies `language_version` but is using language `{lang}` which does not install an environment.  Perhaps you meant to use a specific language?``
      * Otherwise, if `additional_dependencies` is non-empty, log an `ERROR` and `exit(1)` with exactly:
        ``The hook `{id}` specifies `additional_dependencies` but is using language `{lang}` which does not install an environment.  Perhaps you meant to use a specific language?``

      (Note the **two spaces** after `environment.`.) **(observed)** Output is ``[ERROR] The hook `sys-lv` specifies `language_version` …``, exit 1, and no hook runs. These checks run while *loading* the hooks, before any hook runs.
   5. **Resolution is eager.** `get_default_version()` is called for every hook whose version resolves to `default`, even when the hook is later skipped. Some backends spawn processes here: rust runs `cargo --version` and python may run the py launcher on Windows.
2. **Prefix selection:**
   * **Local hooks** (`repository.py:147-168`): if `ENVIRONMENT_DIR is None`, the prefix is `Prefix(os.getcwd())`, which is the git toplevel because `main.py:189` chdirs there. Otherwise it is `Prefix(store.make_local(additional_dependencies))`. That is a store dir with the key `('local' or 'local:dep1,dep2', '1')`, seeded from the empty template (§1.10).
   * **Remote hooks** (`repository.py:171-200`): the prefix is `Prefix(store.clone(repo, rev, hook['additional_dependencies']))`. A **different deps list gives a different clone dir.** The store key is `repo + ':' + ','.join(deps)` when deps are present (`store.py:126-130`), and deps are no longer sorted (`CHANGELOG.md:347`).
   * An unknown id in a remote repo logs this ERROR and exits 1:
     `` `{id}` is not present in repository {repo}.  Typo? Perhaps it is introduced in a newer version?  Often `pre-commit autoupdate` fixes this.``
3. **Which hooks get installed** (`commands/run.py:422-443`): hooks are selected by the `hook` argument and by stage. Ids or aliases listed in `SKIP` are removed from `to_install`, but they are still "run", which prints Skipped.
4. **`install_hook_envs(hooks, store)`** (`repository.py:214-229`):
   1. `_need_installed()` walks the hooks in order. It dedups by `hook.install_key = (prefix, language, language_version, tuple(additional_dependencies))` (`hook.py:39-46`) and keeps the hooks whose `_hook_installed(hook)` is false.
   2. If none are left, return without taking the lock.
   3. Otherwise take the store lock (`store.exclusive_lock()`, file `<store>/.lock`, logging `Locking pre-commit directory` if it blocks) and **recompute** `_need_installed()`, because another process may have finished the work.
   4. Then call `_hook_install(hook)` sequentially.
5. **`_hook_installed(hook)`** (`repository.py:46-62`):
   * If `ENVIRONMENT_DIR is None`, the hook counts as installed.
   * Otherwise `venv = <prefix>/<ENVIRONMENT_DIR>-<language_version>`. The hook is installed if both of these hold:
     * `<venv>/.install_state_v2` exists, **or** `json.load(<venv>/.install_state_v1) == {'additional_dependencies': deps}`.
     * `health_check(prefix, version)` returns `None`.
   * **The health check runs on every invocation for every distinct hook env.** For python and node that costs a subprocess spawn per env per run (§14.9).
6. **`_hook_install(hook)`** (`repository.py:65-106`):
   1. Log three INFO lines. `hook.src` is the repo URL or `local`:
      * `Installing environment for {hook.src}.`
      * `Once installed this environment will be reused.`
      * `This may take a few minutes...`
   2. If the env dir already exists, `rmtree` it (leftovers of an interrupted install).
   3. Under `clean_path_on_failure(venv)`, which rmtrees on **any** `BaseException` including Ctrl-C (`util.py:27-35`):
      1. `install_environment(prefix, version, deps)`.
      2. `health_check`. If it fails, raise `AssertionError(f'BUG: expected environment for {hook.language} to be healthy immediately after install, please open an issue describing your environment\n\nmore info:\n\n{health_error}')`.
      3. Write `<venv>/.install_state_v1staging` with `json.dumps({'additional_dependencies': deps})`, then `os.replace` it to `.install_state_v1`.
         * The format is Python `json.dumps` with default separators, for example `{"additional_dependencies": ["six"]}` and `{"additional_dependencies": []}` **(observed)**.
      4. Create an empty `<venv>/.install_state_v2` with `open(..., 'a+')`.
   4. If an exception escapes, the env dir has already been removed. The top-level error handler then prints `An unexpected error has occurred: <ExcType>: <str>` and `Check the log at <store>/pre-commit.log`, with **exit code 3**. A `FatalError` would exit 1 and Ctrl-C exits 130 (`error_handler.py:75-81`).
7. **Run** (`commands/run.py:189-201`): `filenames = ()` if `pass_filenames` is false. The run is:
   ```python
   with language.in_env(hook.prefix, hook.language_version):
       retcode, out = language.run_hook(hook.prefix, hook.entry, hook.args, filenames, is_local=hook.src == 'local', require_serial=hook.require_serial, color=use_color)
   ```
   Hooks run **sequentially** in pre-commit, and parallelism only exists inside xargs partitions. `out.strip()` is what gets printed. **cwd is always the repo toplevel** for hook processes.

### 1.4 Env dir naming

`lang_base.environment_dir(prefix, d, language_version) = prefix.path(f'{d}-{language_version}')` (`lang_base.py:90-91`). The name is built from the **resolved** version string, verbatim:

| Example | Env dir |
|---|---|
| default python on a 3.12 host | `py_env-python3.12` **(observed)** |
| `default_language_version: {python: python3.13}` | `py_env-python3.13` |
| python with `language_version: /usr/bin/python3.11` | `py_env-/usr/bin/python3.11`, which normpath turns into **nested dirs** `py_env-/usr/bin/python3.11` |
| node with system node | `node_env-system`; downloaded latest node is `node_env-default` |
| `golangenv-system`, `golangenv-1.22.1`, `rustenv-system`, `rustenv-1.79.0`, `rbenv-system`, `rbenv-3.2.2`, `docker-default` | – |

* **Pitfall: version strings containing path separators** produce nested paths. `prefix.path` uses `os.path.join` + `normpath`, so `language_version: ../../x` escapes the prefix.
* snp must replicate the naming for store compatibility. It must also **reject or sanitize** versions that would leave the prefix (recommendation: reject `..` components and absolute paths that would not be under the prefix after the join; see §14.2).

### 1.5 `Prefix` (`prefix.py:7-18`)

* `path(*parts)` is `os.path.normpath(os.path.join(prefix_dir, *parts))`.
  * It is lexical: no symlink resolution.
  * An absolute part discards the earlier parts. This is how python's `health_check` can do `prefix.path(<absolute bin dir>, 'python')`.
* `exists(*parts)` is `os.path.exists(path(*parts))`.
* `star(end)` returns the `os.listdir(prefix_dir)` names that end with `end`. These are names, not paths, in **unsorted** directory order. ruby uses it for `.gemspec` and `.gem`.

### 1.6 `envcontext` (`envcontext.py:15-62`)

* A patch is an ordered tuple of `(key, value)`. The value is one of:
  * `str`: set the variable.
  * `UNSET`: remove it.
  * a template tuple of `str | Var(name, default='')`: format against the **snapshot taken before any patch is applied** (`before = dict(env)`), not against the progressively patched env.
* On exit the env is fully restored with `env.clear(); env.update(before)`.
* `Var('PATH')` with PATH unset gives `''`, so `(bin, os.pathsep, Var('PATH'))` becomes `"<bin>:"`. The trailing empty entry means "cwd" to execvp.
  * snp should avoid creating the empty entry (deviation: safer).
* pre-commit mutates the **process** env, which is safe because hooks run serially.
  * snp must build a per-child env instead. `std::env::set_var` is `unsafe` in edition 2024, and snp runs things concurrently.

### 1.7 `lang_base` helpers

* **`exe_exists(exe)`** (`:63-83`) is the "is a system toolchain available" heuristic used by node, golang and ruby. It returns `True` only when all of these hold:
  1. `parse_shebang.find_executable(exe)` finds a path `found`.
  2. `found` does not match `SHIMS_RE = r'[/\\]shims[/\\]'`, which excludes rbenv/pyenv/asdf-style shims.
  3. Either:
     * the home dir is `/` (`os.path.dirname(home) == home`), or
     * `os.path.commonpath((found, home)) != home`, meaning the exe is **not under `$HOME`**.

  `home = os.path.expanduser('~')`, which is `$HOME` on POSIX and `%USERPROFILE%` on Windows. A `ValueError` from `commonpath` sets `common = None`, which counts as not-in-home. That covers different drives on Windows and a relative `found` mixed with an absolute home.

  **Consequence (observed):** node from nvm (`~/.config/nvm/...`), go in `~/sdk` or `~/go`, and ruby in `~/.rbenv` are **not** "system". With default versions, pre-commit then **downloads** a toolchain. On this host node is under nvm, so the node default version is `default`, while go at `/usr/local/go` gives `system`.
* **`setup_cmd(prefix, cmd, **kw)`** (`:86-87`) runs `cmd_output_b(*cmd, cwd=prefix.prefix_dir, **kw)` with `check=True`. Install commands run **with cwd = prefix** unless noted otherwise.
* **`assert_version_default(binary, version)`** (`:94-99`) raises `AssertionError` with exactly:
  ``for now, pre-commit requires system-installed {binary} -- you selected `language_version: {version}` ``
  **(observed)** ``An unexpected error has occurred: AssertionError: for now, pre-commit requires system-installed docker -- you selected `language_version: 1.0` ``, exit 3.
* **`assert_no_additional_deps(lang, deps)`** (`:102-111`) raises `AssertionError` with exactly:
  ``for now, pre-commit does not support additional_dependencies for {lang} -- you selected `additional_dependencies: {deps}` ``
  `{deps}` is the Python **repr** of the sequence, for example `['x']`.
* **`target_concurrency()`** (`:135-143`):
  * `'PRE_COMMIT_NO_CONCURRENCY' in os.environ` (any value, **even empty**) gives `1`.
  * Otherwise `'TRAVIS' in os.environ` gives `2`.
  * Otherwise `xargs.cpu_count()`, which is `len(os.sched_getaffinity(0))`, falling back to `multiprocessing.cpu_count()` and then `1`. This uses the affinity mask, **not** the cgroup CPU quota.
* **`_shuffled(seq)`** (`:146-153`): `random.Random()`, then `.seed(1542676187, version=1)`, then `.shuffle(list(seq))`. See §1.9 for the exact algorithm and test vectors.
* **`run_xargs(cmd, file_args, *, require_serial, color)`** (`:156-171`):
  * If `require_serial`: `jobs = 1` and the files are **not shuffled**.
  * Otherwise: shuffle, and `jobs = target_concurrency()`.
  * Then `xargs.xargs(cmd, file_args, target_concurrency=jobs, color=color)`.
* **`hook_cmd(entry, args)`** (`:174-178`):
  1. `cmd = shlex.split(entry)` (POSIX mode, no comments).
  2. If `cmd[:2] == ['pre-commit', 'hazmat']`, replace them with `[sys.executable, '-m', 'pre_commit.commands.hazmat']`.
  3. Return `(*cmd, *args)`.

  Error cases:
  * An unbalanced quote makes `shlex` raise `ValueError: No closing quotation`. That is uncaught and exits 3.
  * An empty entry with no args gives an empty cmd, and `normalize_cmd` then raises `IndexError` (exit 3).
* **`basic_run_hook`** (`:181-196`) is `run_xargs(hook_cmd(entry, args), file_args, require_serial=…, color=…)`.

### 1.8 Executable resolution and shebangs (`parse_shebang.py`)

* **`find_executable(exe, env=None)`** (`:22-43`):
  1. `exe = os.path.normpath(exe)`. If `os.sep` is in it, return it **as is**, without an existence check.
  2. Candidate names: if `PATHEXT` is in the env, try `exe + ext` for each `PATHEXT.split(os.pathsep)`, then bare `exe`. Otherwise only `exe`.
  3. For each `dir` in `PATH.split(os.pathsep)` (missing PATH counts as `''`), and for each candidate name, return the first `os.path.join(dir, name)` that `os.path.isfile` and `os.access(X_OK)`.
     * The order is **dir-major**: all extensions in dir 1 before dir 2.
     * An empty PATH entry joins to a bare name, i.e. a cwd-relative lookup.
     * The result is **not** absolutized. A relative PATH entry gives a relative result.
* **`normexe(orig, env)`** (`:46-62`). Message strings, which surface as hook output with exit code 1:
  * No `os.sep` (and no `os.altsep`) in `orig`: `find_executable`. If nothing is found: ``Executable `{orig}` not found``.
  * Else, if it is a directory: ``Executable `{orig}` is a directory``.
  * Else, if it is not a file: ``Executable `{orig}` not found``.
  * Else, if it is not `X_OK`: ``Executable `{orig}` is not executable``. On win32 `X_OK` is always true.
* **`normalize_cmd(cmd, env)`** (`:65-85`):
  1. `exe = normexe(cmd[0])`.
  2. `cmd = parse_filename(exe) + (exe,) + cmd[1:]`, where `parse_filename` returns `()` if the path does not exist and otherwise `identify.parse_shebang_from_file`.
  3. `exe = normexe(cmd[0])` **again** (exactly one level of shebang interpretation).
  4. Return `(exe,) + cmd[1:]`.

  This applies on **all** platforms. It is how Windows runs shebang scripts, and how Linux handles shebangs longer than the kernel limit.
* **`identify.parse_shebang_from_file(path)`** (`identify/identify/identify.py:178-219`). The user's crate `file-identify` 0.4.0 exposes `parse_shebang_from_file`; verify that it matches this exactly:
  * Return `()` if the file is not `X_OK`, or if it does not start with `#!`.
  * Read the first line. It must decode as UTF-8 and contain **only `string.printable`** characters (ASCII printable plus `\t\n\r\x0b\x0c`); otherwise return `()`.
  * Tokenize with `shlex.split(line.strip())`, falling back to `line.split()` on `ValueError`.
  * Strip a leading `('/usr/bin/env', '-S')` or `('/usr/bin/env',)`.
  * If the result is exactly `('nix-shell',)`, scan the following `#!` lines for `-i <interp>`.
  * `EINVAL` gives `()`.
* **Observed messages** (4.1.0, same code in 4.6.2):
  * ``Executable `definitely-not-a-cmd-xyz` not found``
  * ``Executable `/abs/repo/nope.sh` not found`` (script)
  * ``Executable `/abs/repo/.git` is a directory``
  * ``Executable `/abs/repo/f.txt` is not executable``

  Each appears with `- exit code: 1`.

### 1.9 xargs: partitioning, concurrency, output (`xargs.py`)

* **Max command length** (`_get_platform_max_length`, `:49-58`). It is evaluated **once at import** (a default argument), so in_env PATH growth is not counted.
  * POSIX: `SC_ARG_MAX - 2048 - environ_size`, clamped to `[4096, 131072]`. `environ_size = 8*len(env) + Σ(len(k)+len(v)+2)` over `os.environb` (`:41-46`).
  * Windows: `2**15 - 2048 = 30720`.
  * Other: `4096`.
* **Batch files on Windows** (`:155-164`): if the resolved `cmd[0]` ends with `.bat` or `.cmd`, `max = 8192 - len(cmd_exe_path) - len(' /c ') - 1024`.
* **Length of an arg list** (`_command_length`, `:61-69`): `len(' '.join(args).encode(fs_encoding))`. On win32 it is `len(utf-16-le)/2`.
* **Partition** (`:76-117`):
  * `max_args = max(4, ceil(len(files)/target_concurrency))`.
  * Greedily append files **in the given order**. Start a new partition when the length would exceed max, or when `len(partition) == max_args`.
  * A single file longer than the limit raises `ArgumentTooLongError(file)`, which is uncaught (exit 3).
  * Zero files still produces **one** partition: the bare command runs once.
* **Execution** (`:131-184`):
  1. `normalize_cmd(cmd)` happens first. A failure returns `(1, b'Executable ... not found')` without running anything.
  2. `threads = min(len(partitions), target_concurrency)`, run on a ThreadPoolExecutor. `1` means plain `map`.
  3. Results are consumed **in partition order**: stdout is the in-order concatenation.
  4. Retcode aggregation: `if abs(rc) > abs(retcode): retcode = rc`. The first one wins on ties, and negative signal codes count by magnitude. For example `-9` beats `1`, and `1` then `-1` keeps `1`.
* **Process I/O.**
  * Without color: `cmd_output_b(..., check=False, stderr=STDOUT)`, with stdin as a `PIPE` that is closed immediately by `communicate()`, i.e. **an empty pipe, not `/dev/null`**. stdout and stderr are merged into one pipe, in order.
  * With color: `cmd_output_p` (non-Windows) opens a **pty** and clears `ONLCR|OPOST` on the slave (`util.py:132-141`). stdout and stderr both go to the pty, stdin is `/dev/null`, and the parent reads until EOF, treating `EIO` as EOF (`util.py:162-200`). Hooks see `isatty(stdout)=True`.
  * Windows: `cmd_output_p = cmd_output_b`, so plain pipes.
  * **(observed)** Without color, hooks see `stdin_isatty=False stdout_isatty=False`.
* **Spawn failure** (`OSError`) gives `(1, force_bytes(e).rstrip(b'\n') + b'\n')` (`util.py:86-87`).
* **Shuffle algorithm** (needed for exact ordering parity):
  * Python's MT19937 is seeded with `init_by_array([1542676187])`. For int seeds, `version=1` is irrelevant: the key is the 32-bit chunks of `abs(seed)`.
  * `shuffle(x)`: `for i in reversed(range(1, len(x))): j = randbelow(i+1); x[i], x[j] = x[j], x[i]`.
  * `randbelow(n)`: `k = n.bit_length()`, then `r = getrandbits(k)`, repeated while `r >= n`.
  * `getrandbits(k≤32)` is `genrand_uint32() >> (32-k)`.
  * The algorithm is identical in CPython 3.12 and 3.13 (`random.py:242-250`, `350-357`).
  * **Test vectors**, from fresh RNGs:
    * Shuffling `"ab"` gives `"ab"`; `"abc"` gives `"abc"`; `"abcde"` gives `"bacde"`; `"abcdefghij"` gives `"eafbigcdhj"`.
    * The first three `getrandbits(32)` after seeding are `[2472326011, 2061721674, 2625518588]`.
  * **(observed, 22 CPUs)** `entry: echo` over files a–j printed `e a f b` / `i g c d` / `h j`:
    * three partitions of `max_args=4`, in shuffled order;
    * with `PRE_COMMIT_NO_CONCURRENCY=` (empty) it printed `e a f b i g c d h j` in one partition;
    * with `require_serial: true`, `a b c d e f g h i j` (sorted, unshuffled).
* **`CalledProcessError` text** (`util.py:63-78`). Install failures show exactly:
  ```
  command: {cmd!r}            # Python tuple repr, e.g. ('/x/py_env-python3.13/bin/python', '-mpip', 'install', '.')
  return code: {rc}
  stdout:{"\n    " + indented or " (none)"}
  stderr:{"\n    " + indented or " (none)"}
  ```
  Each output line is indented 4 spaces and the block is right-stripped.

### 1.10 Local-repo template (`store.py:39-56`, `resources/empty_template_*`)

* **Files written** into the `local` repo dir (4.6.2): `Cargo.toml`, `main.go`, `go.mod`, `main.rs`, `pre-commit-package-dev-1.rockspec`, `pre_commit_placeholder_package.gemspec`, `setup.py`, `environment.yml`, `Makefile.PL`, `pubspec.yaml`, `renv.lock`, `renv/activate.R`, `renv/LICENSE.renv`. `LOCAL_REPO_VERSION = '1'` (`constants.py:9`). It was **not bumped** when the template changed, so old stores keep 4.1.0's `package.json` and `.npmignore`.
* **Contents relevant to part 1:**
  * `setup.py`: `from setuptools import setup\n\n\nsetup(name='pre-commit-placeholder-package', version='0.0.0', py_modules=[])\n`
  * `Cargo.toml`: `[package]\nname = "__fake_crate"\nversion = "0.0.0"\n\n[[bin]]\nname = "__fake_cmd"\npath = "main.rs"\n`
  * `main.rs`: `fn main() {}\n`
  * `go.mod`: `module pre-commit-placeholder-empty-module\n`
  * `main.go`: `package main\n\nfunc main() {}\n`
  * `pre_commit_placeholder_package.gemspec`: `Gem::Specification.new do |s|\n    s.name = 'pre_commit_placeholder_package'\n    s.version = '0.0.0'\n    s.summary = 'placeholder gem for pre-commit hooks'\n    s.authors = ['Anthony Sottile']\nend\n`
* **Shared-prefix contamination (observed).** All local hooks with the same `additional_dependencies` share **one** dir, and every language builds in it. Observed residue: `build/`, `target/`, `*.egg-info`, `*.gem`, `py_env-*`, `node_env-*`, `rustenv-*`, `golangenv-*`, `rbenv-*`, `docker-default`.
  * With 4.1.0, a local rust hook created `target/`. A later local python hook then failed in `pip install .` with:
    ```
    error: Multiple top-level packages discovered in a flat-layout: ['renv', 'target'].
    ```
    That was exit 3, and it is why 4.5.1 added `py_modules=[]`.
  * **snp must use the 4.6.2 template.** Better, it should skip the placeholder `pip install .` entirely for local hooks (§11.4 step 5).

---
## 2. `language: python` (`pre_commit/languages/python.py`)

### 2.1 Constants

* `ENVIRONMENT_DIR = 'py_env'` (`:23`). `run_hook = lang_base.basic_run_hook` (`:24`): shuffled, partitioned, parallel.
* `bin_dir(venv)` (`:49-52`) is `venv/Scripts` on win32 and `venv/bin` elsewhere. Node reuses it.
* `_version_info(exe)` (`:27-33`) runs `exe -S -c 'import sys;print(".".join(str(p) for p in sys.version_info))'`.
  * Typical output: `3.12.3.final.0`.
  * It is cached per exe (`functools.cache`).
  * On `CalledProcessError` it returns `'<<error retrieving version from {exe}>>'`, which then mismatches and marks the env unhealthy.
  * A missing exe does **not** raise: `normalize_cmd` failure becomes rc 1 inside `cmd_output_b`, and with check=True that raises `CalledProcessError`.
* `_read_pyvenv_cfg(filename)` (`:36-46`) reads with UTF-8. Lines where `line.split('=')` does **not** give exactly 2 parts are skipped: blank lines, comments, and **values that contain `=`**. Keys and values are `.strip()`ed.

### 2.2 `get_default_version()` (`:108-131`, cached) and how `language_version` is interpreted

The algorithm in 4.6.2:

1. `v_minor = f'{X}.{Y}'` and `v_major = f'{X}'`, taken from the **interpreter running pre-commit** (`sys.version_info`).
2. For `potential` in `(v_minor, v_major)`: `exe = f'{impl}{potential}'`, where `impl` is `'python'` for CPython and `sys.implementation.name` otherwise (for example `pypy`). If `find_executable(exe)` succeeds, **return `exe`**, for example `'python3.12'`.
   * This uses no `exe_exists` home filter: any PATH hit counts.
3. `_find_by_sys_executable()` (`:85-105`). For `path` in `(sys.executable, realpath(sys.executable))`, take the lowercase basename with `.exe` partitioned off. If it is not `python`/`pythonw` **and** `find_executable(name)` succeeds, return the name (for example `python3`, `pypy3`).
4. On win32: if `py -{X.Y} -c 'import sys; print(sys.executable)'` succeeds (with `PYTHONIOENCODING=UTF-8`), return `f'python{X.Y}'`.
5. Otherwise return `'default'`.

**(observed)** `py_env-python3.12` on a host whose pre-commit runs on 3.12.3 with `/usr/bin/python3.12` on PATH.

`norm_version(version)` (`:148-165`) maps the resolved version to the argument of `virtualenv -p`:

| `language_version` (resolved) | `norm_version` | virtualenv invocation | Notes |
|---|---|---|---|
| `default` | `None` | no `-p`: virtualenv uses **pre-commit's own interpreter** | env dir `py_env-default` |
| `python` | `None` (`_sys_executable_matches` is always true) | no `-p` | |
| `pythonX`, `pythonX.Y`, `pythonX.Y.Z` that **prefix-match** the running interpreter's `sys.version_info` | `None` | no `-p` | e.g. pre-commit on 3.12.3 with `python3`, `python3.12` or `python3.12.3` |
| `pythonX.Y` not matching, on win32 | `py -X.Y` result if found, else `find_executable(version)` if it differs from `version`, else the next row | `-p <abs exe>` | |
| anything else | `os.path.expanduser(version)` | `-p <value>` | virtualenv's own discovery: PATH names (`python3.11`, `pypy3`), bare versions (`3.11`), specs (`cpython3.11-64`), absolute paths |
| `system` | not special. It becomes a path and **fails**. | `-p system` | **(observed)** `RuntimeError: failed to find interpreter for Builtin discover of python_spec='system'`, exit 3 |

* `_sys_executable_matches(version)` (`:134-145`):
  * `'python'` gives True.
  * Otherwise the version must start with `python`, and the rest must parse as dot-separated ints that equal `sys.version_info[:n]`.
  * Any `ValueError` gives False, e.g. `python3.12-dbg`.

### 2.3 `install_environment(prefix, version, deps)` (`:214-228`)

1. `envdir = <prefix>/py_env-<version>`.
2. Run `[sys.executable, '-mvirtualenv', envdir] (+ ['-p', python])` with **`cwd='/'`**, check=True, and pre-commit's env (in_env is **not** applied).
   * virtualenv honours `VIRTUALENV_*` env vars and `virtualenv.ini`.
   * Its app-data seed cache lives in `~/.local/share/virtualenv`.
3. `with in_env(prefix, version)`, `setup_cmd(prefix, ('python', '-mpip', 'install', '.', *deps))`. That is, `cwd=prefix`, and `python` resolves through the patched PATH to `<envdir>/bin/python`.
   * `deps` are passed **verbatim as pip argv**. An item like `--pre`, `-r req.txt` or `--index-url=…` is interpreted by pip as an option. People do this.
   * pip honours `PIP_*` env vars and `pip.conf` (index URLs, trusted hosts, certs, proxy). `GIT_*` env vars are **not** stripped here.
   * `.` is the hook repo: for local hooks, the placeholder package (§1.10).

**virtualenv-created layout (observed, virtualenv 20.29.2, CPython 3.12.3):**
* `py_env-python3.12/`
  * `bin/`: `activate`, `activate.csh`, `activate.fish`, `activate.nu`, `activate.ps1`, `activate_this.py`, `pip`, `pip3`, `pip-3.12`, `pip3.12`, `python`, `python3`, `python3.12`
  * `lib/python3.12/site-packages/`: `pip`, `pip-*.dist-info`, `pip-*.virtualenv`, `_virtualenv.pth`, `_virtualenv.py`, plus the installed packages (e.g. `pre_commit_placeholder_package-0.0.0.dist-info`)
  * `.gitignore` (`# created by virtualenv automatically\n*\n`), `CACHEDIR.TAG`, `pyvenv.cfg`, `.install_state_v1`, `.install_state_v2`
* `pyvenv.cfg` (observed):
  ```
  home = /usr/bin
  implementation = CPython
  version_info = 3.12.3.final.0
  virtualenv = 20.29.2
  include-system-site-packages = false
  base-prefix = /usr
  base-exec-prefix = /usr
  base-executable = /usr/bin/python3.12
  ```
* **Seeds:** virtualenv seeds `pip` always, and also `setuptools` and `wheel` only for Python **< 3.12**. That is virtualenv's bundled behaviour (`interpreter.version_info[:2] >= (3, 12)` excludes them).
  * Hooks may rely on `pip` being present, e.g. `mypy --install-types --non-interactive` shells out to pip.
* **Timings (observed, warm caches):** `virtualenv` took ≈ 0.25 s (0.41 s with `-p python3.13`); `pip install six` took ≈ 1.3 s. For comparison, `uv venv` took ≈ 0.02 s and `uv pip install six` ≈ 0.4 s.

### 2.4 `in_env` / `get_env_patch(venv)` (`:55-61`, `:168-172`)

Applied in order:

| Var | Value |
|---|---|
| `PIP_DISABLE_PIP_VERSION_CHECK` | `1` |
| `PYTHONHOME` | **unset** |
| `VIRTUAL_ENV` | `<envdir>` (absolute; the store is realpath'ed) |
| `PATH` | `<bin_dir(envdir)>` + `os.pathsep` + original `PATH` |

* **(observed hook env):** `PATH=<store>/repoXXXX/py_env-python3.12/bin:<original PATH>`, `VIRTUAL_ENV=<…>/py_env-python3.12`, `PIP_DISABLE_PIP_VERSION_CHECK=1`, `PRE_COMMIT=1`. `sys.executable` is the venv python, and cwd is the repo root.
* **Process-global env edits:** pre-commit pops `__PYVENV_LAUNCHER__` (macOS framework-python issue; `main.py:39`) and `PYTHONEXECUTABLE` (`main.py:42`) for **every** subprocess. snp must remove both from every child env.

### 2.5 `health_check(prefix, version)` (`:175-211`)

The checks, in order:

1. If `<envdir>/pyvenv.cfg` is missing: `'pyvenv.cfg does not exist (old virtualenv?)'`.
2. If `version_info` is not in the cfg: ``"created virtualenv's pyvenv.cfg is missing `version_info`"``.
3. Run `_version_info.__wrapped__(<envdir>/<bin>/python[.exe])`, **uncached**. If the output differs from `cfg['version_info']`:
   ```
   virtualenv python version did not match created version:
   - actual version: {actual}
   - expected version: {cfg}
   ```
   (with a trailing `\n`).
4. If `base-executable` is not in the cfg, the env is healthy.
5. Otherwise run `_version_info(cfg['base-executable'])`, **cached**. A mismatch gives:
   ```
   base executable python version does not match created version:
   - base-executable version: {v}
   - expected version: {cfg}
   ```
6. Otherwise the env is healthy.

**Consequences:**
* An OS Python patch upgrade (3.12.3 → 3.12.4) silently triggers a reinstall.
* So does removing the base interpreter.
* **A `uv venv` env is always "unhealthy" to pre-commit**:
  * uv writes `version_info = 3.12.3` with no `.final.0`, so check 3 fails.
  * uv writes no `base-executable`.

  See §11.3 (G5) and §14.9.

### 2.6 Platform notes

* **Windows:**
  * The bin dir is `Scripts` and the interpreter is `python.exe`.
  * The `py` launcher is used both for the default version and for `pythonX.Y` lookup.
  * `os.access(X_OK)` is always true.
* **macOS:**
  * `__PYVENV_LAUNCHER__` is popped.
  * Framework Pythons are otherwise handled by virtualenv.
* **cygwin:** nothing python-specific.

---

## 3. `language: node` (`pre_commit/languages/node.py`)

### 3.1 Constants and default version

* `ENVIRONMENT_DIR = 'node_env'` (`:21`). `run_hook = basic_run_hook` (`:22`).
* `get_default_version()` (`:25-35`, cached):
  * win32 always gives `'default'`, because nodeenv does not support `-n system` on Windows.
  * Otherwise, if `exe_exists('node')` **and** `exe_exists('npm')`, it gives `'system'` (§1.7 home/shims filter).
  * Otherwise `'default'`.
  * **(observed)** With nvm-installed node under `$HOME` the default is `default`, so pre-commit **downloads** node.

### 3.2 `get_env_patch(venv)` (`:38-57`)

| Platform | `install_prefix` | `lib_dir` |
|---|---|---|
| cygwin | `cygpath -w <venv>` (stripped) + `\bin` | `lib` |
| win32 | `bin_dir(venv)` = `<venv>\Scripts` | `Scripts` |
| other | `<venv>` | `lib` |

The patch is applied in order:

| Var | Value |
|---|---|
| `NODE_VIRTUAL_ENV` | `<venv>` |
| `NPM_CONFIG_PREFIX` | `install_prefix` |
| `npm_config_prefix` | `install_prefix` |
| `NPM_CONFIG_USERCONFIG` | **unset** |
| `npm_config_userconfig` | **unset** |
| `NODE_PATH` | `<venv>/<lib_dir>/node_modules` |
| `PATH` | `bin_dir(venv)` + pathsep + original PATH |

**(observed, system node):** `PATH=<prefix>/node_env-system/bin:…`, `NODE_VIRTUAL_ENV=<prefix>/node_env-system`, `NPM_CONFIG_PREFIX` and `npm_config_prefix` equal to the same path, `NODE_PATH=<prefix>/node_env-system/lib/node_modules`.

### 3.3 `health_check` (`:67-73`)

* Inside `in_env`, run `node --version` with check=False.
* A non-zero rc gives ``f'`node --version` returned {retcode}'``.
  * For example `127` when the system node behind the nodeenv shim disappears (test `node_test.py:80-91`).
  * It is `1` if `node` is not on PATH at all (`Executable not found` maps to rc 1).

### 3.4 `install_environment(prefix, version, deps)` (`:76-106`, 4.6.2)

> **4.7.0 note (ADR-089).** This is the one function of the shipped package that changed in 4.7.0: the final
> `npm install` runs as `lang_base.setup_cmd(prefix, install, env=no_git_env())` (`node.py:107`, pre-commit#3774), so
> it no longer inherits `GIT_DIR`/`GIT_INDEX_FILE` (§10.3). Everything else below holds for 4.7.0.

1. **Choose the packages:**
   * If `prefix/.git` **and** `prefix/package.json` both exist: `pkgs = (f'git+file://{prefix_dir}', *deps)`. Cloned remote repos always have `.git`, because the store does `git init` + fetch.
   * Otherwise `pkgs = (*deps,)`. Local repos have **no `.git`**, so local node hooks install **only** their deps.
   * If `pkgs` is empty, raise ``AssertionError('`language: node` must have package.json or additional_dependencies')``. That is exit 3. It also means a local node hook **without** deps now fails in 4.6.x.
2. On win32, `envdir = '\\\\?\\' + normpath(envdir)` (long-path prefix) **for the nodeenv call only**.
3. Run `[sys.executable, '-mnodeenv', '--prebuilt', '--clean-src', envdir] (+ ['-n', version] if version != 'default')`.
   * check=True.
   * **cwd is inherited, i.e. the user's repo root.** This matters, see the nodeenv quirks below.
   * pre-commit's env is used (no in_env).
4. With `in_env`, `setup_cmd(prefix, ('npm', 'install', '--allow-git=root', '--install-links', '-g', *pkgs))`, with cwd=prefix.
   * `--allow-git=root` (npm ≥ 11.x config) allows git specs only for root-level (CLI) packages, not transitive ones. Older npm warns about or ignores the unknown config.
   * `--install-links` (npm ≥ 9) installs `file:`/link deps as real copies instead of symlinks.
   * `-g` with `npm_config_prefix=<venv>` puts binaries in `<venv>/bin` (POSIX) or `<venv>\Scripts` (Windows), and modules in `<venv>/lib/node_modules` or `<venv>\Scripts\node_modules`.
   * npm installs a `git+file://` spec by **cloning the repo's HEAD**, which is the detached checkout of `rev`. So `prepare` and `build` scripts run as for a git dependency. 4.6.2 fixed npm 11 with `"scripts": {"build": …}` (`CHANGELOG.md:4-9`).
5. **4.1.0 behaviour (older, for reference):**
   1. `npm install --include=dev --include=prod --ignore-prepublish --no-progress --no-save` in the prefix.
   2. `npm pack`.
   3. `npm install -g <prefix>/<pkg>.tgz *deps`.
   4. Delete `node_modules` and the tgz.

**nodeenv 1.9.1 behaviour** (`site-packages/nodeenv.py`). It must be reimplemented natively, because snp cannot assume Python.

* **Config loading:**
  * nodeenv reads a `[nodeenv]` section from `./tox.ini`, `./setup.cfg` and `~/.nodeenvrc`, relative to **cwd = the user's repo** (`nodeenv.py:387-388`). Keys: `node`, `npm`, `with_npm`, `jobs`, `without_ssl`, `debug`, `profile`, `make`, `prebuilt`, `ignore_ssl_certs`, `mirror`.
  * It also reads **`./.node-version`** (first line) as the default node version (`nodeenv.py:139-141`).
  * Because pre-commit omits `-n` for `default`, a `.node-version` or `[nodeenv] node=` in the **user's repo** silently picks the node version of `node_env-default`.
* **Versions:**
  * `latest` (the default when there is no `-n`) is `index.json[0].version` minus the leading `v`, i.e. the newest **Current** release, not LTS.
  * `lts` is the first entry with `lts != false`.
  * `system` gives a shim env (below).
  * Anything else is used **verbatim** as `X.Y.Z`. Partial versions (`18`) or `v`-prefixed versions fail, because the URL becomes `…/v18/node-v18-linux-x64.tar.gz` (404). nodeenv then only logs `Failed to download from …`, and the later glob raises `ValueError`, so install fails with exit 3.
* **Download base:** `https://nodejs.org/download/release`. When the host is x86_64-musl or riscv64 **and** no mirror is set, it is `https://unofficial-builds.nodejs.org/download/release`. `mirror` with `://` replaces the whole base; without it, it replaces the domain only.
  * Index: `{base}/index.json`.
  * Binary: `{base}/v{ver}/node-v{ver}{postfix}`.
  * `postfix`:
    * `-win-{arch}.zip` on win/cygwin
    * `-linux-x64-musl.tar.gz` on x86_64 musl
    * otherwise `-{platform.system().lower()}-{arch}.tar.gz`
  * Arch map: `x86`/`i686`→`x86`, `x86_64`/`amd64`/`AMD64`→`x64`, `armv6l`, `armv7l`/`armv8l`→`armv7l`, `aarch64`/`arm64`/`arm64/v8`/`armv8`/`armv8.4`→`arm64`, `ppc64le`, `s390x`, `riscv64`.
  * An HTTPError on an `arm64` URL retries with `x64` (Apple Silicon running old node via Rosetta).
  * The User-Agent is `nodeenv/1.9.1 (https://github.com/ekalinin/nodeenv/)`. **There is no checksum verification.**
* **Extraction:**
  1. Extract everything except `node-v{ver}*/README.md`, `CHANGELOG.md` and `LICENSE` into `<env>/src`.
  2. Copytree `src/node-v{ver}*` into `<env>` (POSIX) or `<env>\Scripts` (Windows), preserving symlinks.
  3. Remove `src` (`--clean-src`).

  The resulting POSIX env has `bin/{node,npm,npx,corepack}`, `lib/node_modules/{npm,corepack}`, `include/`, `share/`, plus nodeenv's `bin/activate`, `bin/activate.fish` and `bin/shim`.
* **`-n system`** (POSIX only):
  * Creates `bin/`, `lib/`, `lib/node_modules/`.
  * Finds `nodejs` or else `node` with `which`, using PATH minus `<env>/bin`.
  * Writes `bin/node` (and `bin/nodejs` → `node` symlink) as a bash shim:
    ```bash
    #!/usr/bin/env bash
    export NODE_PATH='<env>/lib/node_modules'
    export NPM_CONFIG_PREFIX='<env>'
    export npm_config_prefix='<env>'
    exec '<system node path>' "$@"
    ```
  * `npm` is **not** placed in the env; the system npm on PATH is used.
  * **(observed)** layout: `bin/{activate,activate.fish,node,nodejs->node,shim}`, `lib/node_modules/pre_commit_placeholder_package` (4.1.0 local template).
* **win32 with `-n system`:** nodeenv logs `Installing system node.js on win32 is not supported!` and exits 1. pre-commit never requests it on Windows by default.

### 3.5 Platform notes

* **Windows:**
  * Everything lives under `Scripts`: `node.exe`, `npm.cmd`, global bins as `.cmd` shims.
  * `NODE_PATH` is `<venv>\Scripts\node_modules`.
  * Hooks are invoked through `.cmd` shims, which use the batch-file arg-length rule (§1.9) and cmd.exe quoting. **Rust pitfall:** see §14.6.
* **cygwin:** `cygpath -w` is used for the npm prefix.
* **macOS:** arm64 tarballs, with the x64 fallback.

---

## 4. `language: golang` (`pre_commit/languages/golang.py`)

### 4.1 Constants and default version

* `ENVIRONMENT_DIR = 'golangenv'` (`:31`). `health_check = basic` (always healthy) (`:32`). `run_hook = basic` (`:33`).
* `_ARCH` (`:35-43`) is `platform.machine().lower()`, mapped through `{'x86_64':'amd64','i386':'386','aarch64':'arm64','armv8':'arm64','armv7l':'armv6l'}`. Other values pass through: `arm64` on macOS, `amd64` on Windows x64, `386`, `ppc64le`, `s390x`, `riscv64`, `loong64`. 32-bit Windows `x86` would give `x86` and a 404.
* `_EXT` (`:50-59`) is `zip` on win32 and `tar.gz` elsewhere.
* `get_default_version()` (`:62-67`) is `'system'` if `exe_exists('go')`, else `'default'`.

### 4.2 Version resolution and download (`:88-120`)

* `_infer_go_version(version)` (cached):
  * Any version other than `default` is returned **verbatim**.
  * `default` does `GET https://go.dev/dl/?mode=json`, takes `[0]['version']` and strips the `go` prefix. That is the newest stable release, e.g. `1.27.1` today; the endpoint lists only the latest 2 stable releases.
* URL: `https://dl.google.com/go/go{version}.{platform.system().lower()}-{_ARCH}.{_EXT}`, e.g. `go1.21.1.linux-amd64.tar.gz` or `go1.22.0.darwin-arm64.tar.gz`.
  * `language_version` must be an **exact** Go release string: `1.21.1`, or `1.20` for releases before 1.21. `1.22`, `go1.22.1` and `~1.22` produce a 404.
  * A 404 raises ``ValueError(f'Could not find a version matching your system requirements (os={os}; arch={arch})')`` (exit 3). Other HTTP errors propagate.
  * **There is no checksum verification.** The go.dev JSON publishes a `sha256` per file; `?mode=json&include=all` covers all versions.
* Install sequence:
  1. Download into a `TemporaryFile`.
  2. `extractall(dest=envdir)`.
  3. Move `envdir/go` to `envdir/.go`.

### 4.3 `install_environment(prefix, version, deps)` (`:130-161`)

1. If `version != 'system'`, call `_install_go(version, envdir)`. That creates `envdir/.go` (GOROOT).
2. `gopath = envdir`. On cygwin it is `cygpath -w envdir`.
3. `env = no_git_env(dict(os.environ, GOPATH=gopath))` (§10.3 for `no_git_env`), then `env.pop('GOBIN')`.
4. If not `system`, also set:
   * `GOTOOLCHAIN=local`, which disables Go's automatic toolchain download (`CHANGELOG.md:117`);
   * `GOROOT=envdir/.go`;
   * `PATH = envdir/.go/bin + pathsep + os.environ['PATH']`. This raises `KeyError` if PATH is unset.
5. Run `setup_cmd(prefix, ('go', 'install', './...'), env=env)`. The cwd is the prefix, and binaries go to `$GOPATH/bin = envdir/bin`.
6. For each dep **in order**: `setup_cmd(prefix, ('go', 'install', dep), env=env)`.
   * The dep is a Go install path, e.g. `golang.org/x/tools/cmd/goimports@latest`. An `@version` is needed when outside the module's requirements.
   * The cwd is the hook module.
7. `rmtree(envdir/pkg)` (the module cache and build cache). It uses `_handle_readonly` because **the Go module cache is read-only (0444/0555)**: a plain recursive delete fails (§14.10).

**Observations and notes:**
* For **`system`**, `GOTOOLCHAIN` is **not** set during install. A hook repo `go.mod` with `toolchain go1.x` makes system go auto-download that toolchain into `envdir/pkg/mod`, which is then deleted.
* User `GOFLAGS` (e.g. `-mod=vendor`), `GOPROXY`, `GONOSUMDB`, `CGO_ENABLED` and similar vars leak into the install. That is intentional, so proxies work.
* **(observed)** `golangenv-system/{bin/pre-commit-placeholder-empty-module, .install_state_v1, .install_state_v2}` for a local hook.

### 4.4 `in_env` / `get_env_patch(venv, version)` (`:70-85`, `:123-127`)

| version | Patch |
|---|---|
| `system` | `PATH = <venv>/bin : $PATH` |
| other | `GOROOT=<venv>/.go`, `GOTOOLCHAIN=local`, `PATH = <venv>/bin : <venv>/.go/bin : $PATH` |

* **`GOPATH` is NOT set at runtime.** The hook sees the user's GOPATH. **(observed)** `GOPATH=/home/dromix/go`, which was inherited.
* Tests (`golang_test.py:172-235`):
  * Pinned-version hooks fail with `go.mod requires go >= 1.23.1` instead of auto-switching. This is intended.
  * The same happens for `go fmt` run over a user module that needs a newer go.

### 4.5 Platform notes

* Windows: `.zip` archive, `go.exe`, and `<venv>\bin`. Note it is `bin`, not `Scripts`.
* cygwin: `GOPATH` comes from `cygpath -w`.
* macOS: `darwin-{amd64|arm64}`.

---

## 5. `language: rust` (`pre_commit/languages/rust.py`)

### 5.1 Constants and default version

* `ENVIRONMENT_DIR = 'rustenv'` (`:24`). `health_check = basic` (`:25`). `run_hook = basic` (`:26`).
* `get_default_version()` (`:29-40`, cached):
  * `'system'` if `cargo --version` exits 0 when run with **`cwd='/'`**. Using `/` means a user `rust-toolchain.toml` with a bogus channel cannot break detection (`rust_test.py:34-39`). It checks that cargo actually runs, which a rustup proxy without a toolchain would not.
  * Otherwise `'default'`.
  * No `$HOME` filter: `~/.cargo/bin/cargo` counts as system.
* `_rust_toolchain(v)` (`:43-48`) maps `default` to `'stable'`. Any other version is passed through verbatim as a **rustup toolchain spec**: `1.79.0`, `nightly`, `nightly-2024-01-01`, `beta`, `stable-x86_64-unknown-linux-gnu`, …

### 5.2 `in_env` / `get_env_patch(target_dir, version)` (`:51-66`)

| Var | Value |
|---|---|
| `PATH` | `<envdir>/bin : $PATH` |
| `RUSTUP_TOOLCHAIN` | `_rust_toolchain(version)`, **only if `version != 'system'`** |

* `CARGO_HOME` and `RUSTUP_HOME` are **not** set at runtime. A hook that runs `cargo …` with a pinned toolchain uses the user's `~/.rustup`, and rustup may auto-install the toolchain there.

### 5.3 `install_environment(prefix, version, deps)` (`:113-160`)

1. **Split the deps:**
   * `cli_deps = {d for d in deps if d.startswith('cli:')}`
   * `lib_deps = set(deps) - cli_deps`

   Both are **sets**, so the iteration order is arbitrary.
2. **Packages to install** (`packages_to_install`, a **set** of arg tuples):
   * `('--path', '.')`, always: the hook repo itself, or the placeholder `__fake_crate` for local hooks, which installs `__fake_cmd`.
   * For each `cli:` dep: strip `cli:`, then `package, _, ver = rest.partition(':')`. Add `(package, '--version', ver)` if `ver` is non-empty, else `(package,)`.
     * Examples: `cli:shellharden:4.2.0` and `cli:shellharden`.
     * The version is a cargo semver requirement, e.g. `4.2.0`, `^4` or `=4.2.0`.
     * **No git URLs** (that is a prek extension).
3. Within an `ExitStack`, run the rest under `in_env(prefix, version)`: PATH plus `RUSTUP_TOOLCHAIN` if not system.
4. If `version != 'system'`, call `install_rust_with_toolchain(toolchain, envdir)` (`:83-110`):
   1. Create a temp dir `rustup_dir`. Set `CARGO_HOME=envdir` and `RUSTUP_HOME=rustup_dir`.
   2. If `find_executable('rustup')` is `None`, bootstrap rustup:
      * Download `https://sh.rustup.rs` on POSIX (this is a **shell script**, `rustup-init.sh`), or `https://win.rustup.rs/x86_64` on win32 (always x86_64, even on ARM64).
      * Save it as `<rustup_dir>/rustup-init[.exe]` and `chmod +x` it.
      * Run `rustup-init -y --quiet --no-modify-path --default-toolchain none`. This installs rustup and proxies into `envdir/bin`. The shebang `#!/bin/sh` is handled by `normalize_cmd`.
   3. Run `rustup toolchain install --no-self-update <toolchain>`. `rustup` is found via the patched PATH, so `envdir/bin/rustup` if it was just bootstrapped.
   4. **The temp `RUSTUP_HOME` is deleted** when the function returns.
5. In the same non-system branch, enter **another** fresh temp dir and set `RUSTUP_HOME=<tmp2>` for the remaining steps.
   * Net effect: the toolchain installed in step 4 is discarded. rustup then **auto-installs** `RUSTUP_TOOLCHAIN` again into `<tmp2>` when `cargo` is first invoked. This relies on rustup auto-install, which 1.28.0 briefly removed and 1.28.1 restored (`RUSTUP_AUTO_INSTALL` opt-out).
   * Then that copy is deleted too.
   * **No toolchain persists.** Only compiled binaries are kept. This design keeps the env small (`CHANGELOG.md:327`) and works with a read-only host `RUSTUP_HOME` (`CHANGELOG.md:334`), at the cost of downloading the toolchain once or twice per install.
6. If there are lib deps: `setup_cmd(prefix, ('cargo', 'add', *[f'{name}@{spec or "*"}' for each]))`, where `name, _, spec = dep.partition(':')`.
   * This **modifies the hook clone's `Cargo.toml`**, which is acceptable because clones are per-deps-set.
   * It needs cargo ≥ 1.62.
7. For each package args tuple: `cargo install --bins --root <envdir> <args…>`, with `cwd=prefix`.
   * There is **no `--locked`**, so the lockfile is ignored and the latest compatible deps are resolved.
   * `CARGO_HOME` is the user's, so the registry cache and `~/.cargo/config.toml` are shared.
   * The `target/` dir is left in the prefix. **(observed)**: `target/` appears in the local repo dir.

**(observed)** `rustenv-system/{bin/__fake_cmd, .crates.toml, .crates2.json, .install_state_v1, .install_state_v2}`.

### 5.4 Platform notes

* Windows: `rustup-init.exe` from `win.rustup.rs/x86_64`, binaries `*.exe` in `<envdir>\bin`.
* macOS and Linux: `sh.rustup.rs`.
* **Better source for snp:** the official binary `https://static.rust-lang.org/rustup/dist/<target-triple>/rustup-init[.exe]`, plus its `.sha256` sibling.

---

## 6. `language: ruby` (`pre_commit/languages/ruby.py`)

### 6.1 Constants and default version

* `ENVIRONMENT_DIR = 'rbenv'` (`:22`). `health_check = basic` (`:23`). `run_hook = basic` (`:24`).
* `get_default_version()` (`:32-37`) is `'system'` if `exe_exists('ruby')` and `exe_exists('gem')`, else `'default'`. rbenv shims (`/shims/`) and `$HOME` installs are not system.
* **Resources bundled in the package:** `resources/rbenv.tar.gz` (rbenv `1.3.2`), `ruby-build.tar.gz` (ruby-build `20251023`) and `ruby-download.tar.gz` (the rbenv-download plugin: RVM binaries via curl). Each tar's root dir has mode `0o755` (`ruby_test.py:39-44`).

### 6.2 `get_env_patch(venv, language_version)` (`:40-72`)

| Var | system | default | `X.Y.Z` (other) |
|---|---|---|---|
| `GEM_HOME` | `<venv>/gems` | same | same |
| `GEM_PATH` | **unset** | unset | unset |
| `BUNDLE_IGNORE_CONFIG` | `1` | `1` | `1` |
| `RBENV_ROOT` | – | `<venv>` | `<venv>` |
| `PATH` | `<venv>/gems/bin : $PATH` | `<venv>/gems/bin : <venv>/shims : <venv>/bin : $PATH` | same as default |
| `RBENV_VERSION` | – | – (not set for `system`/`default`) | `<version>` |

**(observed, system):** `PATH=<prefix>/rbenv-system/gems/bin:…`, `GEM_HOME=<prefix>/rbenv-system/gems`, `BUNDLE_IGNORE_CONFIG=1`.

### 6.3 `install_environment(prefix, version, deps)` (`:115-145`)

1. **If `version != 'system'`** (POSIX only):
   1. `_install_rbenv(prefix, version)` (`:88-101`):
      * Extract `rbenv.tar.gz` into the prefix, which gives `prefix/rbenv/`, then `shutil.move` it to `envdir`.
      * If `version != 'default'`, also extract `ruby-download.tar.gz` and `ruby-build.tar.gz` into `envdir/plugins/`.
   2. Under `in_env`, run `rbenv init -` (cwd=prefix). Its output is ignored; it is run for the directory side effects.
   3. If `version != 'default'`, run `_install_ruby` (`:104-112`): `rbenv download <version>`.
      * That downloads a prebuilt ruby from `https://rvm.io/binaries/<os>/<os_ver>/<arch>/ruby-<v>.tar.bz2`, using `curl`, `tar xj` and bash.
      * On `CalledProcessError` it falls back to `rbenv install <version>` (ruby-build **compiles from source**: needs a C toolchain, openssl and so on; slow).
   4. Run `rbenv rehash` (creates the shims).
2. **Always**, under `in_env`:
   1. `gem build *prefix.star('.gemspec')`. Every `*.gemspec` in the prefix root is passed, in **directory order**. With none, `gem build` searches the cwd and fails.
   2. `gem install --no-document --no-format-executable --no-user-install --install-dir <envdir>/gems --bindir <envdir>/gems/bin *prefix.star('.gem') *deps`.
      * Every `.gem` in the prefix root is passed, including ones from earlier builds.
      * `--no-user-install` overrides a `gem: --user-install` in `GEMRC` (`ruby_test.py:79-84`).
      * The explicit dirs work around OS packagers that set `--install-dir`/`--bindir` (`CHANGELOG.md:267`).
3. **Quirks:**
   * With `default`, no ruby is installed into the env. The `rbenv` shims pick a version via `RBENV_VERSION`, then `.ruby-version` (looked up from the **cwd**: the hook repo at install time, the user's repo at run time), then `<env>/version`, then `system`. So `default` effectively uses whatever `ruby` is on PATH.
   * A user `.ruby-version` naming a version that is not installed breaks the shims (`rbenv: version '…' is not installed`).
   * `BUNDLE_IGNORE_CONFIG=1` stops a user `.bundle/config` (`BUNDLE_PATH`, `BUNDLE_DISABLE_SHARED_GEMS`) from redirecting gems (`ruby_test.py:123-139`).

**(observed)** `rbenv-system/gems/{bin,build_info,cache,doc,extensions,gems,plugins,specifications}`. The prefix root also holds the built `pre_commit_placeholder_package-0.0.0.gem`.

### 6.4 Platform notes

* Windows: only `system` is realistic. rbenv is a bash tool; the code is marked `pragma: win32 no cover`.
* macOS: rvm binaries exist for some OS versions; otherwise ruby-build compiles.

---

## 7. `language: docker` and `language: docker_image` (`pre_commit/languages/docker.py`, `docker_image.py`)

### 7.1 docker: constants

* `ENVIRONMENT_DIR = 'docker'` (`docker.py:16`). `PRE_COMMIT_LABEL = 'PRE_COMMIT'` (`:17`).
* `get_default_version = basic` (`'default'`), `health_check = basic`, `in_env = no_env` (`:18-20`).
* `docker_tag(prefix)` (`:73-75`) is `'pre-commit-' + md5(os.path.basename(prefix_dir).encode()).hexdigest().lower()`.
  * The basename is the store dir name (`repoXXXXXXXX`, random from `mkdtemp(prefix='repo')`), so tags are per clone.
  * **(observed)** `md5("repoegjh37aw")` gives tag `pre-commit-f9bbd8a6ea92c181768b56d24cd49a45`.

### 7.2 docker: build (`build_docker_image`, `:78-92`)

```
docker build --tag <tag> --label PRE_COMMIT [--pull] .
```

* It runs with cwd = prefix via `setup_cmd`, so check=True: failure raises `CalledProcessError` (exit 3).
* `.` comes last for old docker (#477).
* **Install** (`install_environment`, `:95-106`):
  1. `assert_version_default('docker', version)` and `assert_no_additional_deps('docker', deps)`.
  2. `build_docker_image(prefix, pull=True)`.
  3. `os.mkdir(envdir)` (`docker-default`), so the state files have a home.
* **Every run** calls `build_docker_image(prefix, pull=False)` first, because users prune images (`:171`).
* **(observed)**
  * at install: `docker [build] [--tag] [pre-commit-f9bb…] [--label] [PRE_COMMIT] [--pull] [.]  cwd=<store>/repoegjh37aw`
  * then per run: `docker [build] [--tag] [pre-commit-f9bb…] [--label] [PRE_COMMIT] [.]`
* **Local docker hooks** use the local template dir, which has **no Dockerfile**, so the build fails. `language: docker` only makes sense for remote repos that ship a `Dockerfile`.

### 7.3 Shared `docker run` prefix (`docker_cmd(*, color)`, `:145-156`)

```
docker run --rm [--tty] [-u <uid>:<gid>] -v <host_cwd>:/src:rw,Z --workdir /src
```

* `--tty` is added iff `color` is true (`get_docker_tty`, `:141-142`). Because colour means a pty, the container gets a TTY.
* `-u {os.getuid()}:{os.getgid()}` (`get_docker_user`, `:131-138`):
  * omitted if `_is_rootless()`;
  * omitted on `AttributeError` (Windows has no `getuid`).
* `-v`:
  * `host_cwd = _get_docker_path(os.getcwd())`. The cwd is the repo root.
  * The options `rw,Z` mean a private SELinux relabel. Docker Desktop ignores `Z`.
  * The path is passed raw. A cwd containing `:` breaks the `-v` syntax; pre-commit does not escape it.
* `--workdir /src`.
* **`_is_rootless()`** (`:109-128`, cached once):
  * It runs `docker system info --format '{{ json . }}'` (literal argv element `{{ json . }}`).
  * Rootless is true if either:
    * `'name=rootless' in (info.get('SecurityOptions') or ())` (docker), or
    * `info['host']['security']['rootless']` (podman).
  * A `KeyError` gives False.
  * **Bug:** `cmd_output_b` is called with the default `check=True`, so a failing `docker system info` (daemon down) **raises** `CalledProcessError` (exit 3) instead of returning False. The `retcode != 0` branch is dead code. snp should implement the intent: on failure, return False.
  * Not present in 4.1.0. **(observed)** 4.1.0 always passes `-u 1000:1000`.
* **Container-in-container path mapping** (`_get_docker_path`, `:45-66`; `_get_container_id`, `:34-42`). This is for running pre-commit inside a container that talks to the host daemon.
  1. Read `/proc/1/mountinfo` (bytes). Missing file means not in a container. For each line, `re.search` with:
     ```
     /containers(?:/overlay-containers)?/([a-z0-9]{64})(?:/userdata)?/hostname
     ```
     The first match's group 1 is the container id. This covers docker cgroup v1 and v2 (`/var/lib/docker/containers/<id>/hostname`) and podman (`/containers/overlay-containers/<id>/userdata/hostname`). Fixtures are in `tests/languages/docker_test.py:17-215`.
  2. No id: use the path unchanged.
  3. Otherwise run `docker inspect <id>`. On `CalledProcessError` (true docker-in-docker, where the id is not visible) use the path unchanged.
  4. `container, = json.loads(out)`. This raises if the output is not a 1-element list.
  5. For each `mount` in `container['Mounts']`: if `os.path.commonpath((path, mount['Destination'])) == mount['Destination']`, return `path.replace(Destination, Source)`.
     * **Bug-ish:** `str.replace` replaces **every** occurrence, not just the prefix. Implement prefix replacement only.
     * Path-component semantics: `/projectSuffix` does not match `/project`.
  6. No mount matched: use the path unchanged.
  * **4.1.0 algorithm (old):** check that `/proc/1/cgroup` contains `b'docker'`, then take the basename of the third field of the `cpuset` line as the id.

### 7.4 docker `run_hook` (`:159-181`)

1. `build_docker_image(prefix, pull=False)`.
2. `entry_exe, *cmd_rest = hook_cmd(entry, args)`.
3. Run `run_xargs((*docker_cmd(color), '--entrypoint', entry_exe, docker_tag(prefix), *cmd_rest), file_args, require_serial, color)`.
   * Shuffled and parallel unless `require_serial`.
   * Each partition is a **separate container**.
   * **(observed)** `docker [run] [--rm] [-u] [1000:1000] [-v] [<repo>:/src:rw,Z] [--workdir] [/src] [--entrypoint] [my-entry] [pre-commit-f9bb…] [--flag] [a b] [--arg1] [f 2.txt] [f1.txt]`. With `--color=always` there is also `[--tty]` after `--rm`.
   * Filenames are repo-root-relative, so valid inside `/src`.

### 7.5 docker_image (`docker_image.py`)

* `ENVIRONMENT_DIR = None`, so it is not installable. `language_version` and deps are errors (§1.3); the prefix is the cwd for local hooks.
* `run_hook` (`:16-32`) is `run_xargs(docker_cmd(color) + hook_cmd(entry, args), …)`. The entry supplies `[docker run options…] <image> [cmd…]`, e.g. `--entrypoint sh alpine:3 -c "echo hi"`.
* **(observed)** `docker [run] [--rm] [-u] [1000:1000] [-v] [<repo>:/src:rw,Z] [--workdir] [/src] [--entrypoint] [sh] [alpine:3] [-c] [echo hi] [f 2.txt] [f1.txt]`.
* No pull or build: docker pulls on demand. On Windows the docker functions are `pragma: win32 no cover`, i.e. untested.

---

## 8. Environment-less languages: unsupported, unsupported_script, fail, pygrep

### 8.1 `unsupported` (alias `system`) (`unsupported.py`)

* Every member is the generic one: `ENVIRONMENT_DIR=None`, not installable, `no_env`, `basic_run_hook`.
* The entry is resolved through **PATH** by `normalize_cmd`, including shebang handling.
* The prefix is the repo root for local hooks and the clone dir for remote hooks. It is unused, because the run cwd is always the repo root.

### 8.2 `unsupported_script` (alias `script`) (`unsupported_script.py:15-32`)

* `cmd = hook_cmd(entry, args)`, then `cmd = (prefix.path(cmd[0]), *cmd[1:])`. The first token is joined with the **prefix** and normpath'ed:
  * local: `<repo_root>/<entry0>`;
  * remote: `<clone>/<entry0>`;
  * an absolute `entry0` stays absolute.
* Then `run_xargs`. `normexe` errors: `not found`, `is a directory`, `is not executable` (§1.8). The script must be executable (`chmod +x`, committed with mode 100755). On Windows, a shebang makes it runnable.

### 8.3 `fail` (`fail.py:15-27`)

* `run_hook` ignores `args`, `require_serial`, `color` and the prefix.
* It returns `(1, f'{entry}\n\n'.encode() + b'\n'.join(f.encode() for f in file_args) + b'\n')`.
* After `out.strip()` in `run.py`, the user sees the entry text, a blank line, then one filename per line.
* With no files (`pass_filenames: false` or `always_run`) only the entry is shown.
* **(observed):**
  ```
  forbid txt...............................................................Failed
  - hook id: forbid
  - exit code: 1

  Files must not be txt
  second line

  a.txt
  b.txt
  c.txt
  d.txt
  ```
  (from `entry: "Files must not be txt\nsecond line"`). The `fail no files` case printed just `nope`.
* File order is the classifier's order, unshuffled because `fail` does not use `run_xargs`.

### 8.4 `pygrep` (`pygrep.py`): reference behaviour

* Not installable. `run_hook` (`:90-101`) runs `xargs((sys.executable, '-m', 'pre_commit.languages.pygrep', *args, entry), file_args, color=color)`:
  * **`target_concurrency` defaults to 1**, so partitions run serially;
  * **no shuffle**;
  * `require_serial` is ignored.

  Output from partitions (several Python processes for long file lists) is concatenated.
* **CLI** (`main`, `:104-129`), argparse:
  * prog `pygrep.py` (basename of `argv[0]`; Python 3.14 changes this for `-m`);
  * description: `grep-like finder using python regexes.  Unlike grep, this tool returns nonzero when it finds a match and zero otherwise.  The idea here being that matches are "problems".`
  * `-i/--ignore-case` (store_true);
  * `--multiline` (store_true);
  * `--negate` (store_true);
  * positional `pattern` (`help='python regex pattern.'`);
  * positional `filenames` (`nargs='*'`);
  * argparse defaults: `-h/--help`, **`allow_abbrev=True`**, so unique long-option prefixes such as `--multi`, `--neg` and `--ign` are accepted.
  * Usage line (observed): `usage: pygrep.py [-h] [-i] [--multiline] [--negate] pattern [filenames ...]`.
  * Unknown option: `pygrep.py: error: unrecognized arguments: --foo` on stderr, **exit 2**.
  * `-h` in args prints the help and **exits 0, so the hook passes**.
* **Flags:**
  * `re.IGNORECASE` if `-i`.
  * `--multiline` adds `re.MULTILINE | re.DOTALL`.
  * The pattern is `args.pattern.encode()` (UTF-8), giving a **bytes regex**:
    * ASCII-only `\w \d \s \b` and case folding;
    * `.` is any byte except `\n`;
    * `(?u)` is an error (`bad inline flags: cannot use 'u' flag with a bytes pattern`);
    * `\u`, `\U`, `\N{…}` and `\p` are errors.
* **Per-file functions** (the `FNS` table, `:82-87`). Files are opened `'rb'`.

| multiline | negate | Function | Behaviour (exact bytes written to stdout) |
|---|---|---|---|
| False | False | `_process_filename_by_line` (`:22-30`) | Iterates lines split on `b'\n'`. Each line **includes** its trailing `\n`, and the last line may lack it. For **every** line where `pattern.search(line)` matches: `f'{filename}:{lineno}:'` (1-based) + `line.rstrip(b'\r\n')` + `b'\n'`. Returns 1 if any line matched. |
| True | False | `_process_filename_at_once` (`:33-47`) | `contents = f.read()`; only the **first** `pattern.search(contents)` counts. `line_no = contents[:m.start()].count(b'\n')`. Writes `f'{filename}:{line_no+1}:'`, then `m[0].split(b'\n')` with element 0 **replaced** by `contents.split(b'\n')[line_no]` (the full line, which keeps `\r`), joined with `b'\n'`, then `b'\n'`. Returns 1. |
| False | True | `_process_filename_by_line_negated` (`:50-60`) | If **no** line matches: write `filename + '\n'` and return 1. Otherwise return 0. An empty file prints its name. |
| True | True | `_process_filename_at_once_negated` (`:63-74`) | If `pattern.search(contents)` is None: write `filename + '\n'` and return 1. Otherwise return 0. |

* The exit code is the bitwise OR over files (0 or 1).
* **Errors** are uncaught Python exceptions. They print a traceback to stderr, merged into the hook output, and exit 1:
  * an invalid regex: last line `re.error: missing ), unterminated subpattern at position 0`;
  * a missing file: `FileNotFoundError: [Errno 2] No such file or directory: 'x'`;
  * a directory: `IsADirectoryError: [Errno 21] Is a directory: '.'`.

  The files processed before the error still produced output.
* **Argument-order trap:** the command is `[*args, entry, *files]`. A non-option item in `args` becomes the pattern and the entry becomes a "file" (FileNotFoundError). A file named `-x` is parsed as an option (exit 2).
* **Observed outputs** (exact `repr` of stdout bytes; reference = pre-commit's module):

| Invocation | Files | stdout | rc |
|---|---|---|---|
| `TODO` | a.txt=`hello TODO world\nfoo\nbar baz\n`, c.txt=`line1\r\nTODO crlf\r\n` | `a.txt:1:hello TODO world\nc.txt:2:TODO crlf\n` | 1 |
| `foo` | multi.txt=`x foo foo\ny\nfoo` | `multi.txt:1:x foo foo\nmulti.txt:3:foo\n` | 1 |
| `-i FOO` | multi.txt, crlf.txt=`a\r\nfoo bar\r\nbaz\r\n` | `multi.txt:1:x foo foo\nmulti.txt:3:foo\ncrlf.txt:2:foo bar\n` | 1 |
| `foo$` | a.txt | `a.txt:2:foo\n` (`$` matches before the final `\n` of the line) | 1 |
| `foo$` | cr2.txt=`foo\r\r\n` | (empty) | 0 |
| `--multiline 'def\nghi'` | d.txt=`abc\ndef\nghi` | `d.txt:2:def\nghi\n` | 1 |
| `--multiline 'foo.*\n'` | crlf.txt | `crlf.txt:2:foo bar\r\nbaz\r\n\n` | 1 |
| `--multiline '\ncd'` | nl.txt=`ab\ncd\n` | `nl.txt:1:ab\ncd\n` (the match starts at the `\n` ending line 1) | 1 |
| `--multiline '^'` | d.txt | `d.txt:1:abc\n` (empty match) | 1 |
| `--multiline 'foo$'` | multi.txt | `multi.txt:1:x foo foo\n` | 1 |
| `--multiline '(?-m:foo$)'` | multi.txt | `multi.txt:3:foo\n` | 1 |
| `--negate foo` | b.txt=`nothing here\n`, … | `b.txt\n…` | 1 |
| `--negate x` | empty.txt=(0 bytes) | `empty.txt\n` | 1 |
| `--negate --multiline x` | empty.txt | `empty.txt\n` | 1 |
| `caf\w` | `caf\xc3\xa9 x` | (empty) (ASCII `\w`) | 0 |
| `café` | same | `u.bin:1:café x\n` | 1 |
| `-i café` | `CAFÉ` | (empty) (ASCII-only folding) | 0 |

* **Through pre-commit, observed:** the `grep todo` hook printed `.pre-commit-config.yaml:12:    entry: 'TODO'`, `a.txt:1:hello TODO world` and `c.txt:2:TODO crlf`, with `- exit code: 1`. Files came in classifier order, unshuffled.

---
## 9. Platform differences matrix (part-1 languages)

| Aspect | Linux | macOS | Windows (win32) | cygwin |
|---|---|---|---|---|
| Executable lookup | PATH, `isfile && access(X_OK)` | same | `PATHEXT` extensions first, then the bare name, per PATH dir. `X_OK` is always true. | posix |
| `exe_exists` "home" | `$HOME` | `$HOME` | `%USERPROFILE%`. A different drive counts as "not in home". | posix |
| Hook output capture with `--color` | pty (ONLCR/OPOST off), stdin `/dev/null` | pty | pipes (no pty) | pty |
| xargs max cmdline | `ARG_MAX - 2048 - env`, clamped to [4096, 131072] | same | 30720; `.bat`/`.cmd`: `8192 - len(cmd.exe) - 4 - 1024` | posix |
| python venv bin dir | `bin` | `bin` (`__PYVENV_LAUNCHER__` popped) | `Scripts`, `python.exe` | `bin` |
| python default version | `pythonX.Y` / `pythonX` on PATH | same | plus the `py -X.Y` launcher | same as Linux |
| node default version | `system` if node+npm are outside `$HOME` and not shims | same | **always `default`** | same as Linux |
| node env layout | `bin/`, `lib/node_modules` | same | `Scripts\` (node.exe, npm.cmd, global `.cmd` shims), `Scripts\node_modules` | npm prefix `cygpath -w <env>\bin` |
| node download | `node-vX-linux-{arch}.tar.gz` (musl and riscv64 from unofficial-builds) | `darwin-{arm64,x64}` (arm64 falls back to x64) | `node-vX-win-{arch}.zip`; envdir passed as `\\?\…` | win zip |
| golang archive | `go{v}.linux-{arch}.tar.gz` | `go{v}.darwin-{amd64,arm64}.tar.gz` | `go{v}.windows-{arch}.zip` | `GOPATH` from `cygpath -w` |
| rust bootstrap | `https://sh.rustup.rs` (shell script) | same | `https://win.rustup.rs/x86_64` (exe, always x86_64) | – |
| ruby non-system | rbenv + `rbenv download` (rvm.io) or ruby-build | same | **unsupported** (bash tools) | – |
| docker `-u` | `uid:gid` unless rootless | `uid:gid` | omitted (no `os.getuid`) | – |
| docker self-container detection | `/proc/1/mountinfo` | file missing, so off | off | – |

---

## 10. Environment variables

### 10.1 Variables pre-commit itself reads (in the scope of this spec)

| Var | Where | Effect |
|---|---|---|
| `PRE_COMMIT_NO_CONCURRENCY` | `lang_base.py:136` | presence means `jobs=1` |
| `TRAVIS` | `lang_base.py:140` | presence means `jobs=2` |
| `PATH`, `PATHEXT` | `parse_shebang.py:31-37` | exe lookup |
| `HOME` / `USERPROFILE` | `lang_base.py:68` | `exe_exists` |
| `__PYVENV_LAUNCHER__`, `PYTHONEXECUTABLE` | `main.py:39,42` | **removed** from the env for everything |
| `PYTHONIOENCODING` | set to `UTF-8` only for the `py` launcher probe (`python.py:70`) | – |

### 10.2 Variables set or unset for hook execution (in_env)

| Language | Set | Unset | PATH prepend (in order) |
|---|---|---|---|
| python | `PIP_DISABLE_PIP_VERSION_CHECK=1`, `VIRTUAL_ENV=<env>` | `PYTHONHOME` | `<env>/bin` (win `Scripts`) |
| node | `NODE_VIRTUAL_ENV=<env>`, `NPM_CONFIG_PREFIX`, `npm_config_prefix` (= `<env>`; win `<env>\Scripts`), `NODE_PATH=<env>/lib/node_modules` (win `<env>\Scripts\node_modules`) | `NPM_CONFIG_USERCONFIG`, `npm_config_userconfig` | `<env>/bin` (win `Scripts`) |
| golang (system) | – | – | `<env>/bin` |
| golang (other) | `GOROOT=<env>/.go`, `GOTOOLCHAIN=local` | – | `<env>/bin`, `<env>/.go/bin` |
| rust (system) | – | – | `<env>/bin` |
| rust (other) | `RUSTUP_TOOLCHAIN=<stable or version>` | – | `<env>/bin` |
| ruby (system) | `GEM_HOME=<env>/gems`, `BUNDLE_IGNORE_CONFIG=1` | `GEM_PATH` | `<env>/gems/bin` |
| ruby (default) | the system set plus `RBENV_ROOT=<env>` | `GEM_PATH` | `<env>/gems/bin`, `<env>/shims`, `<env>/bin` |
| ruby (`X.Y.Z`) | the default set plus `RBENV_VERSION=<version>` | `GEM_PATH` | same as default |
| docker, docker_image, unsupported*, fail, pygrep | – | – | – |

Notes:
* `PRE_COMMIT=1` and the stage-specific `PRE_COMMIT_*` vars are set by `commands/run.py:377-417` for **all** hooks. They belong to the run spec.
* Everything else is inherited unchanged, including `GIT_INDEX_FILE` and the other `GIT_*` vars that git exports to hooks.

### 10.3 Install-time environments

| Language | Command(s) | cwd | Env |
|---|---|---|---|
| python | `<py> -mvirtualenv <env> [-p X]` | `/` | process env |
| python | `python -mpip install . *deps` | prefix | in_env |
| node | `<py> -mnodeenv --prebuilt --clean-src <env> [-n V]` | **inherited (repo root)** | process env |
| node | `npm install --allow-git=root --install-links -g *pkgs` | prefix | in_env |
| golang | `go install ./...`, then `go install <dep>` for each dep | prefix | `no_git_env(environ + GOPATH=<env>)` − `GOBIN`, plus `GOTOOLCHAIN=local`, `GOROOT`, `PATH=<env>/.go/bin:$PATH` (non-system) |
| rust | `rustup-init …`, `rustup toolchain install …` | inherited | in_env + `CARGO_HOME=<env>` + `RUSTUP_HOME=<tmp>` |
| rust | `cargo add …`, `cargo install --bins --root <env> …` | prefix | in_env (+ `RUSTUP_HOME=<tmp2>` non-system) |
| ruby | `rbenv init -`, `rbenv download V` / `rbenv install V`, `rbenv rehash`, `gem build`, `gem install …` | prefix | in_env |
| docker | `docker build …` | prefix | process env |
| (all) | `get_default_version` probes: `cargo --version` | `/` | process env |

**`no_git_env(env)`** (`git.py:27-48`) drops every `GIT_*` variable **except**:
* those with the prefixes `GIT_CONFIG_KEY_` and `GIT_CONFIG_VALUE_`;
* the names `GIT_EXEC_PATH`, `GIT_SSH`, `GIT_SSH_COMMAND`, `GIT_SSL_CAINFO`, `GIT_SSL_NO_VERIFY`, `GIT_CONFIG_COUNT`, `GIT_HTTP_PROXY_AUTHMETHOD`, `GIT_ALLOW_PROTOCOL`, `GIT_ASKPASS`.

Only golang install and store git operations use it (4.7.0 adds node's `npm install`, ADR-089). pip and (before 4.7.0) npm installs inherit the `GIT_*` vars that git exported, such as `GIT_INDEX_FILE` and `GIT_DIR` in worktrees. prek found this breaks `setuptools_scm` (prek CHANGELOG "Remove git env vars from `uv pip install` subprocess", #1355).

### 10.4 Third-party variables that change backend behaviour (inherited, honoured by the tools)

* **python:** `VIRTUALENV_*` and `virtualenv.ini`. `PIP_*` (`PIP_INDEX_URL`, `PIP_EXTRA_INDEX_URL`, `PIP_TRUSTED_HOST`, `PIP_FIND_LINKS`, `PIP_NO_INDEX`, `PIP_CERT`, `PIP_CLIENT_CERT`, `PIP_PROXY`, `PIP_CONSTRAINT`, `PIP_PRE`, `PIP_ONLY_BINARY`, `PIP_NO_BINARY`, `PIP_NO_BUILD_ISOLATION`, `PIP_CONFIG_FILE`, `PIP_TIMEOUT`, `PIP_RETRIES`, `PIP_KEYRING_PROVIDER`), and `pip.conf` (`$PIP_CONFIG_FILE`, `~/.config/pip/pip.conf`, `~/.pip/pip.conf`, `/etc/pip.conf`, `%APPDATA%\pip\pip.ini`, `$VIRTUAL_ENV/pip.conf`). Also `netrc` and `keyring`.
* **node:** `npm_config_*` / `NPM_CONFIG_*` (registry, cafile, proxy, …), the user `~/.npmrc` (it **is** read: only the env override vars are unset), and a project `.npmrc` **in the prefix** (the hook repo may ship one). nodeenv config files (§3.4).
* **golang:** `GOPROXY`, `GONOSUMDB`, `GOPRIVATE`, `GOFLAGS`, `GOINSECURE`, `GONOPROXY`, `GOSUMDB`, `CGO_ENABLED`, `CC`. The user `go env -w` file is `$GOENV`, by default `~/.config/go/env`.
* **rust:** `CARGO_*` (`CARGO_REGISTRIES_*`, `CARGO_NET_GIT_FETCH_WITH_CLI`, `CARGO_TARGET_DIR`!), `~/.cargo/config.toml`, `RUSTUP_DIST_SERVER`, `RUSTUP_UPDATE_ROOT`, `RUSTUP_AUTO_INSTALL`.
* **ruby:** `GEMRC`, `BUNDLE_*` (except via `BUNDLE_IGNORE_CONFIG`), `RBENV_*`, `RUBY_BUILD_*`, `RVM_BINARIES_BASE`.
* **docker:** `DOCKER_HOST`, `DOCKER_CONTEXT`, `DOCKER_CONFIG`, `DOCKER_TLS_VERIFY`, `DOCKER_BUILDKIT`.
* **downloads** (Python urllib, used by golang, rustup and nodeenv): `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY` (lower and upper case); `SSL_CERT_FILE` and `SSL_CERT_DIR` via OpenSSL defaults.

---
## 11. Python environments with `uv`: evaluation and recommended design

### 11.1 Baseline (pre-commit)

* Env creation is `virtualenv` (a Python library inside pre-commit's own venv), followed by `pip install . *deps` (§2.3).
* **Costs** (observed on this host):
  * venv: ≈ 0.25–0.4 s;
  * pip install: ≈ 1.3 s even for a tiny warm-cache package; multi-second for real hooks such as flake8+plugins, mypy+stubs or black;
  * no parallel downloads; a wheel cache, but no resolution cache.
* snp has no host Python. It **must** either shell out to an interpreter for env creation, or use `uv`.

### 11.2 What prek does (high level; do not copy code)

* **Finding uv**, in order:
  1. `uv` next to the prek binary;
  2. `uv` on PATH, if its version satisfies `>=0.7.0`;
  3. a managed uv in prek's tools dir, validated by running `uv --version`.
* **Installing uv** (pinned at 0.12.19 at the time of writing) when none is found:
  1. Astral's CDN (`https://releases.astral.sh/github/uv/releases/download/<v>/<archive>`).
  2. PyPI and the mirrors tuna, aliyun and tencent (the wheel's `uv` binary is extracted).
  3. `pip install uv==<v>` as the last resort.
  * Install is under a file lock. A static-musl fallback handles NixOS's loader path.
  * `PREK_UV_SOURCE` selects the source: `none|astral|github|pypi|tuna|aliyun|tencent|pip|<url>`. `uv` downloads are **not** checksum-verified (`docs/diff.md`).
* **Venv:** `uv venv <env> --no-project --project / [--python <req>] …`, with `UV_PYTHON` and `UV_SYSTEM_PYTHON` removed.
  * Attempt order depends on the preference:
    1. prek-managed interpreters only (`--python-preference only-managed`, `UV_PYTHON_INSTALL_DIR=<prek tools>/python`, `--no-python-downloads`);
    2. external interpreters (`--no-python-downloads`);
    3. download (`--allow-python-downloads` into prek's store) if the policy allows.

    It retries with downloads only when uv's stderr says `A managed Python download is available`.
  * **No `--seed`**, so prek venvs have no pip. This is a compatibility gap.
* **Install:** `uv pip install --project / --directory <repo> . *deps` (or just `*deps` for local hooks; **nothing** for local hooks without deps).
  * `VIRTUAL_ENV=<env>`, with the `GIT_*` repo env sanitized and `UV_PYTHON`/`UV_SYSTEM_PYTHON` removed.
  * `UV_CACHE_DIR=<prek cache>/uv`: an **isolated cache**, not the user's.
* **Health:** re-query the interpreter version (cached) and compare it with the stored install info. pyvenv.cfg is not used.
* **Extensions:** `language_version` semver ranges (`>=3.9,<3.13`), wheel-style `312`/`python312`, a preference object (`only-managed|managed|system|only-system`), `requires-python` inference from the hook repo's `pyproject.toml`, and PEP 723 inline script metadata for local hooks without deps.
* **pygrep:** prek spawns a Python and keeps Python `re` semantics. snp instead goes native (§12).

### 11.3 Compatibility gaps: uv vs. virtualenv + pip

| # | Gap | Impact | Mitigation in snp |
|---|---|---|---|
| G1 | `uv venv` seeds **nothing** by default. virtualenv seeds pip, plus setuptools and wheel on Python < 3.12. | Hooks that call `pip` (`mypy --install-types`, some plugin installers) break. | Always pass **`--seed`**. uv then seeds pip; per uv's docs it also seeds setuptools and wheel only on Python < 3.12, matching virtualenv (observed: pip only on 3.13; < 3.12 is unverified here). The cost is small with a warm cache. |
| G2 | uv ignores `pip.conf` and `PIP_*`. | Corporate mirrors and private indexes silently stop working, and installs fail or reach the wrong index (a security problem). | Translate (§11.5). If an untranslatable pip setting is present, **fall back to the pip backend** for that env and log it at debug level. |
| G3 | Index strategy: pip merges candidates across `--extra-index-url`; uv uses the first index that has the package. | A different version may be resolved, or dependency confusion behaves differently. | When translating `PIP_EXTRA_INDEX_URL` or `--extra-index-url`, set `UV_INDEX_STRATEGY=unsafe-best-match` (pip semantics). |
| G4 | `additional_dependencies` items that are **pip options** go into the argv. | uv accepts `--pre`, `--no-deps`, `-r`, `-e`, `-c`, `--index-url=`, `--extra-index-url=`, `--trusted-host=`, `--only-binary=`, `--no-binary=`, `--no-build-isolation`, `--no-cache-dir`, `--isolated`, `-q`. It **rejects** `--user`, `--progress-bar=`, `--use-pep517` (observed with uv 0.8.8). | If any dep starts with `-` and is not in a verified allowlist, use the pip backend for that env. |
| G5 | pyvenv.cfg format: uv writes `version_info = 3.12.3`, adds `uv = …` (and `seed = true` with `--seed`), and has no `base-executable`. | pre-commit's `health_check` rejects uv venvs, so a shared store would make pre-commit rebuild them (env flip-flop). | snp's health check accepts both formats (§14.9). Optional compat mode (open question Q3): after `uv venv`, rewrite `version_info` to `X.Y.Z.<releaselevel>.<serial>` and add `base-executable`. |
| G6 | Interpreter discovery: uv prefers **uv-managed** Pythons. `python3` resolved to a managed 3.13.6 rather than `/usr/bin/python3` (3.12.3) (observed). uv does not expand `~`. | A different Python than pre-commit would pick. | snp resolves the interpreter **itself** (§11.4 step 2) and passes an **absolute path** to `uv venv --python`. uv discovery is used only as a fallback. |
| G7 | Resolver differences: pre-release handling, yanked versions, `--no-build-isolation` edge cases, and the order of `Requires-Dist` evaluation. | Rare resolution differences. | Accept. Document. Offer `SNP_PYTHON_INSTALLER=pip` as an escape hatch. |
| G8 | `UV_*` user env vars (`UV_RESOLUTION=lowest-direct`, `UV_PYTHON`, `UV_SYSTEM_PYTHON`, `UV_PROJECT_ENVIRONMENT`, `UV_CONSTRAINT`, `UV_OVERRIDE`) change behaviour. | Surprising failures (prek documents this). | Strip the dangerous ones and keep the network, auth, cache and TLS ones (§11.5). |
| G9 | Project config discovery: `uv pip` reads `uv.toml` and `pyproject.toml [tool.uv]` from the cwd and its ancestors, i.e. the hook repo. | The hook repo could redirect indexes. Low risk, since the repo already runs arbitrary code at install, but it reduces predictability. | Pass `--project /` like prek. User-level `~/.config/uv/uv.toml` stays honoured. Alternative: `--no-config` plus translation (open question Q4). |
| G10 | Hard-links and the cache: uv hard-links from its cache by default (clone on macOS), falling back to copy across filesystems. | A venv shares inodes with the cache. Running `uv cache clean` is safe; mutating installed files in place (rare) would corrupt the cache. | Use the default link mode. Allow `UV_LINK_MODE=copy` passthrough. |
| G11 | `uv` reads `VIRTUAL_ENV` and `CONDA_PREFIX` when no `--python` is given. | It may install into the wrong env. | Always pass `--python <env python>` to `uv pip install`, and set `VIRTUAL_ENV=<env>`. |

### 11.4 Recommended snp design for `language: python`

1. **Choose the installer** (per env, at install time). `SNP_PYTHON_INSTALLER = auto | uv | pip` (default `auto`):
   * `auto`: use uv if it is obtainable (step 3) and none of the G2/G4 fallback triggers fire. Otherwise use pip.
2. **Resolve the interpreter.** This snp-owned logic keeps pre-commit's choices:
   1. **Reference interpreter.** This stands in for pre-commit's own `sys.executable`. When the resolved version is literally `default` (only possible when `get_default_version()` itself returned `default`), use the reference interpreter's **base** executable. If there is none, use uv discovery, then download (below).
      * The reference interpreter is the first of `python3` or `python` on PATH. On Windows: `py -3`, then `python`.
      * Query it once with `-S -c 'import sys;print(sys.implementation.name, *sys.version_info[:3], sys.executable)'`, and cache the result **across runs** in the store, keyed by `(path, mtime, size)`.
   2. `get_default_version()` for python:
      * Try `python{X.Y}`, then `python{X}`, using `find_executable` (PyPy uses `pypy{X.Y}`).
      * Otherwise use the reference exe's basename if it is not `python`/`pythonw`.
      * On Windows, try the `py -X.Y` launcher.
      * Otherwise return `default`.

      This reproduces pre-commit's env-dir names, e.g. `py_env-python3.12`.
   3. Map `language_version` to an interpreter:
      * `python`, or `pythonX[.Y[.Z]]` that prefix-matches the reference interpreter: use the reference interpreter's **base** executable.
      * Otherwise, on Windows, try `py -X.Y`.
      * Otherwise, if `find_executable(version)` succeeds, use it. Note that pre-commit only does this lookup on win32; elsewhere virtualenv does it.
      * Otherwise, if `expanduser(version)` is an existing file, use the path.
      * Otherwise fall back to **uv discovery**: `uv python find --no-project <normalized>` with `--python-preference system`, then managed. Normalize `pythonX.Y` to `X.Y`, `pypyX.Y` to `pypy@X.Y`, and pass anything else verbatim, e.g. `3.11` or `cpython3.11`.
      * Otherwise **download** a managed Python, only if allowed (open question Q2; default proposal: allowed unless `SNP_PYTHON_DOWNLOADS=never`), into `$SNP_HOME/tools/python` via `UV_PYTHON_INSTALL_DIR`.
      * Otherwise fail with an error.
      * `system` is **not** special; it fails like pre-commit, with a clear message.
3. **Obtain uv.** Stop at the first that applies:
   1. `$SNP_UV` (explicit path);
   2. `uv` next to the snp executable;
   3. `uv` on PATH with version ≥ the floor (see Q5; prek uses 0.7.0);
   4. `$SNP_HOME/tools/uv/<pinned>/uv[.exe]`;
   5. download, unless `SNP_UV_DOWNLOAD=0` or offline: a pinned version from GitHub releases or `releases.astral.sh`, verified against **sha256 values embedded in the snp binary** for each target triple (the trust root is the snp release). Extract to a temp dir and `rename` atomically, under a file lock.
4. **Create the env.** Under the per-env lock, after removing any leftover dir:
   ```
   uv venv <envdir> --python <abs interpreter> --seed --no-project --project / --no-python-downloads --quiet
   ```
   * cwd: `<envdir>`'s parent.
   * Env: the base env minus `UV_PYTHON`, `UV_SYSTEM_PYTHON`, `UV_PROJECT_ENVIRONMENT`, `VIRTUAL_ENV`, `CONDA_PREFIX`, `__PYVENV_LAUNCHER__` and `PYTHONEXECUTABLE`.
   * Set `UV_CACHE_DIR` only if the user has not (open question Q6: shared vs. isolated cache).
5. **Install.** Skip this step entirely when the hook is **local and has no deps**. That avoids the placeholder-package problem; the only observable difference is that `pre_commit_placeholder_package` is not installed. Otherwise:
   ```
   uv pip install --python <envdir>/<bin>/python --project / [translated flags] <.?> *deps
   ```
   * cwd: the prefix.
   * Env: `no_git_env(base)` + `VIRTUAL_ENV=<envdir>` + `PIP_DISABLE_PIP_VERSION_CHECK=1` + the translated `UV_*` vars, minus the stripped `UV_*` vars.
   * `.` is included for remote repos, **and** for local repos when the pip backend is used (for pre-commit parity).
6. **pip backend** (fallback or explicit choice):
   1. Create the env with `<interp> -m venv --without-pip <envdir>` followed by `ensurepip`, or simply `<interp> -m venv <envdir>`. Prefer `<interp> -m virtualenv <envdir>` if virtualenv is importable (exact parity).
   2. Then `<envdir>/<bin>/python -m pip install . *deps`, exactly as in §2.3 (cwd=prefix, in_env).
   3. Note: Debian/Ubuntu system Pythons without `python3-venv` fail `-m venv` with `ensurepip is not available`. Detect this and fall back to uv, or print an actionable message.
7. **State.** After success, write snp state (§14.9): interpreter path, `version_info` string, base-exe `(path, mtime, size, ino)` and the installer used. Also write `.install_state_v1` and `.install_state_v2` exactly as pre-commit does (§1.3).
8. **in_env** is identical to pre-commit (§2.4).

### 11.5 Translating pip configuration to uv (env and CLI)

| pip source (env var or `pip.conf` `[global]`/`[install]` key) | uv equivalent |
|---|---|
| `PIP_INDEX_URL` / `index-url` | `UV_DEFAULT_INDEX` (or legacy `UV_INDEX_URL`) |
| `PIP_EXTRA_INDEX_URL` / `extra-index-url` (whitespace-separated) | `UV_INDEX` (space-separated) **and** `UV_INDEX_STRATEGY=unsafe-best-match` |
| `PIP_FIND_LINKS` / `find-links` | `UV_FIND_LINKS` |
| `PIP_NO_INDEX` / `no-index` | `--no-index` flag (no env var) |
| `PIP_TRUSTED_HOST` / `trusted-host` | `UV_INSECURE_HOST` |
| `PIP_CERT` / `cert` | `SSL_CERT_FILE` (plus `UV_NATIVE_TLS=1` if it points at the system store) |
| `PIP_CLIENT_CERT` / `client-cert` | `SSL_CLIENT_CERT` |
| `PIP_PROXY` / `proxy` | `HTTPS_PROXY` and `HTTP_PROXY` (if not already set) |
| `PIP_PRE` / `pre` | `UV_PRERELEASE=allow` |
| `PIP_CONSTRAINT` / `constraint` | `UV_CONSTRAINT` |
| `PIP_NO_BUILD_ISOLATION` / `no-build-isolation` (note pip's inverted boolean quirk) | `UV_NO_BUILD_ISOLATION` |
| `PIP_ONLY_BINARY=:all:` / `PIP_NO_BINARY=:all:` | `UV_NO_BUILD=1` / `UV_NO_BINARY=1` (per-package values: use the CLI `--only-binary` / `--no-binary`) |
| `PIP_TIMEOUT` / `timeout` | `UV_HTTP_TIMEOUT` |
| `PIP_KEYRING_PROVIDER` / `keyring-provider` | `UV_KEYRING_PROVIDER` |
| `PIP_REQUIRE_HASHES` | `UV_REQUIRE_HASHES` |
| anything else that changes resolution or install (`PIP_USE_FEATURE`, `PIP_IGNORE_INSTALLED`, `PIP_TARGET`, `PIP_PREFIX`, `PIP_USER`, …) | **fallback to the pip backend** |

* **Config file search order** (pip's), later files override earlier ones:
  1. site config: `/etc/pip.conf`, `$XDG_CONFIG_DIRS/pip/pip.conf`, `C:\ProgramData\pip\pip.ini`;
  2. user config: `~/.config/pip/pip.conf` and the legacy `~/.pip/pip.conf`; `%APPDATA%\pip\pip.ini`;
  3. `$PIP_CONFIG_FILE`;
  4. the env vars, which override everything.

  `PIP_CONFIG_FILE=os.devnull` disables the files.
* **`UV_*` handling:**
  * **Keep:** `UV_CACHE_DIR`, `UV_NO_CACHE`, `UV_OFFLINE`, `UV_DEFAULT_INDEX`, `UV_INDEX`, `UV_INDEX_URL`, `UV_EXTRA_INDEX_URL`, `UV_FIND_LINKS`, `UV_INDEX_STRATEGY`, `UV_INSECURE_HOST`, `UV_NATIVE_TLS`, `UV_KEYRING_PROVIDER`, `UV_HTTP_TIMEOUT`, `UV_CONCURRENT_*`, `UV_LINK_MODE`, `UV_PYTHON_INSTALL_MIRROR`, `UV_PYPY_INSTALL_MIRROR`, `UV_REQUEST_TIMEOUT`, `UV_COMPILE_BYTECODE`, `UV_NO_PROGRESS`.
  * **Strip:** `UV_PYTHON`, `UV_SYSTEM_PYTHON`, `UV_PROJECT_ENVIRONMENT`, `UV_PROJECT`, `UV_MANAGED_PYTHON`, `UV_NO_MANAGED_PYTHON` (snp passes explicit flags), `UV_CONSTRAINT`, `UV_OVERRIDE`, `UV_BUILD_CONSTRAINT`, `UV_RESOLUTION`, `UV_PRERELEASE` (unless translated from pip), `UV_TORCH_BACKEND`, `UV_VENV_SEED`, `UV_VENV_CLEAR`, `UV_CONFIG_FILE`, `UV_NO_CONFIG`.

### 11.6 Expected wins

* Env creation: ≈ 15–30 ms instead of ≈ 250–400 ms.
* Installs: 3–30× faster with a warm cache (observed: 0.4 s vs 1.3 s for `six`; much larger for dependency-heavy hooks).
* Parallel network fetches.
* Combined with installing **different hook envs in parallel** under per-env locks (pre-commit installs them sequentially under one store lock), first-run install of a 10-hook config typically drops from minutes to seconds.

---

## 12. Native `pygrep` in Rust: specification

### 12.1 Goals

* For every pattern that Python's `re` (bytes mode) accepts and that snp supports: produce **byte-identical stdout** and the **same exit code** as `python -m pre_commit.languages.pygrep`.
* Run in-process. No Python needed.
* Never hang. Linear-time engine where possible; a backtracking engine with a step limit otherwise.
* For unsupported constructs, fail clearly with exit 1. There is an optional fallback to an external Python (Q7).

### 12.2 Invocation model

* snp builds the argv-equivalent `[*args, entry, *files]` and parses it with an **argparse emulation**. It does **not** apply xargs partitioning; it processes all files in one pass.
  * Output order equals the file order. pre-commit's pygrep path is unshuffled and serial.
  * The exit code is the OR of the per-file results.
* argparse emulation rules (matching CPython 3.12 argparse):
  * Options are `-h`/`--help`, `-i`/`--ignore-case`, `--multiline` and `--negate`. Long options accept **unique prefixes** (`--m`, `--multi`, `--neg`, `--ign`, `--he`).
  * `--opt=value` on these store_true flags fails with: `argument --multiline: ignored explicit argument '1'` (exit 2).
  * Short flags combine: `-ii` is ok, and so is `-ih`, which prints help.
  * `--` ends option parsing.
  * An unknown token starting with `-` (but not `-<digits>`, because the parser has no negative-number-like options) is recorded as unrecognized.
  * Positionals: the first is `pattern`; the rest are `filenames`. Options may be interleaved, **but** argparse fills `filenames` greedily at the first positional run. So with `x -i a.txt`, `a.txt` is reported as an unrecognized argument (exit 2).
  * Errors go to the output stream (the hook output merges stderr), exit 2, exactly:
    ```
    usage: pygrep.py [-h] [-i] [--multiline] [--negate] pattern [filenames ...]
    pygrep.py: error: unrecognized arguments: <space-joined tokens>
    ```
    With no pattern: `pygrep.py: error: the following arguments are required: pattern`.
  * `-h` prints this help, exit 0 (wrapping as observed, at 80 columns):
    ```
    usage: pygrep.py [-h] [-i] [--multiline] [--negate] pattern [filenames ...]

    grep-like finder using python regexes. Unlike grep, this tool returns nonzero
    when it finds a match and zero otherwise. The idea here being that matches are
    "problems".

    positional arguments:
      pattern            python regex pattern.
      filenames

    options:
      -h, --help         show this help message and exit
      -i, --ignore-case
      --multiline
      --negate
    ```
* A real improvement: snp knows which argv elements are files, so filenames starting with `-` are **never** parsed as options. This is a deliberate, harmless deviation.

### 12.3 Python `re` (bytes) to Rust translation

> **Partly superseded → 01 Addendum (§A.6, "Pick one regex engine…", gap-fill #5).**
>
> The two-tier design below stands, and it is now the project-wide decision. One shared translator serves `str` and `bytes`. Corrections measured with regex 1.13.1 and fancy-regex 0.19.2:
> - **(1) Tier 2 must not use `BytesMode::Ascii`.** In that mode `\xHH` ≥ 0x80 means U+00HH as UTF-8 (`\xFF` does not match byte `0xFF`), and `[\x80-\xFF]` fails to compile. Run tier 2 on the **Latin-1 lane** instead: fancy in Unicode mode over a byte→char transcoded haystack, with the pattern emitted as `\xHH`.
> - **(2) Bytes literals are always the 2-digit `\xHH`.** `regex::bytes` reads braced `\x{FF}` as U+00FF.
> - **(3) IGNORECASE uses explicit ASCII classes (`[aA]`), never `(?i:…)`.** On the Latin-1 lane, `(?i)` would fold é/É.
> - **(4) `\z` is rejected** (`bad escape \z`), because CPython 3.12 is the target.
> - **(5) Tier-2 `\b`/`\B` are ASCII look-arounds.** fancy rejects `(?-u:…)`.
> - **(6) The relaxed tier-1 prefilter is mandatory for tier 2** (40–1,200× faster), and `backtrack_limit` is `100_000_000`. The 1,000,000 default fails on a real pattern.
> - **(7) An invalid entry is reported once.** pre-commit prints one traceback per xargs partition, which is a Tier B difference.
>
> The rows and probe results below are kept for reference.

Write a small **Python-regex parser**: a port of `sre_parse`'s grammar, **not** of its code. It runs over the UTF-8 bytes of the entry and produces an AST. A code generator then emits a pattern for one of two engines:

* **Tier 1:** `regex::bytes::RegexBuilder::new(p).unicode(false)` (linear time). It is used when the AST has no backtracking-only features.
* **Tier 2:** `fancy_regex::RegexBuilder` with `bytes_mode(BytesMode::Ascii)` (fancy-regex ≥ 0.19) and `backtrack_limit(N)`. It is used for lookaround, backreferences, atomic groups, possessive quantifiers and conditionals.

The generator emits **fully normalized** syntax: every literal byte becomes `\xHH`, flags are expanded, and there is no `(?x)`. This avoids every Rust-vs-Python syntax ambiguity.

| Python construct (bytes pattern) | Python semantics | Emit | Tier |
|---|---|---|---|
| literal char (ASCII) | byte | `\xHH` | 1 |
| non-ASCII char in pattern | its **UTF-8 bytes as separate literals**. A following quantifier applies to the **last byte only** (`é+` is `\xC3\xA9+`). | `\xC3\xA9` | 1 |
| `\a \f \n \r \t \v` | bytes 07 0C 0A 0D 09 0B | `\xHH` | 1 |
| `\` + ASCII non-alnum, e.g. `\.`, `\/`, `\<`, `\>`, `\-`, `\ ` | literal | `\xHH`. **Never** emit `\<` / `\>`: Rust treats them as word-boundary assertions, so `\<x` matched at (1,2) not (0,2) (probe). | 1 |
| `\` + unknown ASCII letter (`\q`, `\u`, `\U`, `\N`, `\p`, `\e`, `\g`, `\k`, `\z` before 3.14) | error `bad escape \X` | error | – |
| `\z` | Python ≥ 3.14: end of string. Python ≤ 3.13: error. | `\z` (accept, superset) | 1 |
| `\xHH` (exactly 2 hex digits) | byte | `\xHH`. Fewer digits is the error `incomplete escape \x…`. | 1 |
| `\0`, `\0o`, `\0oo` | octal byte | `\xHH` | 1 |
| `\NNN`: exactly **three octal digits**, the first being 1–7 (e.g. `\101`) | octal byte. A value > 0o377 (e.g. `\477`) is the error `octal escape value \477 outside of range 0-0o377`. | `\xHH` | 1 |
| inside a class: `\0`–`\7` followed by up to 2 more octal digits | octal byte (never a backreference). `\8` and `\9` inside a class are the error `bad escape \8`. | `\xHH` | 1 |
| `\1`..`\99` (otherwise) | backreference. An invalid or still-open group is an error (`invalid group reference N`, `cannot refer to an open group`). | `\N` | 2 |
| `\d \D \s \S \w \W` | ASCII classes (`\s` is ` \t\n\r\f\v`) | same (unicode off makes them ASCII) | 1 |
| `\b`, `\B` (outside class) | ASCII word boundary | `\b`, `\B` | 1 |
| `\A` | start of string | `\A` | 1 |
| `\Z` | **absolute** end of string (Python ≠ PCRE) | `\z`. **Never** emit `\Z`: regex rejects it and fancy-regex gives it PCRE semantics (probe: `foo\Z` matched `foo\n`). | 1 |
| `.` | any byte except `\n`; any byte with DOTALL | `[^\n]` or `(?s:.)` | 1 |
| `^` | MULTILINE: start or after any `\n`. Otherwise: start of string. | `(?m:^)` or `\A` | 1 |
| `$` | MULTILINE: before any `\n`, or at end. Otherwise: at end **or before a final `\n` at end**. | MULTILINE: `(?m:$)`. Non-MULTILINE: in **by-line mode** `(?m:$)` (exact, because a line holds at most one trailing `\n`); in whole-file mode `(?=\n?\z)`. | 1 / 2 |
| `[...]`, `[^...]` | class over bytes. `]` first is literal. `-` first or last is literal. Escapes as above; `\b` inside means backspace 0x08. Octal inside; `\1`..`\7` inside is **octal**, not a backref. `[[:alpha:]]` is **not** POSIX: it means `[[:alph]` followed by `]`. A nested `[`, `&&`, `--`, `~~`, `\|\|` are literal (FutureWarning only). | Rebuild from items as `[\xHH\xHH-\xHH…]`. Non-ASCII chars expand to their bytes. With IGNORECASE, letters fold to ASCII only. Error on reversed ranges: `bad character range z-a`. | 1 |
| `( … )`, `(?: … )` | groups | same | 1 |
| `(?P<name> … )` | named group (name: ASCII identifier) | `(?P<name>…)` | 1 |
| `(?P=name)` | named backreference | `(?P=name)` | 2 |
| `(?#…)` | comment | dropped | 1 |
| `(?=…)`, `(?!…)` | lookahead | same | 2 |
| `(?<=…)`, `(?<!…)` | lookbehind; Python needs **fixed width** (`look-behind requires fixed-width pattern`) | same. Optionally reject variable width for parity. | 2 |
| `(?(1)yes\|no)`, `(?(name)yes\|no)` | conditional | **always emit the numeric form** `(?(N)yes\|no)`; the translator maps names to group numbers. Probe: fancy 0.19 compiles `(?P<n>a)?(?(n)b\|c)` but returns **no match** on `ab`, while Python matches (0,2); the numeric form matches correctly. | 2 |
| `(?>…)` (3.11+) | atomic group | `(?>…)` | 2 |
| `*+ ++ ?+ {m,n}+` (3.11+) | possessive | route to fancy. Rust `regex` silently accepts `a*+` with **different** semantics (probe). | 2 |
| `* + ? {m} {m,} {,n} {m,n}` and lazy `?` | quantifiers. Python treats `{,}` as `{0,}` (i.e. `*`) and `{}` as a literal; `m > n` is the error `min repeat greater than max repeat`. | `{,n}` becomes `{0,n}` and `{,}` becomes `{0,}` (regex rejects `{,n}`). A count ≥ 4294967295 is the error `the repetition number is too large`. `{` not forming a valid quantifier is a **literal** `{` (`a{`, `a{x}`, `{1,2`), so emit `\x7B`. | 1 |
| `*` at start, or after `\|`, `(` | error `nothing to repeat` | error | – |
| `a**`, `a{2}{3}` | error `multiple repeat` | error | – |
| `\|` | alternation, empty alternatives allowed | same | 1 |
| inline global flags `(?aiLmsux)` | must be at the **start** (3.11+), else `global flags not at the start of the expression`. `a` and `L` have no effect for bytes. `u` is the error `bad inline flags: cannot use 'u' flag with a bytes pattern`. `x` is verbose. | track the flag state; never emit `(?x)` | 1 |
| scoped flags `(?i-ms:…)` | `i`, `m`, `s` (and `x`) on or off within the scope | expand per construct: `(?i:…)` for literals and classes; `^ $ .` emitted per the current state | 1 |
| verbose (`x`) | whitespace and `#…\n` ignored **outside** classes; whitespace inside classes is **literal**; `\ ` is a literal space. Rust's `(?x)` ignores whitespace inside classes too (probe: `(?x)[a b]` did not match `' '`). | handled by the parser | 1 |
| empty pattern | matches everywhere | (empty) | 1 |
| `-i` flag | IGNORECASE: ASCII-only folding for bytes | wrap everything in `(?i:…)` | 1 |
| `--multiline` | `MULTILINE \| DOTALL` | initial flag state `m=1, s=1` | 1 |

**Error reporting:**
* Mirror Python's message texts and the `at position N` suffix for the errors listed above, where N is a **char** offset into the pattern string.
* Print as one line, `re.error: <msg> at position <N>`, then exit 1. This is the last line of Python's traceback.
* If the pattern needs Tier 2 and fancy-regex rejects it, or if any tier hits a compile size limit, print `snp: pygrep: unsupported regular expression construct: <detail>` and exit 1. See Q7 for the Python fallback.

**Engine probe results** (regex 1.13.1, fancy-regex 0.19.2, run in `scratchpad/rxprobe`):

| Check | Result |
|---|---|
| regex `foo$` vs `foo\n` | no match. `foo(?m:$)` gives (0,3). `(?m)^$` on `foo\n` gives (4,4), same as Python. |
| `\Z`, `{,3}`, `(?#c)`, `(?P=n)`, `(?>a)`, `(?a)`, `(?L)`, `\0`, `\012`, `[\b]`, `\N{…}`, `[[]` | regex errors |
| `\<x` | word boundary, not a literal |
| `[[:alpha:]]`, `[a-z&&[^b]]`, `[a--b]`, `[a~~b]` | interpreted as POSIX classes or set operations (differs from Python) |
| `\u0041`, `\U00000041`, `\p{L}` | accepted (Python bytes rejects them) |
| `(?-u)caf\w` vs `café` | no match, same as Python |
| `(?-u)(?i)café` vs `CAFÉ` | no match, same as Python |
| `(?-u)x[é]` | error `Unicode not allowed here`, so the generator must emit bytes |
| `(?-u)a.b` vs `a\xffb` | match. Unicode-mode `.` does **not** match invalid UTF-8, so unicode must stay off. |
| `\s` | `\x0b` matches and `\x1c` does not, same as Python |
| `a\|ab` on `ab` | (0,1), same as Python |
| fancy (`BytesMode::Ascii`) | supports lookahead, lookbehind (even variable-width), backrefs, `(?P=n)`, `(?#…)`, `{,3}`, atomic, possessive, conditionals; inline `(?u)` is rejected (`Changing Unicode mode inline is not supported`) |
| `\Z` under fancy | PCRE semantics, so emit `\z` |
| `\<` under fancy | still a word boundary, so emit `\x3C` |
| `(?i:\x41)` vs `a`, `(?i:[\x41-\x43])` vs `b` | match in both engines: case folding applies to hex-escaped ASCII. `(?i:\xC9)` vs `\xe9` gives no match in both, same as Python bytes (ASCII-only folding). |
| `(?i)(a)\1` vs `aA` | fancy matches (0,2), same as Python (case-insensitive backref) |
| `foo(?=\n?\z)` vs `foo\n` | fancy matches (0,3): the whole-file emission for non-MULTILINE `$` works |
| **pygrep-hooks** patterns (10 hooks, `pre-commit/pygrep-hooks` main) | **6 of the 10 need Tier 2**: `python-check-blanket-noqa` `(?i)# noqa(?!: )`, `python-check-blanket-type-ignore` `# type:? *ignore(?!\[\|\w)`, `python-check-mock-methods` (a `(?x)` verbose pattern with `#` comments containing parentheses, and six `(?<!\bMock)` lookbehinds), `python-no-log-warn` `(?<!warnings)\.warn\(`, `python-use-type-annotations` `# type(?!: *ignore([^a-zA-Z0-9]\|$))`, `rst-backticks` ``^(?!    ).*(^\| )`[^`]+`([^_]\|$)``. The other 4 are Tier 1: `\beval\(`, `^\s*\.\. [a-z]+(\| \| :):$`, ``\w``\w``, and U+FFFD (a 3-byte literal). The 4 Tier-2 patterns probed directly in fancy (noqa, type-ignore, warn, rst-backticks) matched as Python does. mock-methods depends on the translator's verbose-mode handling. |

### 12.4 Matching procedures (must reproduce §8.4 byte-for-byte)

The output is assembled per file into a `Vec<u8>`, and the per-file buffers are written **in input order**.

* **By-line, not negated:**
  1. Split the content on `b'\n'`, keeping the `\n` in each line. The last segment has no `\n` and is processed only if non-empty. An empty file has no lines.
  2. Line numbers start at 1.
  3. For each line where `is_match(line)` holds, emit `{filename}:{n}:` + `line` with trailing `\r`/`\n` bytes stripped (a run of any mix of them) + `\n`.
  4. The result is 1 if any line matched.
* **By-line, negated:** the result is 0 at the first matching line. Otherwise emit `{filename}\n` and the result is 1.
* **Whole-file, not negated:**
  1. Take `m = find(contents)`. If there is none, the result is 0.
  2. `ln = count(b'\n' in contents[..m.start])`.
  3. `first = contents.split(b'\n').nth(ln)`: the full line, **keeping `\r`**.
  4. `parts = m.as_bytes().split(b'\n')`, then set `parts[0] = first`.
  5. Emit `{filename}:{ln+1}:` + `parts.join(b'\n')` + `\n`. The result is 1.
* **Whole-file, negated:** if there is no match, emit `{filename}\n` and the result is 1. Otherwise 0.
* **Filenames:** the exact bytes of the argv string. Use `OsStr::as_encoded_bytes()` on Unix; Windows uses UTF-8 (pre-commit encodes `str` as UTF-8).
* **I/O errors:** on a missing file or a directory, emit an error line and return 1. Suggested format: `{filename}: {io error}`; Python prints a traceback whose last line is e.g. `FileNotFoundError: [Errno 2] No such file or directory: 'x'`. pre-commit stops processing the **remaining files of that partition**. snp continues with the other files: a deliberate deviation, see Q8.
* **Performance:**
  * Compile once.
  * Parallelize per file (rayon, or the run engine's pool) while preserving output order.
  * Read files with `std::fs::read`, or a streaming `read_until` for by-line mode with a size threshold. Avoid `mmap`, because `memmap2` requires `unsafe`.
  * Use `memchr` for line splitting.
  * **Tier 1 by-line prefilter (optional):** run `find_iter` once over the whole buffer with an *anchor-relaxed* variant (every `\A`, `^` becomes `(?m:^)` and `$`, `\z` becomes `(?m:$)`). Then verify only the candidate lines with the exact per-line regex. This is valid because every line-match is also a buffer-match under these rewrites. Do not use it for Tier 2: lookbehind can see the previous line.

### 12.5 Test vectors

Use every row of the §8.4 table, plus these:
* the 10 pygrep-hooks entries against fixture files;
* argparse cases: `--m`, `--neg`, `--ign`, `-ii`, `--multiline=1` (exit 2), `-h` (exit 0);
* errors: `(` gives `missing ), unterminated subpattern at position 0` and `(?u)x` gives `bad inline flags: cannot use 'u' flag with a bytes pattern at position 3`;
* bytes: invalid UTF-8 content and the `\xff` byte;
* CRLF in both modes;
* empty file in all 4 modes;
* the last line without `\n`.

Generate expected outputs by running the reference module **at test-authoring time**, then commit them as golden files. CI does not need Python.

---
## 13. Ideas from prek and lefthook for language backends (opt-in extensions)

All of these must be **opt-in** and must not change the meaning of an unmodified `.pre-commit-config.yaml`. Unknown hook keys make pre-commit warn `Unexpected key(s) present on {src} => {id}: {keys}` (`hook.py:51-56`). Extensions should therefore live in an snp-specific config or under a clearly namespaced key; the config spec decides which.

| Idea | Source | Value | Notes |
|---|---|---|---|
| Per-hook `env:` map, applied **after** the language env patch | prek `env`, lefthook `env` | PATH tweaks; GUI git clients | Must not be able to unset the language's own vars silently: document the precedence. |
| Partial or semver `language_version` (`node: 20`, `go: 1.22`, `>=3.10,<3.13`) | prek | pre-commit **errors** on these today, so this is a pure superset | The env dir name keeps the literal string. Resolve at install time and record the concrete version in snp state. |
| Shared toolchains across hook envs (one node 22.x, one go 1.23.x, one CPython 3.12) | prek | Big download and disk savings | Keep per-hook package dirs. pre-commit keeps a toolchain per env, which is observable only through disk usage. |
| Checksum-verified downloads | prek | Integrity | Adopt **by default** (§14.5). |
| Python via uv, with interpreter downloads | prek | Speed, and no host Python needed | §11 |
| PEP 723 inline metadata for local python hooks without deps | prek | Convenience | Opt-in only. It changes behaviour for scripts that happen to contain such a block. |
| `cargo install --locked`, cargo-binstall for `cli:` deps | prek | Reproducibility, speed | Opt-in. pre-commit resolves fresh. |
| Container runtime choice (docker, podman, Apple `container`), `--init`, podman `--userns=keep-id` | prek | Rootless podman and signal handling | Opt-in env var. The default stays `docker` with exactly pre-commit's args. |
| Native `pygrep` | snp | No Python dependency; ~10–100× faster on many files | §12 |
| `runner:` for scripts (`<runner> <script>`) | lefthook | A clean interpreter choice for `unsupported_script` | Equivalent today: `entry: bash ./script.sh` with `unsupported`. |
| `root:` (cd into a subdir, strip the prefix from paths) | lefthook | Monorepos | The same idea as `pre-commit hazmat cd`, which snp should support natively (§14.3). |
| `use_stdin` / `interactive` | lefthook | Interactive hooks | pre-commit always gives an empty-pipe stdin (or `/dev/null` under the pty). Changing that must be opt-in. |

---

## 14. Implementation notes for snp

### 14.1 Architecture

* **Model languages as a `Copy` enum** with static dispatch (`match`), for example `enum Language { Python, Node, Golang, Rust, Ruby, Docker, DockerImage, Unsupported, UnsupportedScript, Fail, Pygrep, /* part 2 */ … }`:
  * `FromStr` accepts the canonical 4.6.2 names **and** the aliases `system` and `script`.
  * `as_str()` returns the canonical name, which is used in messages.
  * Implement the trait from `04b` on per-language unit structs.
* **Split the lifecycle into pure planning plus effectful execution:**
  * `resolve_version(hook, cfg) -> String`: cached probes.
  * `EnvKey { prefix, language, version, deps }`: the dedup key, identical to `Hook.install_key`.
  * `install(&EnvKey, &Ctx) -> Result<()>`.
  * `env_patch(&EnvKey) -> EnvPatch`: **pure data**.
  * `run_plan(hook, files) -> RunPlan { argv_prefix, partitions, env, cwd }`.
  * The executor runs the plan.
* **Define `EnvPatch`** as `Vec<(OsString, Op)>`, where `Op` is `Set(OsString)`, `Unset`, or `Prepend(Vec<PathBuf>)`.
  * Apply it to an immutable `Arc` snapshot of the base env. The snapshot is taken once at startup, after removing `__PYVENV_LAUNCHER__` and `PYTHONEXECUTABLE` and adding `PRE_COMMIT=1` and the other run vars.
  * The result is a `Vec<(OsString, OsString)>` passed via `Command::env_clear().envs(..)`.
  * **Never call `std::env::set_var`.** It is `unsafe` in edition 2024 and racy with concurrent spawns.
  * Templates resolve against the **base** env, never a partially patched one (§1.6).
  * When `PATH` is unset or empty, do not append a trailing separator.
* **Defaults and probes are lazy.** Compute `get_default_version` only for languages used by selected hooks; cache it in a `OnceLock`, and optionally across runs in the store (keyed by exe metadata). Probes for different languages may run in parallel.

### 14.2 Version strings, env naming, validation

* The env dir is `format!("{ENV_DIR}-{version}")`, joined to the prefix and normalized **lexically** (a Python `normpath` port: collapse `.`, `..` and repeated separators, without touching the FS).
* **Reject** a version whose normalized path leaves the prefix. Example message: `invalid language_version {v:?}: resolves outside the hook environment directory`.
  * This is a deliberate deviation for safety. `python` with an absolute interpreter path still works, because pre-commit nests it.
* Pass versions to tools only as single argv elements. Never use a shell.
* Keep `default`, `system` and the literal strings exactly as given: they are part of the on-disk layout.

### 14.3 Process execution parity

* **Resolution:** implement `find_executable`, `normexe` and `normalize_cmd` exactly (§1.8), using the **child's** `PATH` and `PATHEXT`.
  * Unix `X_OK`: `rustix::fs::access(path, Access::EXEC_OK)`, which is a safe API.
  * Spawn the **resolved path** as both program and argv[0], as Python's `Popen` does.
  * Shebangs: `file_identify::parse_shebang_from_file`. Add parity tests against `identify` (printable check, shlex fallback, `/usr/bin/env -S`, nix-shell).
* **`hook_cmd`:** implement **Python-`shlex.split`-compatible** splitting yourself. The `shlex` crate differs (verified with `shlex` 2.0.1 against CPython):
  * `"a\$b"`: Python gives `a\$b` (inside double quotes, backslash escapes only `"` and `\`); the crate gives `a$b`.
  * `# x`: Python gives `['#','x']`, since `comments=False`; the crate gives `[]` (treated as a comment).

  These differences would silently change hook argv.
  * Unbalanced quotes give `No closing quotation`. Escape at end gives `No escaped character`.
  * Rewrite the `pre-commit hazmat` prefix (§1.7). The snp equivalent runs its own `hazmat` subcommand via `std::env::current_exe()`. Also accept `snp hazmat`.
* **stdin:** use `Stdio::piped()` and drop the `ChildStdin` immediately, giving an **empty pipe**, not `/dev/null`. This matches pre-commit; some tools treat "stdin is a FIFO" specially.
* **Merged stdout and stderr:**
  1. `let (r, w) = std::io::pipe()?; let w2 = w.try_clone()?; cmd.stdout(w).stderr(w2);`
  2. Spawn.
  3. **Drop the `Command`** (it owns the write ends) before reading `r` to EOF, or you deadlock.
  4. Read on a dedicated thread, or async.
* **Colour mode (Unix):** a pty from `rustix::pty` (`openpt`, `grantpt`, `unlockpt`, `ptsname`) plus `rustix::termios`, clearing `OPOST|ONLCR` on the slave.
  * Give the child the slave as stdout and stderr, with stdin `/dev/null`.
  * Close the parent's slave copy, then read the master until `EOF` or `EIO`.
  * Windows uses pipes.
* **Exit codes:** map Unix signal deaths to `-signum`, which is Python's convention and shows as `- exit code: -9`. Aggregate with max-abs, first wins.
* **Spawn errors:** an `OSError` becomes rc 1 with output `"{error}\n"`. A not-found executable becomes rc 1 with output ``"Executable `{x}` not found"``.
* **Ctrl-C** during install: kill children, remove the env dir (§1.3 `clean_path_on_failure`), print `Interrupted (^C)` (via the shared error handler), exit 130.

### 14.4 xargs and shuffling parity

* Port `partition()` exactly (§1.9). Count length in **bytes** on Unix and **UTF-16 code units** on Windows, and include the `+1` separators.
  * Max length: `min(max(ARG_MAX - 2048 - env_size, 4096), 131072)`.
  * `ARG_MAX` comes from `nix::unistd::sysconf(SysconfVar::ARG_MAX)` (safe wrapper), or a per-OS constant. Both Linux (2 MiB) and macOS (1 MiB) clamp to 131072 unless the env is huge.
  * Compute `env_size` from the snp process env once.
* **Shuffle:**
  1. MT19937 with `init_by_array(&[1542676187])`. `rand_mt` 6.x `Mt::new_with_key`, or ~60 lines of own safe code.
  2. Python's `shuffle` and `randbelow` (§1.9).
  3. Assert the test vectors `eafbigcdhj` etc. in unit tests.
* **CPU count:** on Linux use `sched_getaffinity` (`rustix::thread::sched_getaffinity`) to match Python. `std::thread::available_parallelism` also applies cgroup quotas, which would change partition sizes. Elsewhere, use `available_parallelism`.
  * Honour `PRE_COMMIT_NO_CONCURRENCY` (presence) and `TRAVIS`. Optionally add the alias `SNP_NO_CONCURRENCY`.
* Partitions run on a bounded pool (`min(partitions, jobs)`). Results are concatenated **in partition order**.
* pygrep and fail never shuffle. fail never runs a process.

### 14.5 Downloads and archives (go, node, rustup, uv, ruby helpers)

* **HTTP:** a rustls-based client (`ureq` 3.x sync, or `reqwest` with rustls if the engine is async).
  * Honour `HTTP(S)_PROXY`, `NO_PROXY` and `ALL_PROXY` (upper and lower case), `SSL_CERT_FILE`/`SSL_CERT_DIR`, and the platform trust store (`rustls-platform-verifier`).
  * Use connect and read timeouts, plus 3 retries with backoff on 5xx and network errors.
  * Set `User-Agent: snp/<version>`.
* **Verify sha256 before extracting:**
  * Go: `https://go.dev/dl/?mode=json&include=all` → `files[].sha256`.
  * Node: `{base}/v{ver}/SHASUMS256.txt`.
  * rustup-init: `https://static.rust-lang.org/rustup/dist/<triple>/rustup-init[.exe].sha256`.
  * uv: **embedded** pinned hashes.
  * On mismatch, fail closed with a message naming the URL, expected hash and actual hash.
* **Extraction:**
  * Use `tar` + `flate2` (pure-Rust `miniz_oxide` backend) and `zip`.
  * Reject absolute paths, `..` components and drive prefixes. Use `Entry::unpack_in` semantics for tar and `enclosed_name()` for zip.
  * Never write through pre-existing symlinks. Allow symlinks only if their target resolves inside the dest. Strip setuid, setgid and sticky bits.
  * Extract into `<dest>.part-<rand>` on the same filesystem, then `rename`.
  * Strip the archive's top-level dir as each backend needs: go renames `go/` to `.go/`; node copies `node-vX…/` contents into the env root (`Scripts\` on Windows).
* **Stream** downloads through the hasher into a temp file, then verify, then extract. Do not hold whole archives in memory: Go archives are ~70 MB.

### 14.6 Windows specifics

* **Env var keys are case-insensitive.** The custom env merge must dedupe case-insensitively, keeping the first-seen spelling (`Path` vs `PATH`).
* **`PATHEXT`:** dir-major order. Each extension is tried with `exe + ext`, then the bare `exe`.
* **Batch files** (`.cmd`, `.bat`):
  * node global bins and `npm` are `.cmd` shims. Apply the batch length limit (§1.9).
  * Since Rust 1.77.2 (the CVE-2024-24576 "BatBadBut" fix), `std::process::Command` escapes arguments for cmd.exe and **returns `InvalidInput`** for arguments it cannot quote safely.
  * Surface that as a hook failure: rc 1 plus a message naming the offending filename. Never fall back to building a raw `cmd /c` string.
* **Long paths:** node envs nest deeply. Use verbatim `\\?\` paths for filesystem operations where needed, as pre-commit does for the nodeenv argv. Prefer non-verbatim paths (`dunce::simplified`) when passing to child tools that dislike `\\?\`.
* **Python:** `Scripts\python.exe` and the `py` launcher (`py -X.Y -c "import sys; print(sys.executable)"` with `PYTHONIOENCODING=UTF-8`).
* **Unsupported on Windows:** no pty, no docker `-u`, no rbenv. Node `system` is never chosen by default (Q14).

### 14.7 Python backend

Implement §11.4. Key invariants:
* the in_env patch is identical to pre-commit's;
* `.install_state_v1`/`_v2` are written;
* env dirs are named identically;
* the health check accepts virtualenv-, venv- and uv-made envs.

### 14.8 Node backend without nodeenv

* **Versions:**
  * `system`: no download. Create `<env>/bin` and `<env>/lib/node_modules` (`Scripts\node_modules` on Windows). The bash shim is optional, because the env patch already provides the same `NODE_PATH` and prefix values.
  * `default`: `index.json[0]` (latest **Current**, matching nodeenv).
  * `lts`: the first entry with an `lts` field.
  * `X.Y.Z`: exact.
  * Extensions (opt-in or superset): `X`, `X.Y`, ranges.
* **Download base:** `https://nodejs.org/download/release`. Use unofficial-builds for musl and riscv64. Allow a mirror override (`SNP_NODE_MIRROR`); also consider honouring nvm's `NVM_NODEJS_ORG_MIRROR`.
* **Install:**
  1. Compute `pkgs` exactly as §3.4. For a remote repo this means `git+file://<prefix>` when both `.git` and `package.json` exist.
  2. Run `npm install --allow-git=root --install-links -g *pkgs` with cwd=prefix and the env patch applied.
  3. If `pkgs` is empty, fail with the same message.
* **Health:** run `node --version` in the env. Cache the result as in §14.9.
* **Quirk:** nodeenv reads `.node-version` and `[nodeenv]` from the **user's repo** for `default` (Q9).

### 14.9 Health checks without per-run process spawns

* pre-commit spawns python (`-S -c …`) for every python env on every run, and `node --version` for every node env. With 10 envs that is ~100–200 ms per commit.
* **snp design:**
  1. At install, write `<envdir>/.snp_state.json` (versioned schema) containing:
     * the installer;
     * the interpreter path;
     * the `version_info` string in both formats;
     * the base-exe `{path, len, mtime_ns, inode or file_index}`;
     * for node: the resolved node path and its metadata.
  2. **Fast path:** the state file exists, the env `bin/python` exists (launcher or symlink), and the base-exe metadata is unchanged. The env is then healthy, with **no spawn**.
  3. **Slow path:** anything changed, or the state is missing (for example an env created by pre-commit). Run the exact pre-commit check: spawn, then compare.
     * Accept `X.Y.Z` (uv) and `X.Y.Z.level.serial` (virtualenv) as equal when the shared prefix matches and the level and serial are absent or equal.
     * Then refresh the state.
* Only hooks selected for this run and not in `SKIP` are health-checked (`commands/run.py:436-443`).

### 14.10 Filesystem operations

* **`rmtree`:** write a custom `remove_dir_all`.
  * On `EACCES`/`EPERM`, add `u+w` to the entry **and its parent** and retry. This matches `util.py:205-235`.
  * Needed for Go's read-only module cache (`envdir/pkg`) and read-only git objects on Windows.
  * Never follow symlinks; unlink them.
* **Installs happen in place.**
  * Tools embed absolute paths (venv script shebangs, npm bin shims, cargo `.crates.toml`), so build-in-temp-then-rename is **not** possible.
  * Follow pre-commit: remove leftovers, install in the final location, write the state files **last** (`.install_state_v1` via staging and rename, then `_v2`), and remove the dir on any error or interrupt.
  * The state file is the commit marker.
* **Locking:** a per-env lock file (e.g. `<envdir>.lock` next to the env) lets different envs install **in parallel**.
  * If snp shares a store with pre-commit, it must also hold pre-commit's `<store>/.lock` with the same primitive (`fcntl.flock` on POSIX, `msvcrt.locking` on Windows).
  * Otherwise concurrent pre-commit and snp processes could race. See the store spec.

### 14.11 Golang backend

* Resolve versions:
  * `default` → `go.dev/dl/?mode=json` `[0]`;
  * otherwise verbatim (parity).
  * Extension: accept `go1.x.y` and `1.x` by resolving through `include=all`.
* Download, verify, extract to `<env>/.go`.
* Then follow §4.3 exactly:
  * `no_git_env`, `GOPATH=<env>`, unset `GOBIN`, plus `GOTOOLCHAIN=local` and `GOROOT` for non-system;
  * run the `go install` calls sequentially;
  * remove `<env>/pkg` with the permission-fixing rmtree.
* Keep `GOPATH` **unset** in the run env (parity).

### 14.12 Rust backend

* Keep the semantics: `cli:` parsing, `cargo add` for lib deps, `cargo install --bins --root <env>`, `RUSTUP_TOOLCHAIN` only for non-system.
* **Improvements that keep runtime parity:**
  1. Bootstrap rustup from the official **binary** with sha256 instead of the `sh.rustup.rs` script.
  2. Use one install-time `RUSTUP_HOME` for both toolchain install and build. That avoids pre-commit's double download.
     * Option: a persistent `$SNP_HOME/tools/rustup` shared across envs (Q13). Delete it or keep it per policy.
  3. `--profile minimal`, because the toolchain is only needed to compile.
  4. Iterate `cli:` deps in config order, not set order.
* Optional flags, off by default: `--locked` (`SNP_CARGO_LOCKED=1`) and cargo-binstall.
* `cargo install` calls into the same `--root` must stay sequential, because cargo locks `.crates.toml`.

### 14.13 Ruby backend

* `system`: straightforward, as in §6.3.
* Non-system: needs the rbenv, ruby-build and ruby-download tarballs, plus `bash` and `curl` at install time.
  * Embed the same MIT-licensed archives (record version and sha256), or fetch pinned tags with sha256.
  * Low priority; Windows is unsupported, as in pre-commit.
* Sort `star()` results for determinism.

### 14.14 Docker backends

* Use exactly pre-commit's argv (§7). **Fix these bugs:**
  * the rootless probe with check=false;
  * robust JSON parsing (any failure means not rootless, or not in a container);
  * prefix-only path remapping;
  * a cwd containing `:` should give a clear error.
* Cache per process: the rootless result, the container id and the mount map.
* **Performance:** before the per-run `docker build`, check `docker image inspect <tag>` and skip the build when the image exists (Q12; parity escape hatch `SNP_DOCKER_ALWAYS_BUILD=1`).
* The docker tag is `pre-commit-` + md5 of the prefix basename (`md-5` crate). Keep the label `PRE_COMMIT` so `docker image prune --filter label=PRE_COMMIT` style cleanups keep working.

### 14.15 Error and exit-code parity

| Situation | pre-commit | snp |
|---|---|---|
| config-time language errors (§1.3) | `[ERROR] …`, exit 1 | same text, exit 1 |
| install failure (`CalledProcessError`, `AssertionError`, HTTP error, `ValueError`) | `An unexpected error has occurred: <Type>: <msg>`, `Check the log at <store>/pre-commit.log`, exit 3 | same banner and exit code; keep the `command: … / return code: … / stdout: … / stderr: …` block format (§1.9) |
| hook not runnable (not found, not executable) | hook output `Executable …`, hook rc 1 | same, not fatal |
| `shlex` error, `ArgumentTooLongError` | exit 3 | same exit code; better message |
| Ctrl-C | `Interrupted (^C)`, exit 130 | same |

### 14.16 Security checklist

* No shell anywhere. Every hook and installer command is an argv vector.
* Validate env paths (§14.2) and archive paths (§14.5).
* Verify checksums (§14.5). Use TLS everywhere; reject `http://` mirrors unless explicitly allowed.
* Strip `GIT_*` (via `no_git_env`) from **installer** subprocesses: go (parity), pip/uv and npm (deviation that fixes `setuptools_scm` in worktrees).
* Never print proxy credentials or tokens from the env in debug logs.
* `#![forbid(unsafe_code)]` in snp crates. Rely on safe wrappers (`rustix`, `nix`) for pty, termios, access, sysconf and sched_getaffinity.

### 14.17 Performance and memory

* Take the base env snapshot once (`Arc<[(OsString, OsString)]>`). Build each hook's child env once and share it across partitions.
* Capture per partition into a pre-sized `Vec<u8>` and concatenate in order. Avoid UTF-8 conversions of hook output; it is bytes end to end.
* Run independent env installs in parallel (bounded by CPU and network), each under its own lock.
* Spawn-free health checks (§14.9). Lazy toolchain probes.
* pygrep runs in-process (§12): no interpreter start (~30–50 ms per partition in pre-commit), and compiled patterns are reused.

### 14.18 Suggested crates (pin exact versions in `Cargo.lock`; versions are as of Oct 2026)

| Purpose | Crate |
|---|---|
| Regex | `regex` 1.13 (Tier 1), `fancy-regex` 0.19 (Tier 2, `BytesMode::Ascii`), `memchr` |
| Syscalls (safe wrappers) | `rustix` (fs access, pty, termios, `sched_getaffinity`), `nix` (sysconf, if needed) |
| Hashing | `sha2`, `md-5` (docker tag), `hex` |
| Archives | `tar`, `flate2` (`rust_backend`), `zip` |
| HTTP | `ureq` 3 or `reqwest` (rustls) |
| JSON | `serde`, `serde_json` (state, `index.json`, go JSON, docker inspect) |
| Filesystem | `fs-err` (path-carrying I/O errors), `tempfile` |
| RNG | `rand_mt` 6.x (MT19937), or own code |
| User's crates | `file-identify` 0.4 (shebang, tags), `cfgv` 0.2 (config validation) |
| **Do not use** | `shlex` (semantics differ from Python, §14.3); `which` (§1.8 needs exact semantics: relative results for relative PATH entries, empty entries meaning cwd, dir-major `PATHEXT` order, no canonicalization. Owning ~40 lines is simpler than auditing the crate.) |

### 14.19 Testing approach (summary)

* **Fake toolchain shims** first on PATH (`docker`, `go`, `cargo`, `rustup`, `npm`, `gem`, `rbenv`, `uv`). They log argv, cwd and env, which lets tests assert exact command lines. This research used exactly this technique.
* **Golden vectors** for pygrep (§8.4, §12.5) and shuffle (§1.9).
* Real-toolchain integration tests gated behind cargo features or CI matrix jobs (network): python via uv and pip, node system and download, go system and download, rust system and toolchain, ruby system, docker.

---

## 15. Open questions

1. **Q1: Store sharing.** Should snp use pre-commit's store (`~/.cache/pre-commit`) and reuse its envs?
   * Reuse works: virtualenv envs pass snp's health check.
   * The reverse is the problem: pre-commit will **rebuild** uv-created python envs (G5), so the two tools flip-flop.
   * The store spec must decide. Q3 is related.
2. **Q2: Interpreter downloads.** Should snp download a CPython via uv when the requested `pythonX.Y` is not installed? pre-commit fails in this case. Proposed: yes by default, disabled with `SNP_PYTHON_DOWNLOADS=never`. Alternatively make it opt-in for strict parity.
3. **Q3: pyvenv.cfg compat rewrite.** Should uv venvs get virtualenv-style `version_info` and `base-executable` so that pre-commit accepts them?
4. **Q4: uv config isolation.** Use `--project /` (keeps the user's uv.toml) or `--no-config` plus explicit translation?
5. **Q5: uv version.** Which minimum (prek: 0.7.0) and which pinned version for auto-download? Also verify flag availability (`--seed`, `--no-project`, `--project`, `--no-python-downloads`, `--python-preference`) for the chosen floor.
6. **Q6: uv cache.** Share `~/.cache/uv` (dedup with the user's projects, hard links) or isolate it under `$SNP_HOME` (prek's choice: predictable cleanup)?
7. **Q7: pygrep fallback.** For patterns snp cannot translate, fall back to an external `python3 -c <embedded script>` when available, or always fail? What exact error-message format should replace Python's traceback?
8. **Q8: pygrep after an unreadable file.** Should snp continue with the remaining files (proposed) or mimic pre-commit, which stops the rest of the partition?
9. **Q9: nodeenv repo quirk.** Should snp mimic nodeenv reading `.node-version` and `[nodeenv]` in `tox.ini`/`setup.cfg` from the **user's repo** when node's version is `default`? Parity says yes; sanity says no.
10. **Q10: node `default`.** Keep it as the latest Current release (nodeenv parity) or switch to LTS? Parity says latest.
11. **Q11: Local python placeholder.** Skip `pip install .` of the placeholder for local python hooks without deps (proposed: skip)?
12. **Q12: Docker builds.** Skip the per-run `docker build` when the tagged image exists (proposed: skip; parity flag available)?
13. **Q13: Rust toolchain policy.** Pre-commit's "ephemeral toolchain" (download every install) or a persistent shared rustup home? And `--profile minimal`?
14. **Q14: Windows node `system`.** Allow it as an explicit `language_version` on Windows? nodeenv refuses, and pre-commit never picks it by default.
15. **Q15: Message source version.** Mirror 4.6.2 messages everywhere (e.g. language `unsupported` instead of `system` in the §1.3 errors)? Proposed: yes.
16. **Q16: Python reference interpreter.** Which interpreter defines `default` for python: `python3` then `python` on PATH, or `py -3` on Windows? Should the default python version be cached across runs, and how is the cache invalidated?
17. **Q17: The `exe_exists` `$HOME` heuristic.** Replicating it makes snp download node when nvm's node exists. That keeps env names identical but wastes time. Offer an opt-in `SNP_PREFER_SYSTEM_TOOLCHAINS=1`?
18. **Q18: Python 3.14 parity.** Python 3.14 changed argparse's `prog` for `-m` and added `\z` to `re`. Which Python version's pygrep behaviour is canonical? Proposed: CPython 3.12/3.13 output (`pygrep.py`), plus accepting `\z`.
19. **Q19: Golang superset versions.** Should `language_version` like `1.22` or `go1.22.1` be accepted as a superset? The env dir would be named with the literal string, and the resolved version recorded in snp state.
