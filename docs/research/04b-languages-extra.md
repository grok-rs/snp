# 04b: pre-commit language backends, part 2 (less common), `lang_base`, and the Rust `Language` trait

This spec covers `conda`, `coursier`, `dart`, `dotnet`, `haskell`, `julia`, `lua`, `perl`, `r` and `swift`. It also covers the language infrastructure: `lang_base.py`, `prefix.py`, `envcontext.py`, the install and run lifecycle in `repository.py`, and the process helpers the backends depend on. It ends with a Rust trait design that can express all 21 pre-commit languages, including the part-1 languages python, node, golang, rust, ruby, docker, docker_image, unsupported, unsupported_script, fail and pygrep.

---

## 0. Scope, provenance, and how this was verified

* Reference source: `pre-commit` main at `c4292384` (`setup.cfg:3` gives `version = 4.6.2`). Source locations are cited relative to `refs/`, for example `pre-commit/pre_commit/lang_base.py:135`.
* Live verification: the installed `pre-commit 4.1.0` was run in throwaway repos (`scratchpad/tmp-langs2/`), with **fake toolchain shims** placed first on `PATH`. The shims were `conda`, `cs`, `dart`, `dotnet`, `cabal`, `julia`, `luarocks`, `cpan`, `Rscript` and `swift`, all symlinks to one Python logger that records argv, cwd and environment, then creates the expected artifacts. Every command line, cwd and env var marked "(observed)" below comes from those logs.
* Differences between 4.1.0 and 4.6.2 for the in-scope files (from `diff` against the installed package):
  * `lang_base.hook_cmd` gained the `pre-commit hazmat` rewrite in 4.5.0.
  * `julia`: `--startup-file=no` was added to install and run in 4.3.0. `JULIA_DEPOT_PATH` was added in 4.6.1 (`CHANGELOG.md:19`).
  * `store._LOCAL_RESOURCES` dropped `package.json` and `.npmignore` (the node part).
  * `resources/empty_template_pubspec.yaml`: `sdk: '>=2.10.0'` became `'>=2.12.0'` in 4.3.0.
  * Everything else in scope (conda, coursier, dart, dotnet, haskell, lua, perl, r, swift, prefix, envcontext, repository) is byte-identical.
* Registry: `pre-commit/pre_commit/all_languages.py:27-50` holds 21 entries. `language_names = sorted(languages)`, which gives `conda, coursier, dart, docker, docker_image, dotnet, fail, golang, haskell, julia, lua, node, perl, pygrep, python, r, ruby, rust, swift, unsupported, unsupported_script`.

---

## 1. Language infrastructure

### 1.1 The `Language` protocol (`lang_base.py:27-60`)

Every language is a Python **module** with these duck-typed attributes:

| Member | Signature | Meaning |
|---|---|---|
| `ENVIRONMENT_DIR` | `str \| None` | Base name of the env dir inside the hook repo ("prefix"). `None` means there is no environment: no install, and for local hooks the prefix is the CWD. |
| `get_default_version()` | `() -> str` | Value that replaces `'default'` when neither the hook nor `default_language_version` sets one. |
| `health_check(prefix, version)` | `-> str \| None` | `None` means healthy. A string means unhealthy, so the env gets rebuilt. The string is also the "more info" text in the BUG message. |
| `install_environment(prefix, version, additional_dependencies)` | `-> None` | Creates `prefix/<ENVIRONMENT_DIR>-<version>`. Raises on failure. |
| `in_env(prefix, version)` | `-> ContextManager[None]` | Mutates `os.environ` for the duration of the hook run (and is reused inside installs). |
| `run_hook(prefix, entry, args, file_args, *, is_local, require_serial, color)` | `-> tuple[int, bytes]` | Runs the hook. Returns the return code and the merged stdout+stderr. |

### 1.2 Language names, aliases, and config keys

* `clientlib.py:194-198` `_translate_language` maps `'system'` to `'unsupported'` and `'script'` to `'unsupported_script'`. The mapping is applied at load time to manifest hooks (`LanguageMigrationRequired`, `clientlib.py:243`) and to config hooks (`LanguageMigration`, `clientlib.py:456`). After loading, `hook.language` is always canonical.
  * Consequence: on 4.6.2, error messages say ``language `unsupported` ``, while 4.1.0 (observed) says ``language `system` ``.
* `language` is validated with `cfgv.check_one_of(language_names)` **after** translation, and is required in manifests.
* `language_version`: `cfgv.check_string`, default `'default'` (`clientlib.py:260`). `''` is accepted.
* `additional_dependencies`: `check_array(check_string)`, default `[]`.
* Top-level `default_language_version` (`clientlib.py:491-495`): a map whose keys are restricted to `language_names` (`NoAdditionalKeys`). Each value is an optional string with default `'default'`. `system` and `script` are **not** accepted as keys.
* `meta` hooks force `language: unsupported` (`clientlib.py:417-420`).

### 1.3 Lifecycle: version resolution, install, health, run

Sources: `repository.py`, `commands/run.py`, `hook.py`, `store.py`.

1. **Version resolution** (`repository.py:109-121`): merge the manifest dict with the config dict. If `language_version == 'default'`, take `default_language_version[lang]` (which is itself `'default'` when unset). If it is still `'default'`, call `languages[lang].get_default_version()`. All 10 languages in this part return `'default'`, so their env dirs are `<ENV>-default` unless a version is configured.
2. **Env-less languages** (`repository.py:126-142`): if `ENVIRONMENT_DIR is None` and the hook sets a non-default `language_version` or non-empty `additional_dependencies`, pre-commit logs one of the following and calls `exit(1)`. Note the **two spaces** before `Perhaps`.
   * ``[ERROR] The hook `<id>` specifies `language_version` but is using language `<lang>` which does not install an environment.  Perhaps you meant to use a specific language?``
   * ``[ERROR] The hook `<id>` specifies `additional_dependencies` but is using language `<lang>` which does not install an environment.  Perhaps you meant to use a specific language?``

   Both were observed (exit 1).
3. **Prefix selection**:
   * `repo: local` or `meta` with an env-less language: `Prefix(os.getcwd())`. `main.py:189` has already run `os.chdir(git toplevel)`.
   * `repo: local` with an env language: `Prefix(store.make_local(deps))`. This is a store directory populated from templates (§1.8), keyed in `db.db` as `local` or `local:<dep1>,<dep2>` with ref `LOCAL_REPO_VERSION='1'` (`store.py:126-130,212-215`).
   * Remote: `Prefix(store.clone(repo, rev, deps))`. **Each distinct `additional_dependencies` list gets its own clone** (observed: the same repo with 3 different dep lists produced 3 `repo*` dirs). The INFO line is `Initializing environment for <repo>[:<dep1>,<dep2>].`
4. **What gets installed** (`run.py:437-443`): hooks selected by `--hook` id or alias and by stage, **minus SKIP-ped ones**. This happens *before* file filtering, so a hook with no matching files still gets its environment installed. `pre-commit install --install-hooks` installs every hook (`install_uninstall.py:145`).
5. **Dedupe** by `install_key = (prefix, language, language_version, tuple(additional_dependencies))` (`hook.py:39-46`). Hooks with the same key share one env (observed: two lua hooks, one install).
6. **`_hook_installed`** (`repository.py:46-62`):
   ```
   if ENVIRONMENT_DIR is None: True
   venv = prefix.path(f'{ENVIRONMENT_DIR}-{version}')
   ( exists(venv/'.install_state_v2')
     or json.load(venv/'.install_state_v1') == {'additional_dependencies': deps} )
   and not health_check(prefix, version)   # evaluated only if the state check passed
   ```
7. **`install_hook_envs`** (`repository.py:214-229`): compute `_need_installed()`. If it is empty, return without locking. Otherwise take `store.exclusive_lock()` on the `<store>/.lock` file lock. If the lock is blocked, INFO `Locking pre-commit directory` is logged. Then **recompute** `_need_installed()`, which runs the health checks again, and call `_hook_install` **sequentially** in hook order.
8. **`_hook_install`** (`repository.py:65-106`):
   * INFO lines (verbatim; `<src>` is the repo URL or `local`):
     `Installing environment for <src>.` / `Once installed this environment will be reused.` / `This may take a few minutes...`
   * If `venv` exists, `rmtree(venv)` it. This is the readonly-tolerant rmtree (§1.7). See the **path-traversal** finding in §5.1.
   * `with clean_path_on_failure(venv):` (this catches `BaseException`, including Ctrl-C, removes `venv`, and re-raises):
     1. `lang.install_environment(prefix, version, deps)`
     2. `health_error = lang.health_check(prefix, version)`. If truthy, raise `AssertionError(f'BUG: expected environment for {hook.language} to be healthy immediately after install, please open an issue describing your environment\n\nmore info:\n\n{health_error}')`. Observed with a forced R version mismatch.
     3. Write `venv/.install_state_v1staging` with `json.dumps({'additional_dependencies': deps})` (default separators `, ` and `: `, `ensure_ascii=True`; for example `{"additional_dependencies": ["pkgA", "pkgB=1.0"]}`), then `os.replace` it to `.install_state_v1`.
     4. `open(venv/'.install_state_v2', 'a+')`, which creates an empty file.
   * Nothing creates `venv` generically. If a backend never creates it, step 3 raises `FileNotFoundError` (exit 3). Example: coursier with an empty `.pre-commit-channel/` and no deps.
9. **Run** (`run.py:182-200`): the start line `<name>.....` is printed first. Then:
   ```python
   with language.in_env(hook.prefix, hook.language_version):
       retcode, out = language.run_hook(hook.prefix, hook.entry, hook.args, filenames,
           is_local=hook.src == 'local', require_serial=hook.require_serial, color=use_color)
   ```
   * `filenames=()` if `pass_filenames: false`.
   * `in_env` is entered for **every** language, including env-less ones (where it is a no-op).
   * The CWD is the git toplevel.
   * `PRE_COMMIT=1` is set in `os.environ` (`run.py:417`) *before* installs, so installers see it as well (observed).
   * If `in_env` or `run_hook` raises, the error text goes right after the dots with no newline. Observed: `lua hook.......An unexpected error has occurred: CalledProcessError: command: ('luarocks', 'config', '--lua-ver')`.

### 1.4 `Prefix` (`prefix.py:7-18`)

| Method | Semantics |
|---|---|
| `Prefix(prefix_dir)` | NamedTuple (hashable; it is part of `install_key`). |
| `path(*parts)` | `os.path.normpath(os.path.join(prefix_dir, *parts))`. Lexical only: no symlink resolution, `..` collapsed textually. An **absolute part discards the prefix**. On Windows `/` becomes `\`. |
| `exists(*parts)` | `os.path.exists(self.path(*parts))`, which follows symlinks. |
| `star(end)` | `tuple(n for n in os.listdir(prefix_dir) if n.endswith(end))`. Returns **bare names**, not paths. **Unsorted** (filesystem order). Includes directories and dotfiles. A file named exactly `.cabal` matches `'.cabal'`. Raises if the listing fails. |

### 1.5 `envcontext` (`envcontext.py:15-62`)

* `Var(name, default='')`, `UNSET` sentinel. A value is a `str` (set), `UNSET` (pop), or a tuple template of `str | Var`.
* Algorithm:
  1. `before = dict(os.environ)`.
  2. Apply the patches **in order**: `UNSET` becomes `env.pop(k, None)`; a template becomes `''.join(before.get(v.name, v.default) if Var else lit)`; a str is set directly.
  3. **Templates resolve against `before`**, the env at context entry, not against the progressively patched env.
  4. On exit, `env.clear(); env.update(before)`. This is a full restore, so it also discards any other env changes made inside the context.
* PATH idiom: `('PATH', (dir, os.pathsep, Var('PATH')))`. If `PATH` is unset this produces `dir + ':'`, a trailing empty element. On POSIX `execvp` treats that as **CWD** (see §5).
* Contexts nest. Each nested context snapshots the already-patched env (`rust.py` relies on this).
* This is a process-global mutation and not thread-safe. pre-commit gets away with it because hooks run sequentially.

### 1.6 `lang_base` helpers (`lang_base.py`)

| Helper | Lines | Exact semantics |
|---|---|---|
| `FIXED_RANDOM_SEED` | 22 | `1542676187` |
| `SHIMS_RE` | 24 | `re.compile(r'[/\\]shims[/\\]')` |
| `exe_exists(exe)` | 63-83 | `found = parse_shebang.find_executable(exe)` (PATH from `os.environ`). If `None`, return False. `homedir = os.path.expanduser('~')`. `common = os.path.commonpath((found, homedir))`, or `None` on `ValueError` (Windows, different drives). Return `not SHIMS_RE.search(found) and (os.path.dirname(homedir) == homedir or common != homedir)`. So an exe counts as "existing" only if it is **not under a `/shims/` dir** and **not inside `$HOME`**, unless `$HOME` is `/`. Test vectors (`tests/lang_base_test.py:33-62`) with home `/home/me`: `/usr/bin/ruby` gives True; `/foo/shims/ruby` gives False; `/home/me/somedir/ruby` gives False; when `commonpath` raises, True; with home `/`, `/usr/bin/ruby` gives True. Used only by node, golang and ruby `get_default_version` (part 1). |
| `setup_cmd(prefix, cmd, **kw)` | 86-87 | `cmd_output_b(*cmd, cwd=prefix.prefix_dir, **kw)`, with `check=True`, so a nonzero exit raises `CalledProcessError`. stdout and stderr are captured and never shown unless the command fails. |
| `environment_dir(prefix, d, v)` | 90-91 | `prefix.path(f'{d}-{v}')`. Test: `f'{tmp}{os.sep}langenv-default'`. **The version is used verbatim in the path** (§5.1). |
| `assert_version_default(binary, version)` | 94-99 | If `version != 'default'`, raise `AssertionError(f'for now, pre-commit requires system-installed {binary} -- you selected `language_version: {version}`')`. |
| `assert_no_additional_deps(lang, deps)` | 102-111 | If `deps` is non-empty, raise `AssertionError(f'for now, pre-commit does not support additional_dependencies for {lang} -- you selected `additional_dependencies: {deps}`')`. `{deps}` is the **Python list repr**, for example ``you selected `additional_dependencies: ['foo']` `` (observed). |
| `basic_get_default_version()` | 114-115 | `'default'` |
| `basic_health_check(prefix, v)` | 118-119 | `None` |
| `no_install(...)` | 122-127 | `AssertionError('This language is not installable')`. Unreachable in practice, because `repository` never installs env-less languages. |
| `no_env(prefix, v)` | 130-132 | No-op context manager. |
| `target_concurrency()` | 135-143 | `'PRE_COMMIT_NO_CONCURRENCY' in os.environ` (**presence only**; an empty value still counts) gives 1. Otherwise `'TRAVIS' in os.environ` gives 2. Otherwise `xargs.cpu_count()`: `len(os.sched_getaffinity(0))` where available (Linux), else `multiprocessing.cpu_count()`, else 1 (`xargs.py:26-38`). |
| `_shuffled(seq)` | 146-153 | `r = random.Random(); r.seed(1542676187, version=1); l = list(seq); r.shuffle(l)`. For int seeds `version` is irrelevant: `version=2` produces the same output (verified). Test vector: `[str(i) for i in range(10)]` becomes `['4','0','5','1','8','6','2','3','7','9']` (`tests/lang_base_test.py:138-141`). |
| `run_xargs(cmd, file_args, *, require_serial, color)` | 156-171 | `require_serial` gives `jobs=1` and keeps **original order**. Otherwise `file_args = _shuffled(file_args)` and `jobs = target_concurrency()`. Returns `xargs.xargs(cmd, file_args, target_concurrency=jobs, color=color)`. |
| `hook_cmd(entry, args)` | 174-178 | `cmd = shlex.split(entry)` (POSIX, no comments). If `cmd[:2] == ['pre-commit', 'hazmat']`, it becomes `[sys.executable, '-m', 'pre_commit.commands.hazmat', *cmd[2:]]`. Returns `(*cmd, *args)`. Errors: unbalanced quote gives `ValueError: No closing quotation`; a trailing backslash gives `ValueError: No escaped character` (exit 3). An empty entry with no args gives `()`, and `normalize_cmd` then raises `IndexError: tuple index out of range` (observed, exit 3). |
| `basic_run_hook(...)` | 181-196 | `run_xargs(hook_cmd(entry, args), file_args, require_serial=..., color=...)`. Ignores `prefix` and `is_local`. |

**Bit-exact Rust port of `_shuffled`** (verified identical to CPython 3.12 for n = 0, 1, 2, 3, 10, 37 and 1000, including the n=10 test vector). It uses CPython MT19937 `init_by_array([seed])` plus `_randbelow_with_getrandbits`:

```rust
struct Mt19937 { mt: [u32; 624], idx: usize }
impl Mt19937 {
    fn init_genrand(s: u32) -> Self {
        let mut mt = [0u32; 624];
        mt[0] = s;
        for i in 1..624 {
            mt[i] = 1_812_433_253u32.wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_add(i as u32);
        }
        Self { mt, idx: 624 }
    }
    /// CPython seeds ints via init_by_array(32-bit little-endian limbs of |n|); 1542676187 → [1542676187].
    fn init_by_array(key: &[u32]) -> Self {
        let mut g = Self::init_genrand(19_650_218);
        let (n, mut i, mut j) = (624usize, 1usize, 0usize);
        for _ in 0..n.max(key.len()) {
            let p = g.mt[i - 1];
            g.mt[i] = (g.mt[i] ^ (p ^ (p >> 30)).wrapping_mul(1_664_525)).wrapping_add(key[j]).wrapping_add(j as u32);
            i += 1; j += 1;
            if i >= n { g.mt[0] = g.mt[n - 1]; i = 1; }
            if j >= key.len() { j = 0; }
        }
        for _ in 0..n - 1 {
            let p = g.mt[i - 1];
            g.mt[i] = (g.mt[i] ^ (p ^ (p >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i >= n { g.mt[0] = g.mt[n - 1]; i = 1; }
        }
        g.mt[0] = 0x8000_0000;
        g
    }
    fn next_u32(&mut self) -> u32 {
        if self.idx >= 624 {
            for k in 0..624 {
                let y = (self.mt[k] & 0x8000_0000) | (self.mt[(k + 1) % 624] & 0x7fff_ffff);
                self.mt[k] = self.mt[(k + 397) % 624] ^ (y >> 1) ^ if y & 1 == 1 { 0x9908_b0df } else { 0 };
            }
            self.idx = 0;
        }
        let mut y = self.mt[self.idx];
        self.idx += 1;
        y ^= y >> 11; y ^= (y << 7) & 0x9d2c_5680; y ^= (y << 15) & 0xefc6_0000; y ^ (y >> 18)
    }
    fn randbelow(&mut self, n: u32) -> u32 {           // n >= 2 here
        let k = u32::BITS - n.leading_zeros();          // n.bit_length()
        loop { let r = self.next_u32() >> (32 - k); if r < n { return r; } }  // getrandbits(k)
    }
}
pub fn shuffled<T>(mut v: Vec<T>) -> Vec<T> {
    let mut rng = Mt19937::init_by_array(&[1_542_676_187]);
    for i in (1..v.len()).rev() { let j = rng.randbelow((i + 1) as u32) as usize; v.swap(i, j); }
    v
}
```
(For n ≤ 3 the result happens to be the identity permutation.)

### 1.7 Process helpers the backends depend on

**`util.cmd_output_b(*cmd, check=True, **kw)`** (`util.py:90-113`)
* `stdin`, `stdout` and `stderr` default to `PIPE`. `communicate()` closes stdin immediately, so **the child sees EOF on stdin** and installers cannot prompt interactively.
* `cmd = parse_shebang.normalize_cmd(cmd, env=kw.get('env'))`. Lookup uses the `env` kwarg's PATH when one is given, otherwise `os.environ` (which may have been patched by `in_env`).
* `ExecutableNotFoundError` becomes `(1, b'Executable `<orig>` <msg>', None)`, with **no trailing newline** and as if the process had run. A `Popen` `OSError` becomes `(1, str(e).encode().rstrip(b'\n') + b'\n', None)`.
* `check and returncode` raises `CalledProcessError(returncode, cmd, stdout, stderr)`. `cmd` is the **normalized** tuple when normalization succeeded (absolute exe, with the shebang interpreter prepended), otherwise the original tuple.
* `cmd_output` (`util.py:116-120`) decodes stdout and stderr as **strict UTF-8**, so invalid bytes raise `UnicodeDecodeError`.

**`CalledProcessError.__bytes__`** (`util.py:63-75`). Exact format, observed. There are four logical lines and no trailing newline; the error handler's `write_line` adds the final newline:
```
command: <python tuple repr of cmd>
return code: <int>
stdout:<part>
stderr:<part>
```
Here `<part>` is `' (none)'` if empty or `None`, otherwise `'\n    ' + data.replace(b'\n', b'\n    ').rstrip()`. Observed example (conda missing):
```
An unexpected error has occurred: CalledProcessError: command: ('conda', 'env', 'create', '-p', '/…/repok28t1yyr/conda-default', '--file', 'environment.yml')
return code: 1
stdout:
    Executable `conda` not found
stderr: (none)
Check the log at /…/pre-commit.log
```
Observed with a shim that was found but failed: `command: ('/usr/bin/python3', '-I', '/…/shimbin/conda', 'env', 'create', …)`. The shebang interpreter is prepended, and `stdout: (none)` / `stderr: (none)` appear when there is no output.

**`parse_shebang`** (`parse_shebang.py:10-85`)
* `find_executable(exe, env=None)`: first `exe = os.path.normpath(exe)`. If `os.sep in exe`, return `exe` **without checking it exists**. If `'PATHEXT' in env`, the candidates are `exe+ext` for each `ext` in `PATHEXT.split(os.pathsep)`, then bare `exe`. For each element of `env['PATH'].split(os.pathsep)` (an empty element joins to a CWD-relative name), return the first candidate that `isfile` and has `os.access(X_OK)`. Otherwise `None`.
* `normexe(orig, env)`: if there is no `os.sep` or `os.altsep` in `orig`, use `find_executable`, and if that returns `None` raise ``Executable `orig` not found``. Otherwise: a directory raises ``Executable `orig` is a directory``; a non-file raises ``Executable `orig` not found``; a file without X_OK raises ``Executable `orig` is not executable`` (POSIX only).
* `normalize_cmd(cmd, env)`: `exe = normexe(cmd[0])`; `cmd = parse_shebang_from_file(exe) + (exe,) + cmd[1:]`; `exe = normexe(cmd[0])`; return `(exe,) + cmd[1:]`. This runs on **all platforms**. `identify.parse_shebang_from_file` returns `()` if the file is not executable. It reads `#!`, requires a printable-ASCII first line, splits it with shlex (falling back to `.split()`), strips a leading `/usr/bin/env` or `/usr/bin/env -S`, and has a nix-shell special case. Use the `file-identify` 0.4.0 crate's `parse_shebang_from_file` for this.

**`xargs.xargs(cmd, varargs, *, color, target_concurrency, _max_length)`** (`xargs.py:131-184`)
1. `cmd = normalize_cmd(cmd)` against `os.environ`, which `in_env` has patched. `ExecutableNotFoundError` returns `(1, b'Executable `x` not found')` as the hook result. Observed hook output: `- exit code: 1` followed by ``Executable `scalafmt` not found``.
2. Max command length:
   * POSIX: `sysconf(SC_ARG_MAX) - 2048 - environ_size`, clamped to `[2**12, 2**17]`. `environ_size = 8*len(env) + Σ(len(k)+len(v)+2)`. Because this is a default argument, it is evaluated **at import time**.
   * Windows: `2**15 - 2048`.
   * Windows `.bat` / `.cmd` targets: `8192 - len(path_of_cmd.exe) - len(' /c ') - 1024`.
3. `partition()` (`xargs.py:76-117`):
   * `max_args = max(4, ceil(len(varargs)/target_concurrency))`.
   * The length of a command is `len(' '.join(parts))`: bytes in the filesystem encoding on POSIX, UTF-16 code units on Windows. It is counted as `cmd_len + 1` plus `arg_len + 1` per argument.
   * Arguments are packed greedily in order. A single argument that does not fit raises `ArgumentTooLongError(arg)` (exit 3).
   * **Zero varargs still produce one partition (`cmd` alone)**.
4. `threads = min(len(partitions), target_concurrency)`. With 1 thread, a plain `map`; otherwise a `ThreadPoolExecutor`. Results are consumed **in partition order**.
5. Each partition runs `cmd_output_b(check=False, stderr=STDOUT)`, or `cmd_output_p` when `color` is set: a POSIX pty with `OPOST`/`ONLCR` cleared, stdin `/dev/null`, read until EIO.
6. `retcode` is the partition retcode with the largest `abs()`; on ties the **first** wins, and negative signal codes count. Output is the plain concatenation of all partitions.

**Misc helpers**
* `util.win_exe(s)` returns `s + '.exe'` on win32 and `s` elsewhere.
* `util.rmtree` (`util.py:205-235`): `shutil.rmtree` with an error hook. On `EACCES` or `EPERM` from `rmdir`/`remove`/`unlink`, it chmods `+w` on both the path and its parent, then retries. This is for Windows readonly files.
* `util.clean_path_on_failure` (`util.py:27-35`) catches `BaseException`.
* `yaml.yaml_load` is `yaml.load(Loader=CSafeLoader|SafeLoader)` (`yaml.py:8-10`).

### 1.8 Error surfacing (`error_handler.py:19-81`)

| Exception | First line printed | Exit |
|---|---|---|
| `FatalError` | `An error has occurred: FatalError: <msg>` | 1 |
| `KeyboardInterrupt` | `Interrupted (^C): KeyboardInterrupt: ` | 130 |
| anything else (`AssertionError`, `CalledProcessError`, `ValueError`, `IndexError`, `OSError`…) | `An unexpected error has occurred: <TypeName>: <str(exc)>` | 3 |

The first line is followed by `Check the log at <store>/pre-commit.log`, or `Failed to write to log at <path>` when the store is not writable. The log file contains version info, the error, and the traceback.

### 1.9 Local-repo template (`store.py:39-56`, `resources/empty_template_*`)

`store.make_local(deps)` writes these files into a fresh `repo*` dir. Only these matter for part-2 languages:

| File | Content (abridged) | Used by |
|---|---|---|
| `environment.yml` | `channels: [conda-forge, defaults]`, `dependencies: [openssl]` (with a comment that it must not be empty) | conda |
| `Makefile.PL` | `use ExtUtils::MakeMaker; WriteMakefile(NAME => "PreCommitPlaceholder", VERSION => "0.0.1");` | perl (`cpan -T .`) |
| `pre-commit-package-dev-1.rockspec` | `package = "pre-commit-package"`, `version = "dev-1"`, builtin build, no modules | lua (`luarocks make`) |
| `pubspec.yaml` | `name: pre_commit_empty_pubspec`, `environment: {sdk: '>=2.12.0'}`, `executables: {}` | dart |
| `renv.lock` | R `4.0.3`, CRAN `https://cran.rstudio.com`, package `renv` `0.12.5` | r |
| `renv/activate.R` | renv 0.12.5 bootstrap script (12 KB, verbatim resource) | r |
| `renv/LICENSE.renv` | MIT license text | r |

There are no templates for coursier, dotnet, haskell, julia or swift. That makes local `dotnet` and `swift` hooks unusable in practice: `dotnet pack` and `swift build` fail because there is no project file. Local `coursier` and `haskell` hooks **require** `additional_dependencies`. Local `julia` creates an empty `Project.toml`.

---

## 2. Per-language specification (part 2)

Conventions used in this section:
* `<prefix>` is the hook repo dir (a clone, or the store "local" repo).
* `<env>` is `lang_base.environment_dir(prefix, ENVIRONMENT_DIR, version)`, which is always `<prefix>/<ENVIRONMENT_DIR>-default` for these languages unless a version is configured.
* `SEP` is `os.pathsep`: `:` on POSIX, `;` on Windows.
* Unless stated otherwise, commands run through `cmd_output_b` / `setup_cmd`: `check=True`, stdin at EOF, stdout and stderr captured, and failures shown as `CalledProcessError` (exit 3).
* None of these 10 languages overrides `get_default_version`, so all return `'default'`. Only `r` overrides `health_check`.

### 2.1 `conda` (`languages/conda.py`, added in 1.21.0)

| Aspect | Value |
|---|---|
| `ENVIRONMENT_DIR` | `'conda'`, so the env is `<prefix>/conda-default` (`conda.py:18`) |
| default version / health | `'default'` / always healthy |
| `language_version` | Must be `'default'`. Otherwise ``AssertionError: for now, pre-commit requires system-installed conda -- you selected `language_version: X` `` (exit 3). |
| `additional_dependencies` | Supported. Passed verbatim to `<exe> install -p <env> …` (conda match-spec syntax, for example `pkgB=1.0`). |
| Required tools | Install: `conda`, `mamba` or `micromamba` on PATH. Run: nothing beyond the env. |
| `run_hook` | `basic_run_hook` |
| Platforms | linux, macOS, windows (docs) |

**Installer exe** (`conda.py:50-56`): if `os.environ.get('PRE_COMMIT_USE_MICROMAMBA')` is **truthy** (any non-empty string, so `"0"` counts), use `micromamba`. Else if `os.environ.get('PRE_COMMIT_USE_MAMBA')` is truthy, use `mamba`. Else use `conda`. Test cases: `('1','')` gives micromamba, `('','1')` gives mamba, and both unset gives conda (`tests/languages/conda_test.py:13-44`).

**install_environment** (`conda.py:59-77`):
1. `assert_version_default('conda', version)`
2. `<exe> env create -p <env> --file environment.yml`, with cwd `<prefix>`. The relative `environment.yml` resolves against the prefix. Note this is `cmd_output_b(... cwd=prefix)`, not `setup_cmd`, but it is equivalent.
3. If there are deps: `<exe> install -p <env> <dep1> <dep2> …`, cwd `<prefix>`. This is **one invocation** with **no `-y`/`--yes`**; stdin is EOF.
4. `in_env` is **not** active during install. Observed: install commands see the unpatched PATH, and `VIRTUAL_ENV` is still inherited.

**in_env** (`get_env_patch`, `conda.py:24-40`):

| Var | Value |
|---|---|
| `PYTHONHOME` | UNSET |
| `VIRTUAL_ENV` | UNSET |
| `CONDA_PREFIX` | `<env>` |
| `PATH` (POSIX) | `<env>/bin` SEP `$PATH` |
| `PATH` (win32) | `<env>\Library\bin;<env>\Scripts;<env>;<env>\bin;$PATH`, in that exact order |

It does not set `CONDA_DEFAULT_ENV` or `CONDA_SHLVL`, and it does not run any `activate.d` scripts.

**Observed run**: argv `hookc --flag .pre-commit-config.yaml a.txt b.py`, cwd at the git root, `PATH=<env>/bin:<orig>`, `CONDA_PREFIX=<env>`, `VIRTUAL_ENV` removed.

**Local repo**: the template `environment.yml` (openssl from conda-forge/defaults) is always created, so `repo: local` works with or without deps.

**Edge cases**
* A missing `environment.yml` in a remote repo makes conda fail, which surfaces as `CalledProcessError`.
* The chosen installer (`conda`, `mamba`, `micromamba`) is not recorded. Switching later does not trigger a reinstall.
* `micromamba env create -p … --file …` behaviour depends on the micromamba version, and is passed through unchanged.

### 2.2 `coursier` (`languages/coursier.py`, added in 2.8.0; deps and local support since 3.0.0)

| Aspect | Value |
|---|---|
| `ENVIRONMENT_DIR` | `'coursier'`, so the env is `<prefix>/coursier-default` |
| version / health | `'default'` only (assert) / always healthy |
| `additional_dependencies` | Supported. They go to `cs fetch` and `cs install` (for example `scalafmt:3.6.1`, `org:name:ver`). |
| Required tools | Install: `cs` or `coursier`. Run: whatever JVM the apps need. |
| `run_hook` | `basic_run_hook` |
| Platforms | tested on linux |

**install_environment** (`coursier.py:23-62`). The order is significant:
1. `assert_version_default('coursier', version)`
2. `cs = find_executable('cs') or find_executable('coursier')`. This resolves against the **unpatched** `os.environ` PATH and gives an absolute path. If neither is found: `AssertionError('pre-commit requires system-installed "cs" or "coursier" executables in the application search path')` (exit 3).
3. `_install(*opts)` runs `setup_cmd(prefix, (cs, 'fetch', *opts))` and then `setup_cmd(prefix, (cs, 'install', '--dir', <env>, *opts))`, both with cwd `<prefix>`.
4. Everything below runs **inside `in_env`**, so `PATH=<env>` SEP `$PATH` and `COURSIER_CACHE=<env>/.cs-cache`:
   * `channel = prefix.path('.pre-commit-channel')`. If it is a dir, then for each `name in os.listdir(channel)` (unsorted, directories included), compute `app = os.path.splitext(os.path.basename(name))[0]` and call `_install('--default-channels=false', '--channel', channel, app)`. That is **one fetch+install pair per descriptor**.
   * Otherwise, if there are no deps: `FatalError('expected .pre-commit-channel dir or additional_dependencies')` (exit 1, observed).
   * If there are deps: `_install(*deps)`, a **single** fetch and a single install carrying all deps. This runs after any channel apps.

**Observed** (local, deps `['scalafmt:3.6.1','other']`):
```
cs fetch scalafmt:3.6.1 other                                  cwd=<prefix>  PATH=<env>:…  COURSIER_CACHE=<env>/.cs-cache
cs install --dir <env> scalafmt:3.6.1 other                    cwd=<prefix>
```

**in_env** (`coursier.py:65-69`):

| Var | Value |
|---|---|
| `PATH` | `<env>` SEP `$PATH`. This is the **env dir itself**, not `<env>/bin`. |
| `COURSIER_CACHE` | `<env>/.cs-cache`. Jars stay inside the env, so hooks run offline (`CHANGELOG.md:386`). |

**Edge cases**
* Descriptor naming: `echo-java.json` gives `echo-java`; `a.b.json` gives `a.b`; `.hidden` gives `.hidden` (splitext leaves dotfiles alone).
* An existing but **empty** `.pre-commit-channel/` with no deps means nothing is installed and `<env>` is never created. Writing the state file then raises `FileNotFoundError` (exit 3).
* On Windows, `PATHEXT` lets `cs.exe` or `cs.bat` resolve.

### 2.3 `dart` (`languages/dart.py`, added in 2.15.0)

| Aspect | Value |
|---|---|
| `ENVIRONMENT_DIR` | `'dartenv'` |
| version / health | `'default'` only / always healthy |
| `additional_dependencies` | Supported: `name` or `name:version`, split at the **first** `:`. The version can be a constraint such as `^1.2.0`. |
| Required tools | Install: the `dart` SDK. Run: nothing, because executables are AOT-compiled. |
| `run_hook` | `basic_run_hook` |
| Platforms | linux, macOS, windows |

**install_environment** (`dart.py:38-97`):
1. `assert_version_default('dart', version)`
2. `bin_dir = <env>/bin`, then `os.makedirs(bin_dir)`. **No `exist_ok`**, so it fails if the dir already exists.
3. Helper `_install_dir(p: Prefix, pub_cache)`:
   1. `dart_env = {**os.environ, 'PUB_CACHE': pub_cache}`
   2. `pubspec = yaml_load(open(p/'pubspec.yaml'))` (safe loader)
   3. `setup_cmd(p, ('dart','pub','get'), env=dart_env)`, with cwd `p`. This writes `.dart_tool/` and `pubspec.lock` **into `p`**, which is the hook clone itself.
   4. `for executable in pubspec['executables']:` iterates the mapping **keys**. A key's value (pubspec's alias target) is **ignored**. Then: `setup_cmd(p, ('dart','compile','exe','--output', <env>/bin/<win_exe(executable)>, p.path('bin', f'{executable}.dart')), env=dart_env)`
4. `with tempfile.TemporaryDirectory() as tmp: _install_dir(prefix, tmp)`. The temp dir is in the system temp area (`$TMPDIR`), is not inside the store, and is deleted afterwards.
5. For each `dep_s` in deps, using a fresh temp `dep_tmp`:
   1. `dep, _, version = dep_s.partition(':')`
   2. `setup_cmd(prefix, ('dart','pub','cache','add', dep, *(['--version', version] if version else [])), env={**os.environ, 'PUB_CACHE': dep_tmp})`
   3. `os.walk(dep_tmp)` (top-down pre-order) finds the **first directory that directly contains `pubspec.yaml`**. That directory is copied with `copytree` to `<fresh tmp>/pkg`, and `_install_dir(Prefix(pkg), dep_tmp)` runs there.
   4. If none is found: `AssertionError(f'could not find pubspec.yaml for {dep_s}')`

**Observed** (local repo plus deps `['pkg_x:1.2.3','pkg_y']`; the fake package pubspec declared `executables: {pkg_xx: , aliased: other}`):
```
dart pub get                                         cwd=<prefix>     PUB_CACHE=/tmp/tmpA
dart pub cache add pkg_x --version 1.2.3             cwd=<prefix>     PUB_CACHE=/tmp/tmpB
dart pub get                                         cwd=/tmp/tmpC/pkg PUB_CACHE=/tmp/tmpB
dart compile exe --output <env>/bin/pkg_xx /tmp/tmpC/pkg/bin/pkg_xx.dart
dart compile exe --output <env>/bin/aliased /tmp/tmpC/pkg/bin/aliased.dart   # alias "other" ignored
dart pub cache add pkg_y                             cwd=<prefix>     PUB_CACHE=/tmp/tmpD
…
```

**in_env**: `PATH=<env>/bin` SEP `$PATH`. No `PUB_CACHE` at run time.

**Edge cases**
* A missing `executables` key gives `KeyError: 'executables'` (exit 3).
* `executables:` set to null gives `TypeError: 'NoneType' object is not iterable`.
* `{}` compiles nothing; this is the local template's case.
* If two packages define the same executable name, the later one silently overwrites the earlier.
* On Windows the outputs are `<name>.exe`.
* The template SDK constraint is `>=2.12.0`, raised in 4.3.0 to support newer Dart.

### 2.4 `dotnet` (`languages/dotnet.py`, added in 2.8.0)

| Aspect | Value |
|---|---|
| `ENVIRONMENT_DIR` | `'dotnetenv'`. `BIN_DIR = 'bin'`. |
| version / health | `'default'` only / always healthy |
| `additional_dependencies` | **Not supported.** `assert_no_additional_deps('dotnet', deps)` runs right after the version assert. Observed message: ``AssertionError: for now, pre-commit does not support additional_dependencies for dotnet -- you selected `additional_dependencies: ['foo']` `` (exit 3). |
| Required tools | The `dotnet` SDK for install. At run time, framework-dependent tools need the dotnet runtime. |
| `run_hook` | `basic_run_hook` |
| Platforms | linux, windows |

**install_environment** (`dotnet.py:55-111`):
1. Version assert, then deps assert.
2. `build_dir = prefix.path('pre-commit-build')`
3. `setup_cmd(prefix, ('dotnet','pack','--configuration','Release','--property', f'PackageOutputPath={build_dir}'))`
4. `nupkgs = [x for x in os.listdir(build_dir) if x.endswith('.nupkg')]` (unsorted). If the list is empty: `AssertionError('could not find any build outputs to install')`.
5. For each nupkg:
   1. Open it as a zip. **Exactly one** member must end with `.nuspec`. With zero or several, the tuple-unpack raises `ValueError: not enough values to unpack (expected 1, got 0)` or `ValueError: too many values to unpack (expected 1)`.
   2. Parse the XML with ElementTree. `re.match(r'{.*}', root.tag)` extracts the namespace. With no namespace: `AssertionError('could not parse namespace from nuspec')`.
   3. `tree.find(f'.//{ns}id')` takes the first `id` element in that namespace, in document order. If missing: `AssertionError('expected to find an "id" element')`. If its text is empty: `AssertionError('"id" element missing tool name')`.
   4. Write a temp `nuget.config` with this exact content (one line, no newlines): `<?xml version="1.0" encoding="utf-8"?><configuration>  <packageSources>    <clear />  </packageSources></configuration>`
   5. `setup_cmd(prefix, ('dotnet','tool','install','--configfile', <tmp>/nuget.config, '--tool-path', <env>/bin, '--add-source', build_dir, tool_id))`

**Observed**:
```
dotnet pack --configuration Release --property PackageOutputPath=<prefix>/pre-commit-build      cwd=<prefix>
dotnet tool install --configfile /tmp/tmpX/nuget.config --tool-path <env>/bin --add-source <prefix>/pre-commit-build my.tool
```

**in_env**: `PATH=<env>/bin` SEP `$PATH`.

**Edge cases**
* `<prefix>/pre-commit-build/` is **never cleaned up**. A reinstall into the same clone sees stale `.nupkg` files too.
* Several nupkgs (for example a multi-project `.sln`) are each installed (2.19.0).
* Dotted tool ids such as `testeroni.tool` work.
* `repo: local` cannot work, because the template has no project file.
* `nuget.config` clears every package source, so `dotnet tool install` resolves only from `build_dir`.

### 2.5 `haskell` (`languages/haskell.py`, added in 3.4.0)

| Aspect | Value |
|---|---|
| `ENVIRONMENT_DIR` | `'hs_env'` |
| version / health | `'default'` only / always healthy |
| `additional_dependencies` | Supported. Appended as `cabal install` targets (for example `hello`, `pkg-1.0`). |
| Required tools | `cabal` and GHC for install. Run time needs nothing, because binaries are copied. |
| `run_hook` | `basic_run_hook` |
| Platforms | linux, macOS, windows |

**install_environment** (`haskell.py:33-56`):
1. `assert_version_default('haskell', version)`
2. `pkgs = [*prefix.star('.cabal'), *deps]`. These are bare `.cabal` file names in listdir order, then the deps. If the list is empty: `FatalError('Expected .cabal files or additional_dependencies')` (exit 1, observed).
3. `os.makedirs(<env>/bin, exist_ok=True)`
4. `setup_cmd(prefix, ('cabal','update'))`. This needs the network and updates the **user's global** cabal package index, not anything in the store.
5. `setup_cmd(prefix, ('cabal','install','--install-method','copy','--installdir', <env>/bin, *pkgs))`

Observed exactly as above, with deps `hello other-1.0`. The test asserts the installed exe is a regular file, **not a symlink** (`haskell_test.py:35-38`). Builds still go through the user's global cabal store; only the final executables are isolated. `dist-newstyle/` appears in `<prefix>`.

**in_env**: `PATH=<env>/bin` SEP `$PATH`.

### 2.6 `julia` (`languages/julia.py`, added in 4.1.0)

| Aspect | Value |
|---|---|
| `ENVIRONMENT_DIR` | `'juliaenv'` |
| version / health | Default `'default'`. **No version assertion**: any `language_version` is accepted, **ignored**, and only used in the env dir name (a TODO mentions juliaup). Always healthy. |
| `additional_dependencies` | Supported. They go into Pkg REPL `add` syntax (`Example@0.5`, `TOML=fa267f1f-…`, URLs). |
| Required tools | `julia` on PATH, for both install and run. |
| `run_hook` | Custom: an interpreter plus a script. |

**in_env** (`julia.py:49-61`):

| Var | Value |
|---|---|
| `JULIA_LOAD_PATH` | `<env>`. This **replaces** the load path, with no `@`, `@v#.#` or `@stdlib`. |
| `JULIA_PROJECT` | UNSET |
| `JULIA_DEPOT_PATH` | `<env>/depot` + SEP. Added in 4.6.1. The trailing separator makes Julia append its default depots, which keeps bundled stdlib precompile caches available. |

How this works: with `JULIA_PROJECT` unset and no `--project`, `Base.active_project()` falls back to the first `LOAD_PATH` entry that has a project file. That is `<env>/Project.toml`, which is why install always creates one. Test `julia_test.py:92-109` relies on this: it reads `Base.active_project()` at run time and finds both the repo deps and the added deps.

**install_environment** (`julia.py:71-140`). All steps run **inside `in_env`**:
1. `os.makedirs(<env>, exist_ok=True)`
2. Copy the first existing file of `('JuliaProject.toml', 'Project.toml')` from `<prefix>` into `<env>`, keeping its basename (`shutil.copy`, which also copies mode bits). If neither exists, create an empty `<env>/Project.toml`.
3. Copy the first existing file of `('JuliaManifest.toml', 'Manifest.toml')` the same way.
4. `cmd_output_b('julia', '--startup-file=no', '-e', JULIA_CODE, '--', <env>, *deps, cwd=<prefix>)`. The `--startup-file=no` flag dates from 4.3.0. JULIA_CODE is verbatim below, with its leading 8-space indentation kept and **not dedented**:
```julia
        @assert length(ARGS) > 0
        hook_env = ARGS[1]
        deps = join(ARGS[2:end], " ")

        # We prepend @stdlib here so that we can load the package manager even
        # though `get_env_patch` limits `JULIA_LOAD_PATH` to just the hook env.
        pushfirst!(LOAD_PATH, "@stdlib")
        using Pkg
        popfirst!(LOAD_PATH)

        # Instantiate the environment shipped with the hook repo. If we have
        # additional dependencies we disable precompilation in this step to
        # avoid double work.
        precompile = isempty(deps) ? "1" : "0"
        withenv("JULIA_PKG_PRECOMPILE_AUTO" => precompile) do
            Pkg.instantiate()
        end

        # Add additional dependencies (with precompilation)
        if !isempty(deps)
            withenv("JULIA_PKG_PRECOMPILE_AUTO" => "1") do
                Pkg.REPLMode.pkgstr("add " * deps)
            end
        end
        
```
The block above ends with a line of exactly 8 spaces, because the Python literal closes with `"""` indented by 8. Copy the text from `julia.py:110-135` byte-for-byte.

Deps are joined with single spaces, so a dep that contains spaces turns into several Pkg arguments. `hook_env = ARGS[1]` is never used by the code itself; the project is selected through `JULIA_LOAD_PATH`. Packages and precompile caches end up under `<env>/depot` (test `julia_test.py:47-57`).

**run_hook** (`julia.py:21-46`):
```
cmd    = hook_cmd(entry, args)                    # shlex + hazmat rewrite + args
script = cmd[0] if is_local else prefix.path(cmd[0])
cmd    = ('julia', '--startup-file=no', script, *cmd[1:])
return run_xargs(cmd, file_args, require_serial=…, color=…)
```
* Observed (4.1.0, which lacks `--startup-file=no`): `julia <prefix>/src/main.jl --opt .pre-commit-config.yaml a.txt b.py`, cwd at the git root, `JULIA_LOAD_PATH=<env>`, `JULIA_PROJECT` removed.
* Local hooks: the script path stays relative to the CWD (the git root). An absolute entry path is used as-is, because `os.path.join` discards the prefix.
* The interpreter is the literal `julia`, found through PATH.
* `--startup-file=no` (4.3.0) prevents a user's `~/.julia/config/startup.jl` from running (test `julia_test.py:36-44`).

### 2.7 `lua` (`languages/lua.py`, added in 2.17.0)

| Aspect | Value |
|---|---|
| `ENVIRONMENT_DIR` | `'lua_env'` |
| version / health | `'default'` only / always healthy |
| `additional_dependencies` | Supported. **One `luarocks install` per dep**, because luarocks cannot install several at once. |
| Required tools | `luarocks` **for install and for every hook run**, plus the Lua interpreter luarocks is configured for. |
| `run_hook` | `basic_run_hook` |
| Platforms | linux, macOS. Windows "may work"; every function is marked `# pragma: win32 no cover` and the tests skip win32. |

**`_get_lua_version()`** (`lua.py:22-25`): `cmd_output('luarocks', 'config', '--lua-ver')` with `check=True`, cwd at the process CWD, and the current env. Returns `stdout.strip()`, for example `5.4`.

**in_env** (`get_env_patch(d)`, `lua.py:28-43`). Every call spawns `luarocks config --lua-ver`:

| Var | Exact value (V is the lua version, `/` is `os.sep`) |
|---|---|
| `PATH` | `<d>/bin` SEP `$PATH` |
| `LUA_PATH` | `<d>/share/lua/<V>/?.lua;<d>/share/lua/<V>/?/init.lua;;`. This **replaces** the user's value; the `;;` at the end means "append Lua's default path". |
| `LUA_CPATH` | `<d>/lib/lua/<V>/?.so;;` (`?.dll;;` on win32). Also a replacement. |

**install_environment** (`lua.py:53-75`):
1. `assert_version_default('lua', version)`
2. `with in_env(...)`: this spawns `luarocks config --lua-ver` once, without the patch (observed).
3. `os.makedirs(<env>, exist_ok=True)`
4. For each `rockspec in prefix.star('.rockspec')` (listdir order): `setup_cmd(prefix, ('luarocks','--tree', <env>, 'make', rockspec))`
5. For each dep: `setup_cmd(prefix, ('luarocks','--tree', <env>, 'install', dep))`

**Observed** (local, deps `[penlight, luacheck]`, two hooks sharing the env):
```
luarocks config --lua-ver                                        cwd=<git root>
luarocks --tree <env> make pre-commit-package-dev-1.rockspec     cwd=<prefix>  (LUA_PATH/LUA_CPATH/PATH patched)
luarocks --tree <env> install penlight
luarocks --tree <env> install luacheck
luarocks config --lua-ver        ← hook 1 in_env
luacheck .pre-commit-config.yaml a.txt b.py
luarocks config --lua-ver        ← hook 2 in_env
luacheck …
```
If `luarocks` is missing at **run** time, the result is `lua hook….An unexpected error has occurred: CalledProcessError: command: ('luarocks', 'config', '--lua-ver')` / `return code: 1` / ``stdout:\n    Executable `luarocks` not found`` / `stderr: (none)` (exit 3, observed).

### 2.8 `perl` (`languages/perl.py`, added in 2.1.0)

| Aspect | Value |
|---|---|
| `ENVIRONMENT_DIR` | `'perl_env'` |
| version / health | `'default'` only / always healthy |
| `additional_dependencies` | Supported. Appended to the single `cpan` call, in any cpan argument format (`Module::Name`, `AUTHOR/Dist-1.0.tar.gz`, URLs). |
| Required tools | `perl` and `cpan` for install; `perl` at run time. |
| `run_hook` | `basic_run_hook` |
| Platforms | linux, macOS, windows |

**in_env** (`perl.py:21-32`):

| Var | Value |
|---|---|
| `PATH` | `<env>/bin` SEP `$PATH` |
| `PERL5LIB` | `<env>/lib/perl5`. This **replaces** the user's `PERL5LIB`. |
| `PERL_MB_OPT` | `--install_base <q(env)>` |
| `PERL_MM_OPT` | `INSTALL_BASE=<q(env)> INSTALLSITEMAN1DIR=none INSTALLSITEMAN3DIR=none` |

`q` is Python `shlex.quote`:
* `''` becomes `''`.
* If every char matches `[\w@%+=:,./-]` (ASCII `\w`), the string is returned unchanged.
* Otherwise the result is `"'" + s.replace("'", "'\"'\"'") + "'"`.

So Windows paths, which contain `\`, get single-quoted.

**install_environment** (`perl.py:42-50`): `assert_version_default('perl', version)`, then inside `in_env`: `setup_cmd(prefix, ('cpan', '-T', '.', *deps))`. `-T` skips tests. `.` is the distribution in the cwd (`<prefix>`), so the repo needs a `Makefile.PL` or `Build.PL`; the local template supplies a `Makefile.PL`. The install location comes **only** from `PERL_MM_OPT` / `PERL_MB_OPT`.

**Observed**: `cpan -T . SHANCOCK/Perl-Tidy-20211029.tar.gz Foo::Bar`, cwd `<prefix>`, with `PERL5LIB=<env>/lib/perl5`, `PERL_MB_OPT=--install_base <env>`, and `PERL_MM_OPT=INSTALL_BASE=<env> INSTALLSITEMAN1DIR=none INSTALLSITEMAN3DIR=none`.

**Edge cases**
* The first run of `cpan` may trigger CPAN autoconfiguration. Stdin is EOF, so there are no interactive answers.
* `<env>` is created by the installers. If nothing gets installed, the state write fails (§1.3 step 8).

### 2.9 `r` (`languages/r.py`, added in 2.11.0; health check since 3.8.0)

| Aspect | Value |
|---|---|
| `ENVIRONMENT_DIR` | `'renv'` |
| version | `'default'` only (assert) |
| health | **Custom**: compares R versions using 2 `Rscript` spawns |
| `additional_dependencies` | Supported, in any `renv::install()` format (for example `rprojroot@1.0`). |
| Required tools | R, plus network access to CRAN and for renv bootstrap. Install and health use `$R_HOME/bin/Rscript[.exe]`, or `Rscript` from PATH when `R_HOME` is unset. **Run always uses `Rscript` from PATH.** |
| `run_hook` | Custom: validates the entry and adds fixed options. |
| Platforms | linux, macOS, windows |

Constants and helpers:
* `_RENV_ACTIVATED_OPTS = ('--no-save', '--no-restore', '--no-site-file', '--no-environ')` (`r.py:23-25`).
* `_rscript_exec()` (`r.py:149-154`): if `R_HOME` is in the env, `os.path.join(R_HOME, 'bin', win_exe('Rscript'))`; otherwise `'Rscript'`.
* `_execute_r(code, *, prefix, version, args=(), cwd, cli_opts)` (`r.py:28-37`): inside `in_env` and inside a temp dir, write `<tmp>/script.R` with `_inline_r_setup(textwrap.dedent(code))`. Then run `cmd_output(_rscript_exec(), *cli_opts, <file>, *args, cwd=cwd)` with `check=True` and strict UTF-8, and return `stdout.rstrip('\n')`. The reason for a script file instead of `-e` is to avoid argument (un)quoting bugs on Windows (`CHANGELOG.md:275`).
  * `_execute_r_in_renv` passes `cli_opts = _RENV_ACTIVATED_OPTS`. Because `--no-init-file` is **not** passed, R sources `R_PROFILE_USER`, which loads renv.
  * `_execute_vanilla_r` passes `cli_opts = ('--vanilla',)`.
* `_inline_r_setup(code)` (`r.py:245-259`) prepends this text, then a `'\n'` join, then the code:
  ```r
  options(
      install.packages.compile.from.source = "never",
      pkgType = "binary"
  )

  ```

**in_env** (`r.py:123-127`): `R_PROFILE_USER=<env>/activate.R` and `RENV_PROJECT` UNSET. The latter fixes hooks for users with renv projects (`CHANGELOG.md:581`).

**install_environment** (`r.py:190-242`):
1. `assert_version_default('r', version)`
2. `os.makedirs(<env>, exist_ok=True)`
3. `shutil.copy(<prefix>/renv.lock, <env>)`. If the file is missing: `FileNotFoundError` (exit 3).
4. `shutil.copytree(<prefix>/renv, <env>/renv)`
5. **Script A** runs through `_execute_vanilla_r` with cwd `<env>`. The argv is `Rscript --vanilla <tmp>/script.R`, and the env still carries the `in_env` patch. Body (after dedent, observed):
   ```r
   prefix_dir <- '<prefix as Python repr>'
   options(
       repos = c(CRAN = "https://cran.rstudio.com"),
       renv.consent = TRUE
   )
   source("renv/activate.R")
   renv::restore()
   activate_statement <- paste0(
     'suppressWarnings({',
     'old <- setwd("', getwd(), '"); ',
     'source("renv/activate.R"); ',
     'setwd(old); ',
     'renv::load("', getwd(), '");})'
   )
   writeLines(activate_statement, 'activate.R')
   is_package <- tryCatch(
     {
         path_desc <- file.path(prefix_dir, 'DESCRIPTION')
         suppressWarnings(desc <- read.dcf(path_desc))
         "Package" %in% colnames(desc)
     },
     error = function(...) FALSE
   )
   if (is_package) {
       renv::install(prefix_dir)
   }
   ```
   * `prefix_dir` is embedded with Python `repr()`. That gives single quotes, unless the path contains `'` and no `"`, in which case double quotes are used. Backslashes are doubled and non-printables escaped. snp must emit a correctly escaped R string literal.
   * The `activate.R` that gets written is `suppressWarnings({old <- setwd("<env>"); source("renv/activate.R"); setwd(old); renv::load("<env>");})`, where `<env>` is R's `getwd()` (forward slashes on Windows).
   * "Is a package" means `DESCRIPTION` is readable and has a `Package` field. The docs say `Type: Package`, but the code checks the `Package` field.
6. `_write_current_r_version`: renv-activated `renv::settings$r.version(as.character(getRversion()))`, cwd `<env>`.
7. If there are deps: renv-activated `renv::install(commandArgs(trailingOnly = TRUE))`, with the deps as trailing argv after the script path.

**Observed install argv** (the deps step also shown):
```
Rscript --vanilla /tmp/tmpA/script.R                                                         cwd=<env>  R_PROFILE_USER=<env>/activate.R  RENV_PROJECT unset
Rscript --no-save --no-restore --no-site-file --no-environ /tmp/tmpB/script.R                 cwd=<env>
Rscript --no-save --no-restore --no-site-file --no-environ /tmp/tmpC/script.R rprojroot@1.0 glue
```

**health_check** (`r.py:86-107`):
```
installed = execute_r_in_renv('cat(renv::settings$r.version())', cwd=<env>)
current   = execute_r_in_renv('cat(as.character(getRversion()))',  cwd=<env>)
if installed in {'NULL', ''}:
    -> f'Hooks were installed with an unknown R version. R version for hook repo now set to {current}'
elif installed != current:
    -> f'Hooks were installed for R version {installed}, but current R executable has version {current}'
else: None
```
* The "now set to" wording is misleading, because nothing gets written; the env is simply rebuilt.
* Cost (observed): **2 Rscript processes per R env per `pre-commit run`**. Add 2 more under the lock when any install is pending, and 2 more right after the install.
* If Rscript fails inside the health check, `CalledProcessError` **propagates** (exit 3) rather than being treated as "unhealthy".
* Observed BUG path, with the current version forced to 4.4.0:
  ```
  An unexpected error has occurred: AssertionError: BUG: expected environment for r to be healthy immediately after install, please open an issue describing your environment

  more info:

  Hooks were installed for R version 4.3.3, but current R executable has version 4.4.0
  ```

**run_hook** (`r.py:137-187, 262-278`):
1. `cmd = shlex.split(entry)`. This is **not** `hook_cmd`, so there is no hazmat rewrite.
2. `_entry_validate(cmd)` raises `ValueError` (exit 3) **at run time**, after the env install:
   * `cmd[0] != 'Rscript'` gives ``entry must start with `Rscript`.``
   * `cmd[1] == '-e'`, where a single-token entry gives `IndexError: list index out of range` (observed). With `len > 3`: `You can supply at most one expression.`
   * Otherwise, if `len > 2`: ``The only valid syntax is `Rscript -e {expr}`or `Rscript path/to/hook/script` ``. There is **no space before `or`**, verbatim from `r.py:171-172`; observed.
3. `part = cmd[1:]` if `cmd[1] == '-e'` or `is_local`, else `(prefix.path(cmd[1]),)`.
4. `run_xargs(('Rscript', *_RENV_ACTIVATED_OPTS, *part, *args), file_args, …)`. `cmd[0]` is the literal `Rscript` resolved through PATH, so **`R_HOME` is ignored at run time**.

* Observed: `Rscript --no-save --no-restore --no-site-file --no-environ <prefix>/hooks/check.R --foo <files…>` and `Rscript … -e cat("x") <files…>`, both with cwd at the git root and `R_PROFILE_USER` set.
* Test vectors are in `tests/languages/r_test.py:19-130`.

### 2.10 `swift` (`languages/swift.py`)

| Aspect | Value |
|---|---|
| `ENVIRONMENT_DIR` | `'swift_env'`. `BUILD_DIR='.build'`, `BUILD_CONFIG='release'`. |
| version / health | `'default'` only / always healthy |
| `additional_dependencies` | **Not supported** (assert) |
| Required tools | The `swift` toolchain for install. At run time, the Swift runtime libraries on Linux. |
| `run_hook` | `basic_run_hook` |
| Platforms | linux, macOS (`# pragma: win32 no cover`) |

**install_environment** (`swift.py:36-50`):
1. Version assert, then deps assert.
2. `os.mkdir(<env>)`, which is non-recursive and fails if the dir exists.
3. `cmd_output_b('swift','build','--package-path', <prefix>, '-c','release','--build-path', <env>/.build)`. **No `cwd`**, so it inherits the process CWD, which is the user's git root (observed).

**in_env**: `PATH=<env>/.build/release` SEP `$PATH`.

* Observed: `swift build --package-path <prefix> -c release --build-path <env>/.build`, cwd `<git root>`.
* Observed version error: ``AssertionError: for now, pre-commit requires system-installed swift -- you selected `language_version: 5.9` `` (exit 3).
* All intermediate build products live in `<env>/.build`, which can be large.
* `repo: local` cannot work, because there is no `Package.swift` template.

---

## 3. Cross-language matrices

### 3.1 Part 2 (this document)

| lang | env dir | lang_version | add'l deps | health | install cwd | `in_env` vars | run |
|---|---|---|---|---|---|---|---|
| conda | `conda-V` | default only | ✔ (1 call) | – | prefix | PYTHONHOME⊘, VIRTUAL_ENV⊘, CONDA_PREFIX, PATH(+win extras) | basic |
| coursier | `coursier-V` | default only | ✔ (1 fetch+1 install) | – | prefix | PATH(`<env>`), COURSIER_CACHE | basic |
| dart | `dartenv-V` | default only | ✔ (per dep) | – | prefix / tmp pkg | PATH(`<env>/bin`) | basic |
| dotnet | `dotnetenv-V` | default only | ✘ | – | prefix | PATH(`<env>/bin`) | basic |
| haskell | `hs_env-V` | default only | ✔ (1 call) | – | prefix | PATH(`<env>/bin`) | basic |
| julia | `juliaenv-V` | **ignored** (any) | ✔ (1 call) | – | prefix | JULIA_LOAD_PATH, JULIA_PROJECT⊘, JULIA_DEPOT_PATH | `julia --startup-file=no <script>` |
| lua | `lua_env-V` | default only | ✔ (per dep) | – | prefix | PATH, LUA_PATH, LUA_CPATH (spawns luarocks) | basic |
| perl | `perl_env-V` | default only | ✔ (1 call) | – | prefix | PATH, PERL5LIB, PERL_MB_OPT, PERL_MM_OPT | basic |
| r | `renv-V` | default only | ✔ (1 call) | R version | `<env>` | R_PROFILE_USER, RENV_PROJECT⊘ | `Rscript <opts> …` (validated) |
| swift | `swift_env-V` | default only | ✘ | – | **process CWD** | PATH(`<env>/.build/release`) | basic |

`⊘` means UNSET. "default only" means `assert_version_default` raises `AssertionError` at install time (exit 3).

### 3.2 Part 1 summary (needed to validate the trait design; full details live in the part-1 spec)

| lang | env dir | default version | health | version support | deps | in_env | run |
|---|---|---|---|---|---|---|---|
| python | `py_env` | probe: `python3.X`/`python3` on PATH, `sys.executable` name, py launcher (win), else `default` (`python.py:108-131`) | pyvenv.cfg `version_info` vs `python -S -c` output; `base-executable` check | interpreter name or path (`-p`) | `pip install . <deps>` | PIP_DISABLE_PIP_VERSION_CHECK=1, PYTHONHOME⊘, VIRTUAL_ENV, PATH(bin\|Scripts) | basic |
| node | `node_env` | `system` if `exe_exists(node)` and `exe_exists(npm)` (non-win), else `default` | `node --version` rc | nodeenv `-n V` | `npm install -g` | NODE_VIRTUAL_ENV, NPM_CONFIG_PREFIX/npm_config_prefix, NPM_CONFIG_USERCONFIG⊘/npm_config_userconfig⊘, NODE_PATH, PATH | basic |
| golang | `golangenv` | `system` if `exe_exists(go)` | – | downloads `go{V}` (`default` means latest from go.dev JSON) | `go install <dep>` each | system: PATH; else GOROOT, GOTOOLCHAIN=local, PATH | basic |
| rust | `rustenv` | `system` if `cargo --version` rc==0 (cwd `/`) | – | rustup toolchain (`default` means `stable`) | lib: `cargo add`; `cli:` → `cargo install` | PATH, RUSTUP_TOOLCHAIN (non-system) | basic |
| ruby | `rbenv` | `system` if `exe_exists(ruby)` and `exe_exists(gem)` | – | bundled rbenv + ruby-build / ruby-download | `gem install` | GEM_HOME, GEM_PATH⊘, BUNDLE_IGNORE_CONFIG=1, PATH (+RBENV_ROOT, RBENV_VERSION) | basic |
| docker | `docker` | default | – | default only | ✘ | none | build image (no `--pull`) on **every** run, then `docker run … --entrypoint` |
| docker_image | None | default | – | n/a | n/a | none | `docker run …` + entry |
| unsupported (`system`) | None | default | – | n/a | n/a | none | basic |
| unsupported_script (`script`) | None | default | – | n/a | n/a | none | `prefix.path(cmd[0])` + rest (ignores `is_local`) |
| fail | None | default | – | n/a | n/a | none | no spawn: `(1, f'{entry}\n\n' + '\n'.join(files) + '\n')` |
| pygrep | None | default | – | n/a | n/a | none | `xargs((sys.executable,'-m','pre_commit.languages.pygrep',*args,entry), files)`: no shuffle, concurrency 1 |

---

## 4. Rust design: a `Language` backend abstraction for all 21 languages

### 4.1 Design constraints derived from the reference

1. **No process-env mutation.** pre-commit's `in_env` mutates `os.environ`. In Rust 2024, `std::env::set_var` and `remove_var` are `unsafe fn`, which conflicts with `#![forbid(unsafe_code)]`, and they are racy once hooks run in parallel (lefthook-style). So activation is **data**: an `EnvPatch` resolved against an immutable `Env` snapshot and applied per `Command`.
2. **Exe lookup must use the patched env.** `normalize_cmd` resolves `argv[0]` through the patched PATH, honours PATHEXT, and prepends the shebang interpreter. snp therefore resolves executables itself and spawns absolute paths.
3. **Idempotent and crash-safe installs.** State files are written last; a partial env without state files is wiped on the next install. This works with stale dirs, Ctrl-C, panics and concurrent snp processes (store file lock).
4. **Error parity.** Each failure carries the Python exception type name and an exit code (1 for `FatalError`, 3 otherwise), so the user-facing text can match §1.8 byte for byte.
5. **Cheap dispatch.** Backends are zero-sized types behind `&'static dyn Language`. Most behaviour comes from shared helpers.
6. **Sync API.** Installs and runs are bound by subprocesses. Use `std::thread::scope` for xargs partitions and parallel hooks. No async runtime is needed. If the project adopts tokio anyway, make the trait methods `async` with `#[trait_variant::make(Send)]` or boxed futures.

### 4.2 Core types

```rust
#![forbid(unsafe_code)]
use std::{borrow::Cow, collections::BTreeMap, ffi::{OsStr, OsString}, io, num::NonZeroUsize,
          path::{Component, Path, PathBuf}, sync::Arc};

/// pre-commit 4.6.2 `all_languages.languages`, in `language_names` (sorted) order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LanguageId {
    Conda, Coursier, Dart, Docker, DockerImage, Dotnet, Fail, Golang, Haskell, Julia, Lua,
    Node, Perl, Pygrep, Python, R, Ruby, Rust, Swift, Unsupported, UnsupportedScript,
}
impl LanguageId {
    pub const ALL: [Self; 21] = [/* the variants above, in order */];
    pub const fn as_str(self) -> &'static str { /* "conda", …, "docker_image", …, "unsupported_script" */ todo!() }
    /// `clientlib._translate_language` + `check_one_of(language_names)`.
    pub fn from_config(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Self::Unsupported),
            "script" => Some(Self::UnsupportedScript),
            _ => Self::ALL.into_iter().find(|l| l.as_str() == s),
        }
    }
}

/// Resolved `language_version` (after `_hook()` resolution, §1.3 step 1).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LanguageVersion(Arc<str>);
impl LanguageVersion {
    pub const DEFAULT: &'static str = "default";
    pub const SYSTEM: &'static str = "system";
    pub fn as_str(&self) -> &str { &self.0 }
    pub fn is_default(&self) -> bool { &*self.0 == Self::DEFAULT }
    pub fn is_system(&self) -> bool { &*self.0 == Self::SYSTEM }
}

/// Hook repo directory (clone / store-local template / CWD for env-less languages).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Prefix(Arc<Path>);
impl Prefix {
    /// `prefix.path(*parts)`: lexical join+normalize; absolute part replaces the prefix (Python parity).
    pub fn path(&self, parts: &[&OsStr]) -> PathBuf { /* Path::join + lexical normalize */ todo!() }
    /// `prefix.star(end)` but SORTED (deterministic; see §5).
    pub fn star(&self, suffix: &str) -> io::Result<Vec<OsString>> { todo!() }
    pub fn as_path(&self) -> &Path { &self.0 }
}

/// `<ENVIRONMENT_DIR>-<version>` below `prefix` (fixes §5.1).
/// Python allows interpreter PATHS as versions (`language_version: /usr/bin/python3.11`), which in
/// pre-commit yields the NESTED dir `<prefix>/py_env-/usr/bin/python3.11`. So separators are allowed,
/// but every component must be `Normal`: no `..`, no root or drive prefix, no NUL, no `:` on Windows.
pub fn env_dir(prefix: &Prefix, base: &'static str, v: &LanguageVersion) -> Result<PathBuf, LangError> {
    let name = format!("{base}-{}", v.as_str());
    let rel = Path::new(&name);
    let safe = !v.as_str().contains('\0')
        && !(cfg!(windows) && v.as_str().contains(':'))
        && rel.components().all(|c| matches!(c, Component::Normal(_)));
    if !safe {
        return Err(LangError::Value(format!("invalid language_version: {:?}", v.as_str())));
    }
    Ok(prefix.as_path().join(rel))     // lexical parity with prefix.path(): no `..` can remain
}
```

**Environment snapshots and patches** (the `envcontext` equivalent):

```rust
/// Immutable env. On Windows keys compare case-insensitively (Python upper-cases `os.environ` keys).
#[derive(Clone, Debug, Default)]
pub struct Env(Arc<BTreeMap<EnvKey, OsString>>);

#[derive(Clone, Debug)]
pub enum Part { Lit(OsString), Var { name: &'static str, default: &'static str } }
#[derive(Clone, Debug)]
pub enum EnvValue { Set(OsString), Unset, Template(Box<[Part]>) }

#[derive(Clone, Debug, Default)]
pub struct EnvPatch(Vec<(Cow<'static, str>, EnvValue)>);
impl EnvPatch {
    pub fn set(mut self, k: impl Into<Cow<'static, str>>, v: impl Into<OsString>) -> Self {
        self.0.push((k.into(), EnvValue::Set(v.into()))); self
    }
    pub fn unset(mut self, k: impl Into<Cow<'static, str>>) -> Self {
        self.0.push((k.into(), EnvValue::Unset)); self
    }
    /// pre-commit idiom `(d1, SEP, d2, SEP, Var(key))`.
    pub fn prepend_path<I: IntoIterator<Item = PathBuf>>(self, key: &'static str, dirs: I) -> Self { todo!() }
}
impl Env {
    pub fn from_process() -> Self { todo!() }
    /// envcontext semantics: ops in order; every `Var` reads `self` (pre-patch snapshot).
    pub fn patched(&self, p: &EnvPatch) -> Env { todo!() }
    pub fn get(&self, k: &str) -> Option<&OsStr> { todo!() }
    pub fn apply(&self, cmd: &mut std::process::Command) { cmd.env_clear(); /* .envs(..) */ }
}
```

**Host services.** All side effects go through these, so backends stay testable with the shim and golden technique (§5.12):

```rust
pub struct Host<'a> {
    pub base_env: &'a Env,               // process env at startup + PRE_COMMIT=1 (run.py:417)
    pub proc: &'a dyn ProcessRunner,
    pub probe: &'a ToolProbe,            // memoized: default versions, lua version, exe_exists…
    pub temp_root: &'a Path,             // parent for TempDirs (default std::env::temp_dir())
    pub self_exe: &'a Path,              // `pre-commit hazmat …` / pygrep fallbacks → snp itself
    pub home: Option<&'a Path>,          // exe_exists() home check
    pub http: Option<&'a dyn HttpClient>,// golang/rust/ruby toolchain downloads; None ⇒ offline error
}

pub struct Captured { pub code: i32, pub argv: Vec<OsString> /* normalized */, pub stdout: Vec<u8>, pub stderr: Vec<u8> }

pub trait ProcessRunner: Send + Sync {
    /// `util.cmd_output_b(check=False)`: `normalize_cmd` against `env` (PATH, PATHEXT, shebang),
    /// stdin = null, separate stdout/stderr pipes. Exe not found ⇒ Ok(code 1, stdout = "Executable `x` not found").
    fn capture(&self, argv: &[OsString], cwd: &Path, env: &Env) -> io::Result<Captured>;
    /// One xargs partition: stdout+stderr merged on ONE pipe (or a POSIX pty when `pty`), stdin = null.
    fn merged(&self, argv: &[OsString], cwd: &Path, env: &Env, pty: bool) -> io::Result<(i32, Vec<u8>)>;
}
```

**Contexts:**

```rust
pub struct EnvCtx<'a> {
    pub host: &'a Host<'a>,
    pub prefix: &'a Prefix,
    pub version: &'a LanguageVersion,
    pub env_dir: &'a Path,               // from env_dir(); may not exist yet
}
pub struct InstallCtx<'a> {
    pub cx: EnvCtx<'a>,
    pub deps: &'a [String],              // additional_dependencies, config order
}
pub struct RunCtx<'a> {
    pub host: &'a Host<'a>,
    pub prefix: &'a Prefix,
    pub version: &'a LanguageVersion,
    pub env_dir: Option<&'a Path>,
    pub env: &'a Env,                    // base_env.patched(activate()), cached per install_key
    pub entry: &'a str,
    pub args: &'a [String],
    pub files: &'a [OsString],           // empty if pass_filenames: false
    pub cwd: &'a Path,                   // git toplevel
    pub is_local: bool,                  // hook.src == "local"
    pub require_serial: bool,
    pub color: bool,
    pub jobs: NonZeroUsize,              // target_concurrency()
}
pub struct HookOutput { pub code: i32, pub output: Vec<u8> }
```

**Errors.** These map one-to-one onto §1.8:

```rust
#[derive(Debug, thiserror::Error)]
pub enum LangError {
    #[error("{0}")] Fatal(String),                       // FatalError → exit 1
    #[error("{0}")] Assertion(String),                   // AssertionError → exit 3
    #[error("{0}")] Value(String),                       // ValueError (shlex, R entry) → 3
    #[error("{0}")] Index(&'static str),                 // IndexError parity (empty entry) → 3
    #[error("{0}")] Key(String),                         // KeyError parity (dart executables) → 3
    #[error(transparent)] CalledProcess(#[from] CalledProcessError), // → 3, bytes format §1.7
    #[error("{0}")] ArgumentTooLong(String),             // → 3
    #[error(transparent)] Io(#[from] io::Error),         // OSError family → 3
}
impl LangError {
    pub fn python_type(&self) -> &'static str { /* "FatalError" | "AssertionError" | … | io kind → "FileNotFoundError"… */ todo!() }
    pub fn exit_code(&self) -> i32 { if matches!(self, Self::Fatal(_)) { 1 } else { 3 } }
}
```

### 4.3 The trait

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionSupport {
    /// `assert_version_default(<lang name>, v)` at install time (conda coursier dart docker dotnet haskell lua perl r swift)
    DefaultOnly,
    /// any string; meaningful (python node golang rust ruby)
    Any,
    /// any string; silently ignored except in the env-dir name (julia)
    Ignored,
}
#[derive(Clone, Copy, Debug)]
pub struct Support { pub version: VersionSupport, pub additional_dependencies: bool }

pub trait Language: Send + Sync + 'static {
    fn id(&self) -> LanguageId;

    /// `ENVIRONMENT_DIR`. `None` ⇒ env-less (docker_image fail pygrep unsupported unsupported_script):
    /// never installed, `activate` unused, local prefix = CWD, version/deps rejected at config time (exit 1).
    fn environment_dir(&self) -> Option<&'static str>;

    /// Checked by the orchestrator in pre-commit order (version, then deps) BEFORE any FS mutation.
    fn support(&self) -> Support { Support { version: VersionSupport::DefaultOnly, additional_dependencies: true } }

    /// `get_default_version()`. Memoized per process by `ToolProbe`.
    fn default_version(&self, _host: &Host<'_>) -> Result<LanguageVersion, LangError> {
        Ok(LanguageVersion(Arc::from(LanguageVersion::DEFAULT)))
    }

    /// `install_environment()`. Precondition: `cx.env_dir` does NOT exist (installers such as dart/swift/
    /// ruby/conda require absence). On `Err` the orchestrator deletes `env_dir`.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<(), LangError>;

    /// `health_check()`: Ok(None) healthy, Ok(Some(reason)) ⇒ rebuild, Err ⇒ abort (parity).
    fn health(&self, _cx: &EnvCtx<'_>) -> Result<Option<String>, LangError> { Ok(None) }

    /// `in_env()` as data. Also used by install steps that pre-commit runs "with in_env(…)".
    fn activate(&self, _cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> { Ok(EnvPatch::default()) }

    /// argv before filenames. Default = `lang_base.hook_cmd(entry, args)` (with hazmat → `<snp> hazmat`).
    fn command(&self, cx: &RunCtx<'_>) -> Result<Vec<OsString>, LangError> {
        hook_cmd(cx.entry, cx.args, cx.host.self_exe)
    }

    /// `run_hook()`. Default = `run_xargs(self.command(cx)?, cx)` (= `basic_run_hook`).
    fn run(&self, cx: &RunCtx<'_>) -> Result<HookOutput, LangError> { run_xargs(self.command(cx)?, cx) }
}

/// Registry: ZST backends, zero-cost `&'static dyn`.
pub fn backend(id: LanguageId) -> &'static dyn Language {
    match id {
        LanguageId::Conda => &Conda, LanguageId::Coursier => &Coursier, LanguageId::Dart => &Dart,
        LanguageId::Docker => &Docker, LanguageId::DockerImage => &DockerImage, LanguageId::Dotnet => &Dotnet,
        LanguageId::Fail => &Fail, LanguageId::Golang => &Golang, LanguageId::Haskell => &Haskell,
        LanguageId::Julia => &Julia, LanguageId::Lua => &Lua, LanguageId::Node => &Node,
        LanguageId::Perl => &Perl, LanguageId::Pygrep => &Pygrep, LanguageId::Python => &Python,
        LanguageId::R => &R, LanguageId::Ruby => &Ruby, LanguageId::Rust => &RustLang,
        LanguageId::Swift => &Swift, LanguageId::Unsupported => &Unsupported,
        LanguageId::UnsupportedScript => &UnsupportedScript,
    }
}
```

### 4.4 Orchestration: idempotent install, health checks, activation, run

```rust
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct InstallKey { prefix: Prefix, lang: LanguageId, version: LanguageVersion, deps: Arc<[String]> }

pub fn install_hook_envs(hooks: &[ResolvedHook], store: &Store, host: &Host<'_>) -> Result<(), LangError> {
    let need = || -> Result<Vec<&ResolvedHook>, LangError> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for h in hooks {                                   // order = config order
            if seen.insert(h.install_key()) && !is_installed(h, host)? { out.push(h); }
        }
        Ok(out)
    };
    if need()?.is_empty() { return Ok(()); }               // fast path: no lock
    let _lock = store.exclusive_lock()?;                   // File::lock(<store>/.lock); log "Locking pre-commit directory" if blocked
    for h in need()? { install_one(h, host)?; }            // re-check under lock (another process may have finished)
    Ok(())
}

fn is_installed(h: &ResolvedHook, host: &Host<'_>) -> Result<bool, LangError> {
    let lang = backend(h.lang);
    let Some(base) = lang.environment_dir() else { return Ok(true) };
    let dir = env_dir(&h.prefix, base, &h.version)?;
    let state_ok = dir.join(".install_state_v2").try_exists()?
        || read_state_v1(&dir)?.is_some_and(|deps| *deps == *h.deps);
    Ok(state_ok && lang.health(&h.env_ctx(host, &dir))?.is_none())
}

fn install_one(h: &ResolvedHook, host: &Host<'_>) -> Result<(), LangError> {
    log::info!("Installing environment for {}.", h.src);
    log::info!("Once installed this environment will be reused.");
    log::info!("This may take a few minutes...");
    let lang = backend(h.lang);
    let Some(base) = lang.environment_dir() else {             // no panics: mirror lang_base.no_install
        return Err(LangError::Assertion("This language is not installable".into()));
    };
    let dir = env_dir(&h.prefix, base, &h.version)?;
    check_support(h.lang, lang.support(), &h.version, &h.deps)?;   // verbatim assert_* messages
    if dir.try_exists()? { remove_tree_readonly_safe(&dir)?; }      // §1.7 rmtree parity, no symlink following
    let guard = RemoveDirOnDrop::new(&dir);                         // clean_path_on_failure (Err, panic)
    let cx = h.env_ctx(host, &dir);
    lang.install(&InstallCtx { cx, deps: &h.deps })?;
    if let Some(msg) = lang.health(&h.env_ctx(host, &dir))? {
        return Err(LangError::Assertion(format!(
            "BUG: expected environment for {} to be healthy immediately after install, \
             please open an issue describing your environment\n\nmore info:\n\n{msg}", h.lang.as_str())));
    }
    std::fs::create_dir_all(&dir)?;                                 // snp hardening (§1.3 step 8 edge)
    write_atomic(&dir.join(".install_state_v1"), state_v1_json(&h.deps).as_bytes())?; // staging + rename
    std::fs::OpenOptions::new().create(true).append(true).open(dir.join(".install_state_v2"))?;
    guard.disarm();
    Ok(())
}

/// Activation is computed ONCE per InstallKey per run (pre-commit recomputes per hook; e.g. lua spawns luarocks each time).
pub fn hook_env(h: &ResolvedHook, host: &Host<'_>, cache: &EnvCache) -> Result<Env, LangError> {
    let lang = backend(h.lang);
    match lang.environment_dir() {
        None => Ok(host.base_env.clone()),
        Some(base) => cache.get_or_try_insert(h.install_key(), || {
            let dir = env_dir(&h.prefix, base, &h.version)?;
            Ok(host.base_env.patched(&lang.activate(&h.env_ctx(host, &dir))?))
        }),
    }
}
```

`check_support` must emit exactly:
* `DefaultOnly` and `!v.is_default()`: ``for now, pre-commit requires system-installed {lang} -- you selected `language_version: {v}` ``
* `!additional_dependencies` and non-empty deps: ``for now, pre-commit does not support additional_dependencies for {lang} -- you selected `additional_dependencies: {py_list_repr(deps)}` ``

### 4.5 Mapping of every language onto the trait

Legend: "–" means the trait default. `P(x)` means `prepend_path("PATH", [x])`.

| Language | `environment_dir` | `support` (version / deps) | `default_version` | `health` | `activate` | `install` | `command` / `run` |
|---|---|---|---|---|---|---|---|
| conda | `conda` | DefaultOnly / ✔ | – | – | PYTHONHOME⊘ VIRTUAL_ENV⊘ CONDA_PREFIX P(bin) (+win extras) | env create + install | – |
| coursier | `coursier` | DefaultOnly / ✔ | – | – | P(`<env>`), COURSIER_CACHE | locate cs; channel apps; deps (under activate) | – |
| dart | `dartenv` | DefaultOnly / ✔ | – | – | P(bin) | pub get + compile; per-dep pub cache add | – |
| dotnet | `dotnetenv` | DefaultOnly / ✘ | – | – | P(bin) | pack; parse nupkg; tool install | – |
| haskell | `hs_env` | DefaultOnly / ✔ | – | – | P(bin) | cabal update + install | – |
| julia | `juliaenv` | **Ignored** / ✔ | – | – | JULIA_LOAD_PATH, JULIA_PROJECT⊘, JULIA_DEPOT_PATH | copy project/manifest; `julia -e` (under activate) | `command`: `julia --startup-file=no` + script_entry |
| lua | `lua_env` | DefaultOnly / ✔ | – | – | P(bin), LUA_PATH, LUA_CPATH (memoized `--lua-ver`) | make rockspecs; install deps (under activate) | – |
| perl | `perl_env` | DefaultOnly / ✔ | – | – | P(bin), PERL5LIB, PERL_MB_OPT, PERL_MM_OPT | `cpan -T .` (under activate) | – |
| r | `renv` | DefaultOnly / ✔ | – | **R versions** | R_PROFILE_USER, RENV_PROJECT⊘ | copy renv; 2–3 Rscript scripts (under activate) | `command`: validate + `Rscript <4 opts>` |
| swift | `swift_env` | DefaultOnly / ✘ | – | – | P(.build/release) | `swift build` (cwd = process CWD) | – |
| python | `py_env` | Any / ✔ | probe | pyvenv.cfg | PIP_…=1, PYTHONHOME⊘, VIRTUAL_ENV, P(bin\|Scripts) | virtualenv + pip | – |
| node | `node_env` | Any / ✔ | exe_exists probe | `node --version` | node vars | nodeenv + npm | – |
| golang | `golangenv` | Any / ✔ | exe_exists(go) | – | system vs managed GOROOT | download go (http) + go install | – |
| rust | `rustenv` | Any / ✔ | `cargo --version` | – | P(bin), RUSTUP_TOOLCHAIN | rustup (http) + cargo install | – |
| ruby | `rbenv` | Any / ✔ | exe_exists(ruby, gem) | – | GEM_*, RBENV_* | extract bundled tarballs + gem | – |
| docker | `docker` | DefaultOnly / ✘ | – | – | – | `docker build --pull` + mkdir | `run`: rebuild, then `docker run` |
| docker_image | None | n/a | – | – | – | unreachable | `command`: docker_cmd + hook_cmd |
| unsupported | None | n/a | – | – | – | unreachable | – (basic) |
| unsupported_script | None | n/a | – | – | – | unreachable | `command`: `prefix.path(cmd[0])` always |
| fail | None | n/a | – | – | – | unreachable | `run`: no spawn |
| pygrep | None | n/a | – | – | – | unreachable | `run`: native regex engine, no shuffle, 1 job |

### 4.6 Shared generic building blocks

| Helper | Semantics | Users |
|---|---|---|
| `hook_cmd(entry, args, self_exe)` | Python-compatible shlex split (§5.3), hazmat rewrite, append args | default `command`; julia, docker, docker_image, unsupported_script |
| `run_xargs(cmd, cx)` | serial order or `shuffled` + `jobs`, then `xargs` | every spawning language except pygrep and fail |
| `xargs(cmd, files, jobs, cx)` | §1.7: normalize once, partition, scoped threads, abs-max retcode, in-order concatenation | run_xargs, pygrep-equivalent |
| `prepend_bin(env_dir, &[sub…])` | `PATH = env_dir/sub … SEP $PATH` | conda, dart, dotnet, haskell, lua, perl, swift (`.build/release`), coursier (`""`), golang, rust, python, node, ruby |
| `script_entry(cmd, prefix, is_local)` | `cmd[0] = cmd[0] if is_local else prefix.path(cmd[0])` | julia; unsupported_script (always prefixed); r (`cmd[1]` unless `-e` or local) |
| `check_support` | the two verbatim assertions | every env language; DefaultOnly: conda, coursier, dart, docker, dotnet, haskell, lua, perl, r, swift; no-deps: docker, dotnet, swift |
| `setup_cmd(cx, argv, cwd, env)` | `capture` with check=True giving `CalledProcessError` | all installers |
| `with_activation(cx)` | `base_env.patched(activate(cx)?)` for "install inside in_env" steps | coursier, julia, lua, perl, r, node, python (pip step), ruby, rust |
| `copy_into_env(prefix, env, candidates)` | first existing file, copied keeping its basename | julia (Project/Manifest), r (renv.lock + renv/ tree) |
| `per_dep` vs `single_call` | luarocks and dart run one command per dep; the rest pass all deps in one call | lua, dart, golang / conda, coursier, haskell, perl, r, julia, python, node, ruby |
| `star_sorted(prefix, suffix)` | `prefix.star` but deterministic | haskell `.cabal`, lua `.rockspec`, ruby `.gemspec`/`.gem` |
| `temp_dir()` | `tempfile::TempDir` under `host.temp_root` | dart (PUB_CACHE, copies), dotnet (nuget.config), r (script.R), rust (RUSTUP_HOME) |
| `exe_exists(exe, env, home)` | §1.6 | node, golang, ruby default versions |
| `py_repr` / `sh_quote` | Python `repr()` (tuples, lists, str) and `shlex.quote` | CalledProcessError text, assert messages, perl, r (`prefix_dir` R literal needs its own escaper) |

### 4.7 Sketches for representative backends

```rust
pub struct Perl;
impl Language for Perl {
    fn id(&self) -> LanguageId { LanguageId::Perl }
    fn environment_dir(&self) -> Option<&'static str> { Some("perl_env") }
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        let d = cx.env_dir;
        let q = sh_quote(d.as_os_str());                                     // Python shlex.quote
        Ok(EnvPatch::default()
            .prepend_path("PATH", [d.join("bin")])
            .set("PERL5LIB", d.join("lib").join("perl5"))
            .set("PERL_MB_OPT", os_concat(["--install_base ".as_ref(), q.as_os_str()]))
            .set("PERL_MM_OPT", os_concat(["INSTALL_BASE=".as_ref(), q.as_os_str(),
                  " INSTALLSITEMAN1DIR=none INSTALLSITEMAN3DIR=none".as_ref()])))
    }
    fn install(&self, ic: &InstallCtx<'_>) -> Result<(), LangError> {
        let env = ic.cx.host.base_env.patched(&self.activate(&ic.cx)?);
        let argv = ["cpan", "-T", "."].into_iter().map(OsString::from)
            .chain(ic.deps.iter().map(OsString::from)).collect::<Vec<_>>();
        setup_cmd(&ic.cx, &argv, ic.cx.prefix.as_path(), &env).map(drop)
    }
}

pub struct Lua;
impl Language for Lua {
    fn id(&self) -> LanguageId { LanguageId::Lua }
    fn environment_dir(&self) -> Option<&'static str> { Some("lua_env") }
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        let v = cx.host.probe.lua_version(cx.host)?;      // OnceLock<Result<Arc<str>,…>>: `luarocks config --lua-ver`, trimmed
        let so = if cfg!(windows) { "dll" } else { "so" };
        let d = cx.env_dir;
        let share = d.join("share").join("lua").join(&*v);
        let mut lua_path = share.join("?.lua;").into_os_string();
        lua_path.push(share.join("?").join("init.lua;;"));
        Ok(EnvPatch::default()
            .prepend_path("PATH", [d.join("bin")])
            .set("LUA_PATH", lua_path)
            .set("LUA_CPATH", d.join("lib").join("lua").join(&*v).join(format!("?.{so};;"))))
    }
    fn install(&self, ic: &InstallCtx<'_>) -> Result<(), LangError> {
        let env = ic.cx.host.base_env.patched(&self.activate(&ic.cx)?);
        std::fs::create_dir_all(ic.cx.env_dir)?;
        let tree = ic.cx.env_dir.as_os_str();
        for spec in ic.cx.prefix.star(".rockspec")? {
            setup_cmd(&ic.cx, &os_vec(["luarocks".as_ref(), "--tree".as_ref(), tree, "make".as_ref(), &spec]),
                      ic.cx.prefix.as_path(), &env)?;
        }
        for dep in ic.deps {
            setup_cmd(&ic.cx, &os_vec(["luarocks".as_ref(), "--tree".as_ref(), tree, "install".as_ref(), dep.as_ref()]),
                      ic.cx.prefix.as_path(), &env)?;
        }
        Ok(())
    }
}

pub struct Julia;
impl Language for Julia {
    fn id(&self) -> LanguageId { LanguageId::Julia }
    fn environment_dir(&self) -> Option<&'static str> { Some("juliaenv") }
    fn support(&self) -> Support { Support { version: VersionSupport::Ignored, additional_dependencies: true } }
    fn command(&self, cx: &RunCtx<'_>) -> Result<Vec<OsString>, LangError> {
        let mut cmd = hook_cmd(cx.entry, cx.args, cx.host.self_exe)?;
        let first = cmd.first_mut().ok_or(LangError::Index("tuple index out of range"))?;
        if !cx.is_local { *first = cx.prefix.path(&[first.as_os_str()]).into_os_string(); }
        cmd.splice(0..0, ["julia".into(), "--startup-file=no".into()]);
        Ok(cmd)
    }
    /* activate: JULIA_LOAD_PATH/JULIA_PROJECT⊘/JULIA_DEPOT_PATH; install: §2.6 */
    fn install(&self, _ic: &InstallCtx<'_>) -> Result<(), LangError> { todo!() }
}

pub struct Fail;
impl Language for Fail {
    fn id(&self) -> LanguageId { LanguageId::Fail }
    fn environment_dir(&self) -> Option<&'static str> { None }
    fn install(&self, _: &InstallCtx<'_>) -> Result<(), LangError> {
        Err(LangError::Assertion("This language is not installable".into()))
    }
    fn run(&self, cx: &RunCtx<'_>) -> Result<HookOutput, LangError> {
        let mut out = format!("{}\n\n", cx.entry).into_bytes();
        for (i, f) in cx.files.iter().enumerate() {
            if i > 0 { out.push(b'\n'); }
            out.extend_from_slice(f.as_encoded_bytes());
        }
        out.push(b'\n');
        Ok(HookOutput { code: 1, output: out })
    }
}
```

R overrides `health`, which runs two renv-activated scripts with cwd `<env>`, and `command`:
```rust
fn command(&self, cx: &RunCtx<'_>) -> Result<Vec<OsString>, LangError> {
    let cmd = py_shlex_split(cx.entry)?;                       // NOT hook_cmd (no hazmat rewrite)
    if cmd.first().map(String::as_str) != Some("Rscript") { return Err(LangError::Value("entry must start with `Rscript`.".into())); }
    let second = cmd.get(1).ok_or(LangError::Index("list index out of range"))?;
    if second == "-e" { if cmd.len() > 3 { return Err(LangError::Value("You can supply at most one expression.".into())); } }
    else if cmd.len() > 2 {
        return Err(LangError::Value("The only valid syntax is `Rscript -e {expr}`or `Rscript path/to/hook/script`".into()));
    }
    let mut out: Vec<OsString> = ["Rscript", "--no-save", "--no-restore", "--no-site-file", "--no-environ"].map(Into::into).into();
    if second == "-e" || cx.is_local { out.extend(cmd[1..].iter().map(Into::into)); }
    else { out.push(cx.prefix.path(&[OsStr::new(second)]).into_os_string()); }
    out.extend(cx.args.iter().map(Into::into));
    Ok(out)
}
```

---

## 5. Implementation notes for snp

### 5.1 Security finding: `language_version` path traversal deletes arbitrary directories (verified)

* **Root cause.** `environment_dir()` interpolates `language_version` verbatim into `prefix.path(f'{ENV}-{version}')`. `normpath` then collapses `..` lexically. `_hook_install` calls `rmtree(venv)` **before** `install_environment` gets a chance to reject the version (`repository.py:79-82`), and `clean_path_on_failure` removes the same path again on failure.
* **Reproduction** with pre-commit 4.1.0, sandboxed in scratchpad (`tmp-langs2/scripts/trav.sh`): a local `language: perl` hook with `language_version: '/../../../victim'`. That resolves to `<store>/repoXXXX/perl_env-/../../../victim`, which is `<store parent>/victim`. Result: the output was ``AssertionError: for now, pre-commit requires system-installed perl -- you selected `language_version: /../../../victim` ``, and **the pre-existing `victim/` directory with its file was deleted**.
* **snp fix.**
  * Build the env dir with `env_dir()` (§4.2). Every component of `<ENV>-<version>` must be `Component::Normal`, which rejects `..`, roots, drives, NUL and (on Windows) `:`. Separators stay allowed because python accepts interpreter paths as versions.
  * Run `check_support` **before** any filesystem mutation.
  * Make `remove_tree` assert that `path.starts_with(prefix) && path != prefix` and that `path` is not a symlink.
  * The same reasoning applies to `default_language_version` values.
  * Error text for rejected versions is new snp text (pre-commit has none). Keep the exit code at 3.

### 5.2 Spawning processes (parity with `cmd_output_b`, `cmd_output_p`, `xargs`)

* Never call `std::env::set_var` or `remove_var`; they are `unsafe` in edition 2024. Build each `Command` with `env_clear()` plus the resolved `Env`, or apply only the diff. Either way the child env must equal `base_env.patched(patch)`.
* **Resolve argv[0] yourself**, using the *child's* PATH and `PATHEXT` exactly as in §1.7: `normexe`, then shebang via `file_identify::parse_shebang_from_file`, then `normexe` again. Spawn the absolute path. A not-found exe must turn into **retcode 1 with output** ``Executable `<orig>` not found`` (no newline), not into an `io::Error`.
* For X_OK use `rustix::fs::access(path, Access::EXEC_OK)` (a safe API), which matches Python's `os.access`. On Windows treat "exists" as executable, as Python does.
* stdin is always **null/EOF** (`Stdio::null()`): during installs (no interactive prompts from conda, cpan or dotnet) and during hooks.
* **Merged output.** pre-commit uses `stderr=STDOUT`, so ordering is interleaved exactly as the child writes. Use `std::io::pipe()` (stable since 1.87): pass `writer.try_clone()?` as stdout and `writer` as stderr, drop the parent's copies, and `read_to_end` on the reader while waiting. Do **not** use two pipes and concatenate them.
* **Color/pty** (POSIX, when `color` is set): open a pty (for example `rustix-openpty`, or `nix::pty::openpty`), clear `OPOST|ONLCR` on the slave, give the child stdin `/dev/null`, and read the master until `EIO`. Windows falls back to pipes. Docker adds `--tty` when color is set.
* **Return codes.** Python reports a signal as `-signum`. Use `ExitStatusExt::signal()` to produce negative codes, and aggregate across partitions with **max abs, first wins** (§1.7).
* `CalledProcessError` display must reproduce §1.7 byte for byte. That includes Python **tuple repr** of the *normalized* argv: `('a',)` for one element, `'…'` or `"…"` quoting per Python's `repr(str)` rules, `\\` and `\n` escapes, `\xNN` / `\uNNNN` for non-printables, printable non-ASCII kept as is. A `py_repr` module covering `str`, `tuple` and `list` is needed. It is also used by `assert_no_additional_deps` (list repr, for example `['foo']`) and by R's `prefix_dir` literal.
* Decoding: `cmd_output` decodes strictly as UTF-8 (used by R's `out`, lua's `--lua-ver`, python version probes). Prefer `String::from_utf8_lossy` and document the deviation, because Python would crash with `UnicodeDecodeError` (exit 3). Arguably the lossy behaviour is a bug fix.

### 5.3 shlex parity: do NOT use the `shlex` crate as-is

Python `shlex.split(s)` is `shlex(s, posix=True)` with `whitespace_split=True` and **`commenters=''`**. The `shlex` crate (checked versions 1.3.0 and 2.0.1) differs in four ways:
1. It **treats `#` at the start of a word as a comment** (`bytes.rs:139-141`).
2. Its whitespace set is `' \t\n'`, whereas Python's is `' \t\r\n'`.
3. Inside double quotes it applies POSIX-shell escaping of `$` and backtick; Python only lets `\` escape `\` and `"` there.
4. Its error behaviour is different.

Write a small hand-rolled splitter and test it against these vectors, all produced by CPython 3.12:

| input (Python repr) | `shlex.split` |
|---|---|
| `'a #b'` | `['a', '#b']` |
| `'a "x\\$y"'` | `['a', 'x\\$y']` (the backslash is kept) |
| `'a\rb'` | `['a', 'b']` |
| `'a""b'` | `['ab']` |
| `'""'` | `['']` |
| `'a ""'` | `['a', '']` |
| `"a 'b\\c'"` | `['a', 'b\\c']` |
| `'a b\\ c'` | `['a', 'b c']` |
| `'a "b\\"c"'` | `['a', 'b"c']` |
| `'x\xa0y'` | `['x\xa0y']` (NBSP is not whitespace) |
| `'a\x0bb\x0cc'` | `['a\x0bb\x0cc']` |
| `'a\\'` | `ValueError: No escaped character` |
| `'a "b'`, `"a 'b"` | `ValueError: No closing quotation` |

`perl` needs Python `shlex.quote` (§2.8): an empty string becomes `''`; a string where every char is in `[A-Za-z0-9_@%+=:,./-]` is returned as is; anything else becomes `'…'`, with `'` replaced by `'"'"'`.

### 5.4 Environment handling

* **Windows key case.** `os.environ` upper-cases keys, so `Path` and `PATH` are the same key. Rust's `Command` compares keys case-insensitively on Windows, so the `Env` map must as well (an `EnvKey` that is case-folded on Windows).
* **Trailing separator.** `prepend_path` with an unset `PATH` yields `"<dir>:"`, and the empty element means **CWD** on POSIX, which is a mild security smell. Recommendation: omit the separator when the var is unset or empty. This is a deliberate, harmless deviation; mention it in the changelog.
* `PRE_COMMIT=1` must be in `base_env` before installs (§1.3 step 9), because installers observe it.
* Variables that pre-commit **replaces outright**, without merging the user's value: `PERL5LIB`, `PERL_MB_OPT`, `PERL_MM_OPT`, `LUA_PATH`, `LUA_CPATH`, `JULIA_LOAD_PATH`, `JULIA_DEPOT_PATH`, `CONDA_PREFIX`, `COURSIER_CACHE`, `R_PROFILE_USER`. Keep these as replacements for parity.
* Truthiness: `PRE_COMMIT_USE_MICROMAMBA` and `PRE_COMMIT_USE_MAMBA` count as **non-empty means true**, so even `"0"` is true. `PRE_COMMIT_NO_CONCURRENCY` and `TRAVIS` are **presence** checks.
* Env vars **read** by the part-2 code paths: `PATH`, `PATHEXT`, `HOME`/`USERPROFILE` (via `expanduser`), `PRE_COMMIT_USE_MICROMAMBA`, `PRE_COMMIT_USE_MAMBA`, `PRE_COMMIT_NO_CONCURRENCY`, `TRAVIS`, `R_HOME`, `TMPDIR`/`TEMP`/`TMP` (temp dirs).
* Env vars **set or unset**: `PATH`, `CONDA_PREFIX`, `PYTHONHOME`⊘, `VIRTUAL_ENV`⊘, `COURSIER_CACHE`, `PUB_CACHE` (install only), `JULIA_LOAD_PATH`, `JULIA_PROJECT`⊘, `JULIA_DEPOT_PATH`, `LUA_PATH`, `LUA_CPATH`, `PERL5LIB`, `PERL_MB_OPT`, `PERL_MM_OPT`, `R_PROFILE_USER`, `RENV_PROJECT`⊘. Inside the Julia code, `JULIA_PKG_PRECOMPILE_AUTO` is set via `withenv`.

### 5.5 Concurrency, ordering, determinism

* `target_concurrency`: snp can use `std::thread::available_parallelism()`. On Linux it respects sched affinity, like Python's `sched_getaffinity`, **and** cgroup CPU quotas, which Python ignores. That only affects how many partitions run concurrently, not their content, so the difference is acceptable. Fall back to 1 on error. Keep `PRE_COMMIT_NO_CONCURRENCY` giving 1 and `TRAVIS` giving 2.
* Keep the **bit-exact shuffle** (§1.6). It decides which files land in which partition, and therefore what each hook process sees and the output order. Not shuffling when `require_serial` is set is part of the contract (test `test_xargs_require_serial_is_not_shuffled`).
* `xargs` max length: compute it **from the env the child actually receives**, i.e. the patched env (pre-commit computes it from the env at import time). The difference is only a few bytes; prefer correctness.
* `Prefix.star`, `os.listdir(build_dir)`, `os.listdir(.pre-commit-channel)` and `os.walk(PUB_CACHE)` follow filesystem order in pre-commit. snp should **sort** them (byte order) for reproducibility. Nobody can rely on ext4 hash order.
* Installs: pre-commit runs them one at a time under one store lock. snp may install **distinct** `InstallKey`s in parallel while holding the lock, as long as no two installs share a `prefix`: installers write into the prefix (`dart pub get`, `dotnet pack`, `cabal`, `cpan`). Buffer each install's log lines and emit them in config order, so output stays deterministic.

### 5.6 Filesystem behaviour

* **Do not pre-create the env dir** before `install`. `dart` (`os.makedirs` without `exist_ok`), `swift` (`os.mkdir`), `conda env create -p`, and ruby's `shutil.move(…, envdir)` (part 1) all depend on it being absent.
* **Do** `create_dir_all(env_dir)` after a successful install and health check, before writing state. This avoids the coursier and perl `FileNotFoundError` edge case.
* State files: write `.install_state_v1staging`, then `rename` to `.install_state_v1`; then create `.install_state_v2` (empty). Write them **last**. This is only required if snp shares pre-commit's store or wants mutual compatibility (open question 1). Either way, keep "completion marker written last" as the idempotency protocol.
* Removal: `std::fs::remove_dir_all` has been symlink-safe since Rust 1.58.1. On Windows, add readonly handling: on `PermissionDenied`, clear the readonly bit on the entry and its parent, then retry (parity with `util._handle_readonly`).
* `RemoveDirOnDrop` covers both `Err` and panics. Also install a Ctrl-C handler (for example the `ctrlc` crate) that sets a flag, lets the children die (same process group), and unwinds normally, so guards run. Exit 130 with `Interrupted (^C): KeyboardInterrupt: ` (§1.8).
* Copies: `shutil.copy` copies data and **mode bits**; `copytree` copies recursively, follows symlinks to files, and copies stat via `copy2`. For R's `renv/` and Julia's TOMLs, `fs::copy` plus a recursive walk is enough. Preserve the exec bit.
* Temp dirs (`tempfile::TempDir`) live under `$TMPDIR`, as in pre-commit. The dart PUB_CACHE downloads can be large; consider `<store>/tmp` for same-filesystem renames, which is harmless.

### 5.7 Parsing untrusted inputs safely

* **dart `pubspec.yaml`**: parse with the same safe YAML stack as the config. Only `executables` (a mapping) matters.
  * A **missing key** produces `KeyError: 'executables'` (exit 3); `null` produces `TypeError`. snp may give friendlier errors, but the exit code should stay 3.
  * Validate each executable name as a single path component, to stop `../` escaping `<env>/bin`.
* **dotnet nupkg**: use the `zip` crate. Read **only** the single `*.nuspec` entry, with a size cap (for example 1 MiB) as zip-bomb protection. Parse with `roxmltree`, which does not expand external entities. Namespace and `id` lookup are as in §2.4 (first `id` element in the root's namespace, document order).
* **julia/coursier/haskell/perl/r/lua deps** are passed as argv elements, never through a shell, so there is no quoting risk. The exception is julia, where the deps are space-joined inside Julia code; that is pre-commit's own design, so keep it.

### 5.8 Performance and memory

* Memoize per process with `OnceLock`: the lua version, every `get_default_version` probe (`exe_exists`, `cargo --version`, python discovery), and `find_executable` results keyed by (PATH, name).
* **Compute activation once per `InstallKey`** and share it via `Arc<Env>` across hooks. This removes the repeated `luarocks config --lua-ver` spawns (observed once per hook).
* R health costs 2 Rscript startups per R env per run, roughly 0.3–2 s each with renv. An optional snp optimization: record `(Rscript path, size, mtime, R version)` in `<env>/.snp_r_stamp` at install time. On later runs, only spawn the two scripts when the stamp is missing or the binary changed. If the stamp matches, skip the check; the env is then treated as healthy. pre-commit would also judge it healthy, so there is no observable difference.
* Avoid running the health check twice when an install is needed (pre-commit recomputes under the lock). snp may cache "healthy" results within a run, but must re-check `state_ok` under the lock.
* Command lines: build `Vec<OsString>` once and share the immutable command prefix between partitions (for example `Arc<[OsString]>`).
* Stream install output to a per-install log buffer, or to `<store>/snp.log`, instead of holding it in memory. Only the tail is needed for the error message. Keep the full output for the log file, because pre-commit's message includes all of it.

### 5.9 Platform notes

| Topic | Linux | macOS | Windows |
|---|---|---|---|
| exe suffix | none | none | `win_exe` adds `.exe` (dart outputs, `R_HOME/bin/Rscript.exe`); `PATHEXT` lookup |
| conda PATH | `<env>/bin` | `<env>/bin` | `Library\bin;Scripts;<env>;bin` |
| lua | supported | supported | untested; `?.dll` |
| swift | supported | supported | unsupported (`# pragma: win32 no cover`) |
| perl `INSTALL_BASE` | usually unquoted | usually unquoted | quoted with `'…'`, because `\` is unsafe for `shlex.quote` |
| R `activate.R` path | `getwd()` | `getwd()` | forward slashes from R |
| path separator | `:` | `:` | `;` (also `JULIA_DEPOT_PATH` trailing `;`) |
| xargs limit | `ARG_MAX`-derived, clamped to `[4096, 131072]` | same | `30720`; `.bat`/`.cmd`: `8192 - len(cmd.exe) - 4 - 1024` |
| readonly removal | n/a | n/a | chmod and retry |

### 5.10 Parity decisions on quirks and bugs found

| # | Quirk (pre-commit 4.6.2) | Recommendation |
|---|---|---|
| 1 | `language_version` traversal deletes directories (§5.1) | **Fix** (reject) |
| 2 | R validation message lacks a space: `` `…{expr}`or `…` `` | Keep verbatim for output parity |
| 3 | R health message says "now set to" but sets nothing | Keep verbatim |
| 4 | R run uses PATH `Rscript` while install and health use `$R_HOME` | Keep, but warn in debug logs when they resolve to different binaries |
| 5 | Lua requires `luarocks` at **run** time | Keep (needed for `LUA_PATH`); memoize |
| 6 | dart ignores pubspec executable alias targets | Keep. Fixing it would change which binaries exist. |
| 7 | `repo: local` dotnet and swift cannot work (no templates) | Keep the behaviour; optionally improve the error message |
| 8 | coursier with an empty channel dir crashes on the state write | Fix via `create_dir_all` (harmless) |
| 9 | An empty entry gives `IndexError: tuple index out of range`; an R single token gives `IndexError: list index out of range` | Produce the same type and text, exit 3 |
| 10 | Health-check command failure aborts instead of rebuilding | Keep (parity), or treat it as unhealthy behind a flag |
| 11 | `PRE_COMMIT_USE_MAMBA=0` means "use mamba" | Keep (documented truthiness) |
| 12 | `pre-commit-build/` left inside the dotnet clone | Keep |
| 13 | Julia accepts and ignores `language_version` | Keep (parity); maybe log at debug |
| 14 | Trailing PATH separator when `PATH` is unset | Fix (omit the separator) |
| 15 | Unsorted `listdir` orders | Fix (sort) |

### 5.11 What snp must replace because pre-commit relies on its own Python interpreter

* `hook_cmd` hazmat rewrite: `pre-commit hazmat …` becomes `<snp exe> hazmat …`, with snp implementing `cd`, `ignore-exit-code` and `n1` (`commands/hazmat.py`). Also consider accepting `snp hazmat …` in entries.
* `pygrep` runs `sys.executable -m pre_commit.languages.pygrep`. snp should implement it natively, which needs Python-`re`-compatible regex semantics (part-1 spec).
* `python` and `node` use `sys.executable -m virtualenv` and `-m nodeenv` (part 1). These need a discovered Python, or a reimplementation.

### 5.12 Testing strategy (the technique used for this spec)

* **Shim harness.** A directory of fake `conda`, `cs`, `dart`, `dotnet`, `cabal`, `julia`, `luarocks`, `cpan`, `Rscript` and `swift` executables (one script, dispatching on `argv[0]`). They log argv, cwd and selected env vars as JSON lines, and create the artifacts the next step expects (env `bin/` entries, nupkg with nuspec, `activate.R`, and so on). Put them first on PATH and assert on the log as a **golden file**. The same harness runs against `pre-commit` for differential testing.
* Golden error strings: every message quoted in §1–§2.
* Unit vectors: the shuffle (n=10 gives `[4,0,5,1,8,6,2,3,7,9]`), shlex (§5.3), `exe_exists` (§1.6), R entry validation (`r_test.py:19-130`), dotnet nuspec parsing (namespace present, absent, empty id).
* Real-toolchain integration tests, gated by `which <tool>`, mirroring `tests/languages/*_test.py`.

### 5.13 lefthook-inspired extensions that fit the trait (opt-in, off by default)

* Per-hook `env:`, applied **after** the language patch (`base_env.patched(lang).patched(hook_env)`).
* Per-hook `root:` (run in a subdir and strip the prefix from filenames, like `hazmat cd`), done by changing `RunCtx.cwd` and remapping `files`.
* Parallel hook execution across hooks with disjoint file sets. This is safe because `Env` is immutable and the store lock only guards installs.
* Live streaming of install and hook output (lefthook style), while still producing pre-commit's final summary format.
* Lazy installs: skip installing envs for hooks that have no matching files. pre-commit installs them anyway (§1.3 step 4), so this has to be opt-in.

---

## 6. Open questions

1. **Store sharing.** Should snp reuse pre-commit's store (`$PRE_COMMIT_HOME` or `~/.cache/pre-commit`, `db.db`, `repo*`, `.install_state_v1/v2`) to avoid re-cloning and reinstalling, or use its own (`~/.cache/snp`)? Sharing means byte-compatible state JSON, identical env dir names (`<ENV>-<version>`) and identical lock semantics, with a risk of mismatched env layouts. Recommendation: use a separate store by default, with an import/sharing mode decided by the store spec.
2. **Branding in messages.** Messages say "pre-commit" (for example ``for now, pre-commit requires system-installed …``). Keep them verbatim for script and grep compatibility, or rebrand as "snp"? Recommendation: verbatim in compat mode.
3. **Version strictness.** §4.2 keeps pre-commit's nested-dir layout for path-like versions (python `language_version: /usr/bin/python3.11` gives `<prefix>/py_env-/usr/bin/python3.11`) and rejects only `..`, root, drive, NUL and `:` components. Alternatively, should snp **encode** the version into one flat component (percent-encoding or a short hash plus a readable slug)? That is cleaner, but it breaks layout compatibility with a shared store. On Windows, python versions such as `C:\Python311\python.exe` contain `:` and **must** be encoded, because the §4.2 rule would reject them. pre-commit itself produces an invalid path there, so check what it actually does on Windows. Decide together with the part-1 spec.
4. Should R health results be cached across runs via the stamp file (§5.8), given that a broken renv library would no longer be detected? pre-commit does not detect that either, so the risk looks low.
5. `SNP_*` aliases for `PRE_COMMIT_USE_MAMBA`, `PRE_COMMIT_USE_MICROMAMBA` and `PRE_COMMIT_NO_CONCURRENCY`? If added, which wins when both are set?
6. Should snp honour `R_HOME` at run time (fixing quirk 4) or keep strict parity?
7. Lua: should the lua version also be recorded at install time and checked as a health check (prek does this), so envs get rebuilt after a Lua upgrade? That would be an improvement with no compat cost, except an extra spawn at install.
8. Is the parallel install of distinct envs (§5.5) acceptable given interleaved network and CPU load, or should it be capped (for example `min(jobs, 4)`)?
9. Julia: keep ignoring `language_version`, or implement juliaup selection as the upstream TODO suggests? That would diverge from pre-commit.
10. Should `cmd_output` UTF-8 strictness be relaxed to lossy (§5.2)? Doing so changes crash behaviour into success, which is a visible difference.
11. Windows: is snp going to support `lua` and `swift` on Windows at all (pre-commit does not test them there), or should it error early with a clear message?
