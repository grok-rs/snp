# 10 — Test strategy: proving `snp` is a drop-in replacement for pre-commit

Status: research spec. Audience: engineers implementing snp (Rust 1.98.0, edition 2024) who will NOT re-read the Python/Go sources.
All `path.py:LINE` citations are relative to `refs/pre-commit/` unless prefixed (`prek/...`, `file-identify/...`).
"Observed" = captured in this session by running the real binaries in throwaway repos under `scratchpad/tmp-test-strategy/`.

---

## 0. Oracles, versions, environment facts

| Item | Value | Notes |
|---|---|---|
| Target compatibility version | **pre-commit 4.6.2** (`setup.cfg` `version = 4.6.2`) | refs/pre-commit is at 4.6.2 |
| Installed reference | pre-commit **4.1.0** at `~/.local/bin/pre-commit` (pipx venv, CPython 3.12.3, PyYAML 6.0.2, identify 2.6.7, cfgv 3.4.0) | older than target |
| 4.6.2 oracle built this session | `scratchpad/tmp-test-strategy/venv462/bin/pre-commit` via `uv venv venv462 --python 3.12 && uv pip install --python venv462/bin/python pre-commit==4.6.2` | deps resolved: identify 2.6.20, cfgv 3.5.0, PyYAML 6.0.3 (libyaml `CSafeLoader` available), virtualenv 21.14.5, nodeenv 1.11.0 |
| git | 2.43.0 locally | `git ls-files --deduplicate` (used by 4.6.1+ `get_all_files`, `git.py:155`) needs git >= 2.31 |
| CPUs | 22 (`nproc`) | affects xargs partitioning (see §5.4) |
| Rust toolchains | `1.95.0` (**active default**), `1.98.0` installed | snp MUST ship `rust-toolchain.toml` with `channel = "1.98.0"` or every `cargo` call silently uses 1.95 |
| Stale binary | `~/.cargo/bin/snp` = **snp v1.0.0** (`cargo install --list`: "snp v1.0.0 (/home/dromix/projects/snp)") | tests must never resolve `snp` via `PATH`; use `assert_cmd::cargo::cargo_bin!("snp")` / `env!("CARGO_BIN_EXE_snp")`; the differential harness takes an explicit `--snp-bin` |
| User global git config | `core.pager=delta`, `interactive.difffilter=delta --color-only`, `merge.conflictstyle=zdiff3`, several `includeIf` | every test must isolate git config (§7.3.2), otherwise merge-conflict fixtures and diff output differ |

### 0.1 Behavior deltas 4.1.0 → 4.6.2 that change test expectations (`CHANGELOG.md`)

| Version | Change | Test impact |
|---|---|---|
| 4.2.0 | python default version: try versioned exe (`python3.12`) before `sys.executable`; fix conflict detection when a file is named `HEAD` | `git_test.py:144-150` only passes on >= 4.2 |
| 4.3.0 | rootless docker detection; julia avoids `startup.jl`; dart sdk bound | docker/julia/dart tests |
| 4.4.0 | `run --fail-fast` CLI flag; `language: unsupported`/`unsupported_script` (aliases of `system`/`script`; `system`→`unsupported`, `script`→`unsupported_script` after defaults, `clientlib_test.py:385-394`); docker-in-docker cgroups v2; `SecurityOptions: null`; error context for invalid `stages` now includes `==> At key: stages` (`clientlib_test.py:329`) | version-gate these scenarios in the differential harness |
| 4.5.0 | `pre-commit hazmat` subcommand | hazmat scenarios need >= 4.5 |
| 4.5.1 | local python hook without deps fix | — |
| 4.6.0 | `hook-impl --hook-dir` optional (git 2.54 config-based hooks: `hook_dir is None` → no legacy run, `hook_impl.py:36-37`); `--hook-type` required | hook-impl tests |
| 4.6.1 | node installs via git; `JULIA_DEPOT_PATH`; autoupdate errors on unknown `--repo` (`repos (dne, foo) were not found in .pre-commit-config.yaml`, exit 1); faster pre-push commit existence check; `run --all-files` no longer duplicates conflicted filenames (`ls-files --deduplicate`) | autoupdate/pre-push/all-files scenarios |
| 4.6.2 | node `scripts.build` fix | node only |

**Decision:** the differential oracle is 4.6.2 (pinned, installed in an isolated venv by the harness bootstrap). 4.1.0 is used only as a secondary oracle for scenarios tagged `stable_since <= 4.1`.

### 0.2 Baseline timings measured this session (pre-commit, median of 9, 22 CPUs, Linux 7.0)

| Scenario | pre-commit 4.6.2 | pre-commit 4.1.0 |
|---|---|---|
| `--version` | 172 ms | — |
| `run noop-01` (1 local `system` hook `true`, `always_run`, `pass_filenames: false`) | 234 ms | — |
| `run` (10 such hooks, nothing staged) | 243 ms | 272 ms |
| `run -a` (10 hooks) | 223 ms | — |

Python interpreter start-up (~170 ms) dominates. snp budget proposals in §7.11.

---

## 1. Top-level decisions (summary)

1. **Three-tier comparison model**: Tier A byte-exact (run output, `[INFO]/[WARNING]/[ERROR]` lines, hook output, exit codes, files written, git state); Tier B semantic (argparse usage/help text, YAML parser error *bodies*, Python tracebacks, `pre-commit.log` body); Tier C ignored (durations, patch file names, env-install chatter of external tools). Every scenario declares the tier per stream.
2. **Differential harness runs both tools sequentially at the *same absolute paths*** with fixed `GIT_*_DATE`, isolated `HOME`/`GIT_CONFIG_GLOBAL`/`PRE_COMMIT_HOME`, so absolute paths and commit SHAs are identical and need no normalization.
3. **Port pre-commit's suite as Rust integration tests** (~300 catalogued rows in §5, many parametrized, plus ~60 language pure-logic cases) using a `snp-testkit` crate that recreates `testing/resources/*` fixture repos and conftest fixtures (`in_merge_conflict`, etc.).
4. **Python-parity property tests** for every place where pre-commit leans on Python semantics: `random.Random(1542676187).shuffle` (file order!), `re.search` (regex dialect incl. lookbehind), `str.splitlines(True)`, `unicodedata.east_asian_width`, `round(x, 2)` float repr, YAML 1.1 implicit typing & PyYAML quoting, `shlex.split/quote`. Oracle vectors are generated once by a pinned Python and committed as JSON (CI needs no Python); live-oracle mode runs nightly.
5. **Deterministic probe hook binary** (`snp-probe`, Rust, in workspace) used as `entry:` in scenarios so outputs reveal argv, partitioning, env, tty-ness, cwd and file mutations identically under both tools and on all OSes.
6. Gate tests by capability: `net_*` (network), `tc_<lang>_*` (toolchain), `pty_*`, `win_*`, `diff_*` via nextest filtersets; default `cargo nextest run` is hermetic and offline.
7. Benchmarks: hyperfine end-to-end vs pre-commit 4.6.2 and prek (pinned), divan/criterion micro-benchmarks, peak-RSS via `/usr/bin/time -v`; CI regression gate on hyperfine medians.

---

## 2. Anatomy of pre-commit's own test suite

### 2.1 Runner configuration

- `tox.ini`: `coverage run -m pytest {posargs:tests} --ignore=tests/languages --durations=20`; `coverage report --omit=pre_commit/languages/*,tests/languages/*`. **Language tests are excluded from the default run** and executed per language in CI.
- `.github/workflows/languages.yaml`: matrix computed by `testing/languages` (runs only languages whose imported modules changed vs `origin/main`; `--all` on push). Excluded combos (`testing/languages:11-16`): windows×{docker, docker_image, lua, swift}. CI installs: coursier (`testing/get-coursier.sh`, v2.1.0-RC4, sha256-pinned), dart 2.19.6 (`testing/get-dart.sh`), `lua5.3 liblua5.3-dev luarocks` (apt), Strawberry perl on windows, `haskell-actions/setup@v2`, `r-lib/actions/setup-r@v2`; Python 3.10.
- pytest env (`tox.ini [pytest] env`, via pytest-env): `GIT_AUTHOR_NAME=test`, `GIT_COMMITTER_NAME=test`, `GIT_AUTHOR_EMAIL=test@example.com`, `GIT_COMMITTER_EMAIL=test@example.com`, `GIT_ALLOW_PROTOCOL=file`, `VIRTUALENV_NO_DOWNLOAD=1`, **`PRE_COMMIT_NO_CONCURRENCY=1`** (all of pre-commit's own tests run hooks with 1 job — snp must run its suites both with and without it).
- `requirements-dev.txt`: covdefaults, coverage, distlib, pytest, pytest-env, re-assert.

### 2.2 `tests/conftest.py` fixtures → snp testkit equivalents

| Fixture (line) | Behavior | snp equivalent |
|---|---|---|
| `tempdir_factory` (23-35) | numbered subdirs `0,1,2…` under pytest tmpdir | `TestEnv::mkdir()` returning `ChildPath` under one `TempDir` |
| `in_tmpdir` (38-42) | chdir into fresh dir | never chdir the test process; pass `current_dir` to commands |
| `in_git_dir` (45-50) | `<tmp>/repo`, `git init` | `TestEnv::git_repo("repo")` |
| `in_merge_conflict` (73-84) + `_make_conflict` (53-70) | `make_consuming_repo('script_hooks_repo')`, add empty `placeholder`, commit; clone; in clone: `checkout origin/master -b foo`; write `conflict_file`=`herp\nderp\n`, `foo_only_file`=`foo`, commit; `checkout origin/master -b bar`; `conflict_file`=`harp\nddrp\n`, `bar_only_file`=`bar`, commit; `git merge foo` (fails) | `Fixtures::merge_conflict()`; requires isolated git config (`merge.conflictstyle` from user config changes markers) |
| `in_conflicting_submodule` (87-95) | repo1 adds repo2 as submodule `sub`; conflict made inside `sub` | `Fixtures::conflicting_submodule()`; local-path `git submodule add` needs `protocol.file.allow=always` (git >= 2.38.1) — pre-commit's suite gets this via `GIT_ALLOW_PROTOCOL=file` |
| `commit_msg_repo` (98-115) | local hook `must-have-signoff`, name `Must have "Signed off by:"`, entry `grep -q "Signed off by:"`, `language: system`, `stages: [commit-msg]` | same YAML |
| `prepare_commit_msg_repo` (118-143) | local hook `add-signoff`, name `Add "Signed off by:"`, entry `./add_sign_off.sh` (`#!/usr/bin/env bash\nset -eu\necho "\nSigned off by: " >> "$1"\n`, executable), `language: script`, `stages: [prepare-commit-msg]` | same |
| `failing_prepare_commit_msg_repo` (146-163) | entry `bash -c "exit 1"`, `language: system`, stage prepare-commit-msg | same |
| `dont_write_to_home_directory` (166-180, session autouse) | `Store.get_default_directory` raises unless explicitly chosen | snp tests ALWAYS set `PRE_COMMIT_HOME` (and `SNP_HOME` if snp adds one) + `HOME` to temp; add a CI guard that fails if `~/.cache/pre-commit` or `~/.cache/snp` mtime changes during the test run |
| `configure_logging` (183-186) | logging without color | — |
| `mock_store_dir` (189-197) | default store dir → temp | env `PRE_COMMIT_HOME` |
| `store` (200-202) | `Store(<tmp>/.pre-commit)` | `PRE_COMMIT_HOME=<tmp>/.pre-commit` |
| `cap_out` (221-227) | captures `output.write`/`write_line_b` bytes, `\r\n`→`\n` | capture child stdout (snp writes all user output to stdout, see §6 item 1) |
| `set_git_templatedir` (230-234, session autouse) | `GIT_TEMPLATE_DIR=<empty dir>` so `git init` creates **no sample hooks** | testkit sets `GIT_TEMPLATE_DIR` to an empty dir for every git invocation |

### 2.3 `testing/` helpers (semantics to replicate in `snp-testkit`)

- `fixtures.py:39-42 git_dir` → `git -c init.defaultBranch=master init <path>` (branch name `master` is asserted in outputs like `[master abc1234] commit!`).
- `fixtures.py:45-50 make_repo(src)` → copy `testing/resources/<src>/*` into new git dir, `git add .`, commit msg `make_repo`.
- `fixtures.py:53-65 modify_manifest` / `68-80 modify_config` → YAML load, mutate, `yaml_dump` (PyYAML, `indent=4`, `default_flow_style=False`, `sort_keys=False`, `pre_commit/yaml.py:14-19`), commit (`modify_manifest`/`modify_config` messages).
- `fixtures.py:83-92 sample_local_config()` = `{'repo': 'local', 'hooks': [{'id': 'do_not_commit', 'name': 'Block if "DO NOT COMMIT" is found', 'entry': 'DO NOT COMMIT', 'language': 'pygrep'}]}`.
- `fixtures.py:95-96 sample_meta_config()` = `{'repo': 'meta', 'hooks': [{'id': 'check-useless-excludes'}]}`.
- `fixtures.py:99-113 make_config_from_repo(path, rev=None, hooks=None, check=True)` → `{'repo': f'file://{path}', 'rev': rev or HEAD sha, 'hooks': hooks or [{'id': h['id']} for h in manifest]}` (validated + defaults applied when `check`).
- `fixtures.py:123-128 write_config` wraps a single repo dict into `{'repos': [cfg]}`; `add_config_to_repo` writes + `git add` + commit (`add_config_to_repo`); `remove_config_from_repo` = `git rm` + commit; `make_consuming_repo(src)` = make_repo + new git dir with that config committed.
- `util.py:169-179 cmd_output_mocked_pre_commit_home` → subprocess with `PRE_COMMIT_HOME=<tmp>`, stderr merged into stdout, `\r\n`→`\n`.
- `util.py:185-234 run_opts` defaults: `all_files=False, files=(), color=False, verbose=False, hook=None, fail_fast=False, remote_branch='', local_branch='', from_ref='', to_ref='', pre_rebase_upstream='', pre_rebase_branch='', remote_name='', remote_url='', hook_stage='pre-commit', show_diff_on_failure=False, commit_msg_filename='', prepare_commit_message_source='', commit_object_name='', checkout_type='', is_squash_merge='', rewrite_command=''`; asserts not (`all_files` and `files`).
- `util.py:247-256 git_commit(*args, msg='commit!', all_files=True)` → `git commit --allow-empty --no-gpg-sign [-a] -m <msg> [args]`; stderr merged.
- `language_helpers.py:279-309 run_language(path, language, exe, args=(), file_args=(), version=None, deps=(), is_local=False, require_serial=True, color=False)` → install env if `ENVIRONMENT_DIR` set, `health_check` must return None, run in env, `\r\n`→`\n`. **Note default `require_serial=True`** (no shuffle) in language tests.
- `testing/make-archives`: builds deterministic `rbenv.tar.gz`, `ruby-build.tar.gz`, `ruby-download.tar.gz` (uid/gid 0, uname root, mtime 0, sorted members, gzip mtime 0) from pinned commits (`rbenv 10e96bfc`, `ruby-build 447468b1`, `rvm-download 09bd7c6`); `ruby_test.py:38-46` asserts the archive root dir mode is `0o755`. Only relevant if snp vendors rbenv.
- `testing/zipapp/`: packaging of pre-commit as a zipapp — irrelevant to snp.

---

## 3. Fixture repos in `testing/resources/` (recreate verbatim)

All `*.sh` / entry scripts are mode **0775** and must be committed as `100755`. Manifests are `.pre-commit-hooks.yaml`. Recreate under `crates/snp-testkit/resources/<name>/` and expose `Fixtures::make_repo("<name>")` (copy, `git add .`, `git update-index --chmod=+x` for scripts on Windows, commit `make_repo`).

| Repo | Manifest (exact) | Script(s) (exact) | Purpose / used by |
|---|---|---|---|
| `script_hooks_repo` | `- id: bash_hook`, `name: Bash hook`, `entry: bin/hook.sh`, `language: script`, `files: ''` | `bin/hook.sh`: `#!/usr/bin/env bash` / `echo "$@"` / `echo 'Hello World'` | the canonical passing hook; most run/install tests; output `foo.py\nHello World` |
| `failing_hook_repo` | `id: failing_hook`, `name: Failing hook`, `entry: bin/hook.sh`, `language: script`, `files: .` | `echo 'Fail'` / `echo "$@"` / `exit 1` | failure output `Fail\n<files>` |
| `arg_per_line_hooks_repo` | `id: arg-per-line`, `name: Args per line hook`, `entry: bin/hook.sh`, `language: script`, `files: ''`, `args: [hello, world]` | `for i in "$@"; do echo "arg: $i"; done` | argv fidelity (spaces, `{}` braces) |
| `arbitrary_bytes_repo` | `id: hook`, `name: hook`, `entry: ./hook.sh`, `language: script`, `files: \.py$` | `hook.sh` (repo root): `echo -e '\xe2\x98\x83\xb2'` then `exit 1` | non-UTF-8 hook output must pass through raw: `b'\xe2\x98\x83\xb2\n'` |
| `exclude_types_repo` | `id: python-files`, `name: Python files`, `entry: bin/hook.sh`, `language: script`, `types: [python]`, `exclude_types: [python3]` | `echo "$@"` / `exit 1` | `exclude_types` |
| `types_repo` | `id: python-files`, `name: Python files`, `entry: bin/hook.sh`, `language: script`, `types: [python]` | `echo "$@"` / `exit 1` | `types` |
| `types_or_repo` | `id: python-cython-files`, `name: Python and Cython files`, `entry: bin/hook.sh`, `language: script`, `types: [file]`, `types_or: [python, cython]` | `echo "$@"` / `exit 1` | `types_or` (`.pxd` is cython) |
| `logfile_repo` | `id: logfile test hook` (id with spaces!), `name: Logfile test hook`, `entry: bin/hook.sh`, `language: script`, `files: .`, `log_file: test.log` | `echo "This is STDOUT output"` / `echo "This is STDERR output" 1>&2` / `exit 1` | `log_file` + stderr merged into output |
| `modified_file_returns_zero_repo` | 3 hooks all `name: Bash hook`, `language: script`: `bash_hook` (`bin/hook.sh`, `files: 'foo.py'`), `bash_hook2` (`bin/hook2.sh`, `files: ''`), `bash_hook3` (`bin/hook3.sh`, `files: 'bar.py'`) | `hook.sh`: `for f in $@; do echo -e '\x01\x97' > "$f"; echo "Modified: $f!"; done`; `hook2.sh`: `echo "$@"`; `hook3.sh`: like hook.sh without echo | "files were modified by this hook" detection with exit 0; also used by try-repo |
| `not_found_exe` | `id: not-found-exe`, `name: Not found exe`, `entry: i-dont-exist-lol`, `language: system`, `files: ''` | — | `Executable \`i-dont-exist-lol\` not found`, ret 1 |
| `prints_cwd_repo` | `id: prints_cwd`, `name: Prints Cwd`, `entry: pwd`, `language: system`, `files: \.sh$` | — | hook cwd == repo root (`pwd -L`) |
| `python_hooks_repo` | `id: foo`, `name: Foo`, `entry: foo`, `language: python`, `files: \.py$` | `foo.py`: `def main(): print(repr(sys.argv[1:])); print('Hello World'); return 0`; `setup.py`: `setup(name='foo', version='0.0.0', py_modules=['foo'], entry_points={'console_scripts': ['foo = foo:main']})` | python env install (needs python+virtualenv; `pip install .` with build isolation fetches setuptools → **network** unless a local wheelhouse is provided via `PIP_NO_INDEX=1` + `PIP_FIND_LINKS=<dir>`; recommended for hermetic `tc_python_` tests) |
| `stdout_stderr_repo` | `stdout-stderr` (`./stdout-stderr-entry`) and `tty-check` (`./tty-check-entry`), both `language: script` | `stdout-stderr-entry`: `echo 0; echo 1 1>&2; echo 2; echo 3 1>&2; echo 4; echo 5 1>&2`; `tty-check-entry`: prints `stdin: True/False`, `stdout: …`, `stderr: …` via `[ -t N ]` | ordering of interleaved stdout/stderr (`0\n1\n2\n3\n4\n5\n` — requires single pipe for both), pty with color (`stdin: False\nstdout: True\nstderr: True\n`) |
| `img1.jpg`, `img2.jpg`, `img3.jpg` | — | 16×16 JFIF JPEGs, md5 `98db9bca0b042ad6912e96b2487027b2`, `bd8b9a9817f35c4644dd33716e4e7e16`, `6e414b11dd5f9614700131929e6adee8` | binary stash/restore tests; any 3 distinct small binary files work |

---

## 4. Conventions for the catalog below

- IDs: `CFG` config/clientlib, `RUN` run, `SFO` staged_files_only, `XRG` xargs/lang_base, `INS` install/uninstall/init-templatedir, `HKI` hook-impl, `GIT` git helpers, `STO` store, `REP` repository, `LNG` languages, `AUP` autoupdate, `MIG` migrate-config & yaml_rewrite, `TRY` try-repo, `GC` gc/clean, `MET` meta hooks, `OUT` output/color/logging, `ERR` error handler, `CLI` main/argparse, `HAZ` hazmat, `VAL` validate/sample-config, `UTL` util/parse_shebang/envcontext/prefix.
- "Needs": `—` hermetic; `posix` (xfail on Windows in pre-commit); `win` (Windows only); `net`; `tc:<tool>`; `pty`.
- Port level: **I** = integration test through the snp binary (preferred); **U** = unit test of the equivalent Rust function; **D** = also a differential scenario.
- Expected strings are verbatim; `\n` shown explicitly where it matters.

---

## 5. Behavior catalog & test cases to port

### 5.1 Config / manifest validation (`tests/clientlib_test.py`, `tests/commands/validate_*_test.py`)

| ID | Source | Setup | Action | Expected | Needs / port |
|---|---|---|---|---|---|
| CFG-01 | clientlib_test.py:34-41 | — | type tag check | `definitely-not-a-tag`, `fiel` → validation error; `file` ok | — U |
| CFG-02 | :44-70 | repo `git@github.com:pre-commit/pre-commit-hooks`, rev sha, hook `{id: pyflakes, files: '\.py$'}` (+`args: [foo,bar,baz]`) | validate | valid | — U,D |
| CFG-03 | :73-89 | hook `exclude: 0` | validate | invalid (`=====> Expected string got int`) | — U,D |
| CFG-04 | :92-95 | `repo: local` + `rev: foo` | validate | invalid | — U,D |
| CFG-05 | :98-100 | `sample_local_config()` | validate | valid | — U |
| CFG-06 | :103-108 | — | config-hook schema items | **no defaults** in config hook schema (defaults would clobber manifest values, #227) → snp: every override field is `Option<T>` | — U (type-level) |
| CFG-07 | :111-122 | `ci: {skip: [foo]}` + meta identity | validate | valid, **no warning**; `ci: invalid` (string) → error | — U,D |
| CFG-08 | :125-176 | remote repo `https://gitlab.com/pycqa/flake8` with rev | validate | no warning for `v0.12.4`, `b27f281`, `b27f281eb9398fc8504415d7fbdabf119ea8c5e1`, `19.10b0`, `4.3.21-2`; WARNING for `''`, `HEAD`, `stable`, `master`, `some_branch_name`: ``The 'rev' field of repo 'https://gitlab.com/pycqa/flake8' appears to be a mutable reference (moving tag / branch).  Mutable references are never updated after first install and are not supported.  See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.  Hint: `pre-commit autoupdate` often fixes this.`` | — U,D |
| CFG-09 | :179-187 | `repo: meta` + `rev: 3.7.7` | validate | error | — U |
| CFG-10 | :190-207 | `files: None` | sensible-regex validator | error text exactly `"\n==> At key: files\n=====> Expected string got NoneType"` (YAML `files: ~` / `files:`) | — U,D |
| CFG-11 | :210-276 | hook `{id: flake8, files: <re>}` | validate | `dir/*.py` → ``The 'files' field in hook 'flake8' is a regex, not a glob -- matching '/*' probably isn't what you want here``; `dir[\/].*\.py` → ``pre-commit normalizes slashes in the 'files' field in hook 'flake8' to forward slashes, so you can use / instead of [\/]`` (same for `[/\\]`, `[\\/]`, bracket text echoed verbatim). Same for local hook (`do_not_commit`) and meta hook (`identity`); applies to `exclude` too | — U,D |
| CFG-12 | :279-311 | top-level `files: <re>`, `repos: []` | validate | ``The top-level 'files' field is a regex, not a glob -- matching '/*' probably isn't what you want here``; ``pre-commit normalizes the slashes in the top-level 'files' field to forward slashes, so you can use / instead of [\/]`` | — U,D |
| CFG-13 | :314-332 | local hook `stages: [invalid]` | validate | exactly: `\n==> At Config()\n==> At key: repos\n==> At Repository(repo='local')\n==> At key: hooks\n==> At Hook(id='do_not_commit')\n==> At key: stages\n==> At index 0\n=====> Expected one of commit-msg, manual, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg but got: 'invalid'` (choices sorted) | — U,D (>=4.4) |
| CFG-14 | :335-382 | `stages: [commit, push]` (hook) / `default_stages: [commit, push]` | validate | WARNING ``hook id `do_not_commit` uses deprecated stage names (commit, push) which will be removed in a future version.  run: `pre-commit migrate-config` to automatically fix this.``; top-level: ``top-level `default_stages` uses deprecated stage names (commit, push) which will be removed in a future version.  run: `pre-commit migrate-config` to automatically fix this.``; none for `pre-commit, pre-push` | — U,D |
| CFG-15 | :385-402 | local hooks `language: system` / `script` | validate+defaults | normalized to `unsupported` / `unsupported_script`; missing `language` in local hook → error | — U (>=4.4) |
| CFG-16 | :405-435 | manifests incl. `language_version: python3.4`, `files: ''` + `always_run: true` | validate | valid | — U |
| CFG-17 | :438-458 | meta: `id: i-dont-exist`; `identity` + `language: python`; `name: False`; `entry: echo hi` | validate | all errors; entry → `'entry' cannot be overridden` (`clientlib.py` `NotAllowed`) | — U,D |
| CFG-18 | :461-467 | meta `check-hooks-apply` defaults | regex | `files` = `^\.pre\-commit\-config\.yaml$` (Python `re.escape` escapes `-`); matches `.pre-commit-config.yaml`, not `foo/.pre-commit-config.yaml` | — U |
| CFG-19 | :470-481 | `default_language_version: {pony: '1.0'}` / `{python: 3}` | validate | errors (unknown language key; non-string) | — U,D |
| CFG-20 | :484-487 | — | `parse_version` | `0.1 > 0.0`, `2.1 >= 2` | — U |
| CFG-21 | :490-536 | `minimum_pre_commit_version: '999'` top-level / in hook / with `repos: 5` | validate | ``\n==> At Config()\n==> At key: minimum_pre_commit_version\n=====> pre-commit version 999 is required but version {VERSION} is installed.  Perhaps run `pip install --upgrade pre-commit`.`` — **checked before any other key** (repos: 5 still yields this). Hook-level variant nests `==> At key: repos / At Repository(repo='local') / At key: hooks / At Hook(id='do_not_commit') / At key: minimum_pre_commit_version`. `'0'` passes | — U,D (VERSION is an open question §10) |
| CFG-22 | :539-545 | — | WarnAdditionalKeys | allowed-key list == schema keys; warnings: `Unexpected key(s) present at root: foo` (validate_config_test.py:275-294), `Unexpected key(s) present on https://gitlab.com/pycqa/flake8: args` (:253-272), local hooks: `Unexpected key(s) present on local => too-much: foo, hello` (repository_test.py:201-215; keys sorted, `, `-joined) | — U,D |
| CFG-23 | :548-592 | stages `[commit-msg, push, commit, merge-commit]` in `default_stages`, manifest hook, config hook | defaults | → `[commit-msg, pre-push, pre-commit, pre-merge-commit]` (order kept); config hook without `stages` stays without the key | — U |
| CFG-24 | :595-607 | manifest file `hooks: {}` (mapping, not list) | `load_manifest` | ``\n==> File {path}\n=====> \n=====> pre-commit version 5 is required but version {VERSION} is installed.  Perhaps run `pip install --upgrade pre-commit`.`` (observed same via `validate-manifest` on a config-shaped file) | — U,D |
| CFG-25 | observed | `always_run: yes`, `pass_filenames: off` | validate-config | **valid** (YAML 1.1 booleans) | — D |
| CFG-26 | observed | remote repo `rev: 1.10` | validate-config | exit 1: `\n==> File <path>\n==> At Config()\n==> At key: repos\n==> At Repository(repo='https://example.com/r')\n==> At key: rev\n=====> Expected string got float` | — D |
| CFG-27 | observed | duplicate top-level key `repos: []` twice | validate-config | exit 0, silent (last wins) | — D |
| CFG-28 | observed | anchors + merge key `<<: *common` in hooks; unknown root key `x-common` | validate-config | exit 0; `[WARNING] Unexpected key(s) present at root: x-common` | — D |
| CFG-29 | observed | missing `name` in local hook | validate-config | `==> At Hook(id='x')\n=====> Missing required key: name` (full chain from `==> File`) | — D |
| VAL-01 | validate_config_test.py:249-250 | repo's own `.pre-commit-config.yaml` | validate-config | ret 0 | — |
| VAL-02 | :297-305 | `does-not-exist`, `{` (bad YAML), `{}` (no repos) | validate-config | each ret 1 | — D (Tier B body) |
| VAL-03 | validate_manifest_test.py:1-18 | same three | validate-manifest | ret 1 | — D |

### 5.2 `run` (`tests/commands/run_test.py`)

Output primitives (port as **U** with exact strings):

| ID | Source | Input | Expected |
|---|---|---|---|
| RUN-01 | :39-42 | `_start_msg(start='start', end_len=5, cols=15)` | `start....` (dots = cols − width(start) − end_len − 1) |
| RUN-02 | :45-104 | `_full_msg`, cols 15/20 | `start......end\n`; CJK `啊あ아` width 6 → `啊あ아.....end\n`; color → `start......\x1b[41mend\x1b[m\n`; postfix `post ` → `start......post end\n`; postfix never colored |
| RUN-03 | :576-591 | `_compute_cols` | `[]`→80; one name `'a'*51`→81; names 51 & 52 → 82. Formula `max(80, max_width + 3 + len('(no files to check)') + 1 + len('Skipped'))` (`run.py:238-250`) |
| RUN-04 | :594-608 | `SKIP` parsing | `{}`, `''`, `','` → ∅; `',foo'`→{foo}; `'foo,bar'`; `' foo , bar'`→{foo,bar} (split on `,`, strip, drop empty; `run.py:129-131`) |

End-to-end (port as **I**, most also **D**):

| ID | Source | Setup | Action | Expected |
|---|---|---|---|---|
| RUN-10 | :159-173 | consuming repo of `failing_hook_repo`, stage empty `foo.py` | `run` | ret 1; contains `Failing hook`, `Failed`, `hook id: failing_hook`, `Fail\nfoo.py\n` |
| RUN-11 | :176-181 | `arbitrary_bytes_repo`, stage `foo.py` | `run` | ret 1; raw bytes `\xe2\x98\x83\xb2\n` in stdout (no lossy decoding) |
| RUN-12 | :184-210 | `modified_file_returns_zero_repo`, stage `bar.py` and `foo.py` | `run` | ret 1; in order: hook1 `Failed` + `- files were modified by this hook\n\nModified: foo.py`, hook2 `Passed`, hook3 `Failed` + `- files were modified by this hook\n` (no output block) |
| RUN-13 | :213-248 | `types_repo` (`bar.py`, `bar.notpy`); `types_or_repo` (`bar.notpy`,`bar.pxd`,`bar.py`); `exclude_types_repo` (`exe` = `#!/usr/bin/env python3` +x, `bar.py`) | `run` | ret 1; only matching files appear: `bar.py` / `bar.pxd`+`bar.py` / `bar.py` not `exe` |
| RUN-14 | :251-282 | top-level `exclude: ^foo\.py$` (or `files: ^bar\.py$`) + meta `identity`; add `foo.py`,`bar.py` | `run --verbose` | ret 0; stdout starts `identity` + 65 dots + `Passed\n`; ends `\n\n.pre-commit-config.yaml\nbar.py\n\n` (resp. `\n\nbar.py\n\n`) |
| RUN-15 | :285-299 | meta identity | `run --verbose` with clock (1.234→2.0) / (1.0→1.0) | `\n- duration: 0.77s\n` / `\n- duration: 0s\n` (`round(dt,2) or 0`, `run.py:202`). snp: injectable clock (U) |
| RUN-16 | :302-346 + observed | `modified_file_returns_zero_repo`, stage `bar.py` | `run --show-diff-on-failure` (± `--color`, ± `--all-files`) | ret 1; `All changes made by hooks:` then `git --no-pager diff --no-ext-diff --color=<always\|never>` written **directly to fd 1** by git; with `--all-files` the preceding block is exactly `pre-commit hook(s) made changes.\nIf you are seeing this message in CI, reproduce locally with: \`pre-commit run --all-files\`.\nTo run \`pre-commit\` as part of git workflow, use \`pre-commit install\`.\n`. NB pre-commit's own test passes a bytes object as `expected_outputs` and thus only checks single bytes (weak test) — snp asserts the full text |
| RUN-17 | :349-399 | `script_hooks_repo` | `run` variants | `{}`→`Bash hook`…`Passed` ret 0; `--verbose`→`foo.py\nHello World`; `bash_hook`→Passed; `nope`→ret 1 ``No hook with id `nope` in stage `pre-commit` `` (stage name follows `--hook-stage`, e.g. `pre-push`); `--all-files --verbose`/`--files foo.py --verbose`→contains `foo.py`; nothing staged → `Bash hook`, `(no files to check)`, `Skipped`, ret 0 |
| RUN-18 | :402-424 | `logfile_repo`, stage foo.py | `run` | ret 1; stdout contains `This is STDOUT output\n` and `This is STDERR output\n`; `test.log` created in repo root containing those lines (append mode `ab`; only the stripped hook output + `\n` is written, not the surrounding blank lines; `output.py:14-29`) |
| RUN-19 | :427-473 | `always_run: true`, nothing staged; alt config `-c alternate_config.yaml`; `verbose: true` hook | `run` | `Passed` ret 0; hook-level `verbose` prints `Hello World` |
| RUN-20 | :476-497 | — | `run --from-ref master` (or only `--to-ref`) | ret 1, stdout `[ERROR] Specify both --from-ref and --to-ref.\n`; with all push args set → ret 0, no error |
| RUN-21 | :500-525,566-573,1242-1248 | hook printing env (probe `env`) | run with `--is-squash-merge 1` / `--rewrite-command amend` / `--checkout-type 1` / `--pre-rebase-upstream master --pre-rebase-branch topic` | hook env has `PRE_COMMIT_IS_SQUASH_MERGE=1`, `PRE_COMMIT_REWRITE_COMMAND=amend`, `PRE_COMMIT_CHECKOUT_TYPE=1`, `PRE_COMMIT_PRE_REBASE_UPSTREAM=master` & `PRE_COMMIT_PRE_REBASE_BRANCH=topic` (only when both), always `PRE_COMMIT=1`; with from/to: `PRE_COMMIT_FROM_REF`, `PRE_COMMIT_TO_REF`, legacy `PRE_COMMIT_ORIGIN`(=from), `PRE_COMMIT_SOURCE`(=to); `PRE_COMMIT_COMMIT_MSG_SOURCE`, `PRE_COMMIT_COMMIT_OBJECT_NAME`; push vars `PRE_COMMIT_LOCAL_BRANCH/REMOTE_BRANCH/REMOTE_NAME/REMOTE_URL` only when all four given (`run.py:371-417`) |
| RUN-22 | :527-563 | `in_merge_conflict` | `run` | ret 1 `[ERROR] Unmerged files.  Resolve before committing.` (two spaces); `--files placeholder` → ret 0, `Bash hook` runs; with extra unstaged change still ret 1 same error; after `git add .` → `[INFO] Checking merge-conflict files only.` then `Bash hook`…`Passed` (`git.py:115`) |
| RUN-23 | :611-678 | `SKIP=bash_hook`; aliased repo `SKIP=foo_bash` + `run foo_bash`; local python hook with `additional_dependencies: ['/pre-commit-does-not-exist']` + `SKIP=skipme` (or alias `skipme-1`) | `run` / `run --all-files` | `Bash hook`…`Skipped` (yellow); alias: exactly one `Bash hook` and one `Skipped`; skipped hook's env is **never installed** → ret 0 |
| RUN-24 | :681-694 | — | `run` / `run --verbose` | `[bash_hook]` never printed; verbose prints `- hook id: bash_hook` |
| RUN-25 | :697-724 | duplicate `{id: bash_hook}` entry; aliased (`{id: bash_hook, alias: foo_bash}`) | `run` / `run bash_hook` / `run foo_bash` | dup → 2× `Bash hook`; `run bash_hook` matches both id and alias entries (2); `run foo_bash` → 1 |
| RUN-26 | :727-754 | — | `run ☃`; hook `args: ['☃']` via `git commit` | no encode/decode errors (snp: no panics, args passed as OsString) |
| RUN-27 | :757-776, repository_test.py:159-163 | 400 files named `'a'*100 + str(i)`; hook `files: ''` | `git commit` with installed hook | ret 0 (xargs partitioning; Windows command-line limit) ; 15000 `os.devnull` args to a script hook succeed |
| RUN-28 | :779-817 | `.gitattributes` `*.jpeg diff=empty`; `.git/config` `[diff "empty"] textconv = "true"`; local system hook (types `[jpeg]`) appending `\x00` to the file | `run` | ret 1 `Failed` — modification detection must use `git diff --no-ext-diff --no-textconv --ignore-submodules` (`run.py:274-279`) |
| RUN-29 | :820-848 | 3 pygrep hooks `hook 1..3` with stages pre-commit / pre-push / manual | `run --hook-stage <s>` | exactly one `hook ` line; output starts `hook 1...`/`hook 2...`/`hook 3...` |
| RUN-30 | :851-909 | `commit_msg_repo`, `.git/COMMIT_EDITMSG`=`This is the commit message`; `post-checkout` identity; `prepare_commit_msg_repo` | `run --hook-stage commit-msg --commit-msg-filename .git/COMMIT_EDITMSG`; `--hook-stage post-checkout`; `--hook-stage prepare-commit-msg --commit-msg-filename … --prepare-commit-message-source commit --commit-object-name HEAD` | `Must have "Signed off by:"`…`Failed` ret 1; `identity...` ret 0; `Add "Signed off by:"`…`Passed` and file contains `Signed off by: ` |
| RUN-31 | :912-988 | local hooks: system `identity-copy` (`files: \.py$`) + pygrep `DO NOT COMMIT`; failing `sh -c "! grep -iI todo $@" --`; meta `check-useless-excludes` | `run` | ret 0 / ret 1 / `Check for useless excludes` ret 0 |
| RUN-32 | :991-1023 | config modified but not staged | `run` | ret 1 stdout `[ERROR] Your pre-commit configuration is unstaged.\n\`git add .pre-commit-config.yaml\` to fix this.\n`; **not** emitted with `--all-files` or `--files .pre-commit-config.yaml` (no stash mode) |
| RUN-33 | :1006-1013 | — | `run --hook-stage commit-msg` (no filename) | ret 1, stdout exactly ``[ERROR] `--commit-msg-filename` is required for `--hook-stage commit-msg`\n`` |
| RUN-34 | :1026-1040 | `subdir/foo.py` staged | from `subdir/`: `run -v --files foo.py` | output contains `subdir/foo.py` (args made relative to toplevel before chdir; `main._adjust_args_and_chdir`) |
| RUN-35 | :1043-1064 | `pass_filenames` × `args` | `run --verbose` | (True,[]):`foo.py\nHello World`; (False,[]):`\nHello World`; (True,[some,args]):`some args foo.py`; (False,[some,args]):`some args`; `foo.py` present iff pass_filenames |
| RUN-36 | :1067-1117 | failing hook ×2 | top-level `fail_fast: true`; hook-level `fail_fast` on first; meta `identity` with `fail_fast` (passing) then `identity` named `run me!`; CLI `--fail-fast` | exactly one `Failing hook` (stop after first *failing* hook whose own/global/CLI fail_fast is set); a passing fail_fast hook does not stop the run (`run me!` printed once). `--fail-fast` needs >= 4.4 |
| RUN-37 | :1120-1160 | Classifier | — | nonexistent files dropped (`os.path.lexists`, so **broken symlinks are kept**); Windows: `a\b\c` → `a/b/c`; POSIX: backslash kept (`a/b\c`); symlink `foo→bar`: `types=[symlink]` → (`foo`,), `types=[file]` → (`bar`,) |
| RUN-38 | :1163-1200 | names `.pre-commit-hooks.yaml`, `pre_commit/git.py`, `pre_commit/main.py` | `filter_by_include_exclude(names, include, exclude)` | ('', '^$') → all; broken symlink `link` matches; `^.*\.py$` → 2 py; `\.yaml$` → **search, not match**; exclude `\.py$` → yaml only |
| RUN-39 | :1203-1233 | 2 local hooks | `run do_not_commit` | `identity-copy` not printed |
| RUN-40 | :1236-1239 | env `_PRE_COMMIT_SKIP_POST_CHECKOUT=1`, **no config file** | `run --hook-stage post-checkout` | ret 0, nothing printed (checked before config load, `run.py:366-372`) |

Additional observed run behaviors (4.6.2) — add as **D** scenarios:

| ID | Setup | Action | Expected |
|---|---|---|---|
| RUN-50 | local hooks pass/fail(`sh -c 'echo out; echo err >&2; exit 3' --`)/no-match(`files: \.nomatch$`) + meta identity; 2 files staged | `run` | stdout exactly: `passing hook.....Passed` (dots to col 80), `failing hook.....Failed`, `- hook id: fail`, `- exit code: 3`, ``, `out`, `err`, ``, `nothing matches......................................(no files to check)Skipped`, `identity…Passed`, `- hook id: identity`, `- duration: 0.03s`, ``, `.pre-commit-config.yaml`, `a.txt`, `b.py`, `` ; **stderr empty**; exit 1 |
| RUN-51 | same | `run --color always` | `\x1b[42mPassed\x1b[m`, `\x1b[41mFailed\x1b[m`, `\x1b[46;30mSkipped\x1b[m` (no-files), SKIP → `\x1b[43;30mSkipped\x1b[m`, subtle lines `\x1b[2m- hook id: fail\x1b[m`; hook subprocess runs under a **pty** (stderr+stdout merged) |
| RUN-52 | same | `SKIP=fail,identity run` | `failing hook……Skipped` (dots computed with `Skipped`), identity `Skipped` **followed by `- hook id: identity`** (identity has `verbose: true`; skipped hooks still print the subtle block when verbose) |
| RUN-53 | same | `run --files does-not-exist.txt` | every hook `(no files to check)Skipped`, ret 0 |
| RUN-54 | same | `run --color=never --files a.txt pass` | argparse `--files` is `nargs='*'` and **swallows `pass`** → all hooks run on `a.txt` (no hook filter). snp CLI must reproduce |
| RUN-55 | `repos: []` | `run`, `run -a` | exit 0, no output |
| RUN-56 | no config file | `run` | exit 1, stdout `An error has occurred: InvalidConfigError: \n=====> .pre-commit-config.yaml is not a file\nCheck the log at <PRE_COMMIT_HOME>/pre-commit.log\n` (note trailing space after `InvalidConfigError:`) |
| RUN-57 | outside git | `run` | exit 1, `An error has occurred: FatalError: git failed. Is it installed, and are you in a Git repository directory?\nCheck the log at …/pre-commit.log` |
| RUN-58 | staged `a.txt` + unstaged change on same file; fixer hook appends `fixed` | `run --show-diff-on-failure` | `[WARNING] Unstaged files detected.`, `[INFO] Stashing unstaged files to <HOME>/patch<epoch>-<pid>.`, hook Failed + `- files were modified by this hook`, `All changes made by hooks:` + diff, `[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...`, `[INFO] Restored changes from <HOME>/patch<epoch>-<pid>.`; worktree restored to pre-run unstaged content; **patch file left in PRE_COMMIT_HOME** |
| RUN-59 | invalid `PRE_COMMIT_COLOR=bogus` | `run` | exit 2; argparse usage on stderr + `pre-commit run: error: argument --color: invalid use_color value: 'bogus'` (argparse applies `type=` to env-derived default) |
| RUN-60 | — | `pre-commit` with **no subcommand** | behaves like `run` (observed exit 1 running hooks) |

### 5.3 staged-files-only / stash (`tests/staged_files_only_test.py`)

Setup unless stated: `in_git_dir`, `foo` = `"1\n2\n…\n8\n"` staged (`FOO_CONTENTS`, :23). `status` = `git status -s` code for the file. Port as **I** by running `snp run` with a probe hook that asserts/prints the worktree content during the hook and optionally mutates it (probe `cat foo`, `write foo …`), then checking worktree+index after. Also **U** against the Rust stash module.

| ID | Source | Scenario | During hook | After |
|---|---|---|---|---|
| SFO-01 | :58-65 | nothing unstaged | content = staged, `A` | unchanged, `A` |
| SFO-02 | :68-77 | unstaged `herp\nderp\n` | staged content, `A` | `herp\nderp\n`, `AM` |
| SFO-03 | :80-86 | patch dir missing (deleted) | — | no crash; dir created (`os.makedirs(..., exist_ok=True)`, `staged_files_only.py:73`) |
| SFO-04 | :89-98 | `git config diff.external <script>`; `color.diff always` | same as SFO-02 | same — diff must use `--no-ext-diff` and `--no-color` / binary-safe flags |
| SFO-05 | :101-116 | unstaged adds line 9; hook rewrites `1`→`a` (non-conflicting) | staged content | `FOO.replace('1','a') + '9\n'`, `AM` (hook fix + user change merged) |
| SFO-06 | :119-134 | unstaged `1`→`a`; hook `1`→`b` (conflict) | staged | `1`→`a` (hook fixes rolled back), `AM` |
| SFO-07 | :137-186 | binary `img.jpg`: none unstaged / unstaged img2 / conflict img3 during hook | img1 | img1 `A` / img2 `AM` / img2 `AM` (binary patches: `git diff --binary`) |
| SFO-08 | :189-253 | submodule `sub` staged at rev1; unstaged checkout rev2 | **sub untouched** (stays rev2, `AM`) | rev2 `AM` — submodule changes are never stashed (`--ignore-submodules`) |
| SFO-09 | :256-281 | unstaged change to `bar` + uncommitted file inside submodule; also with `submodule.recurse=1` | `bar` == `''`, submodule file kept | both restored |
| SFO-10 | :284-304 | unstaged `☃` (UTF-8) / `ú` latin-1 | staged | restored byte-exact |
| SFO-11 | :307-329 | latin-1 `ú \n` (trailing ws) then hook truncates file → patch fails to apply | staged | original latin-1 content restored; `git apply` stderr with invalid UTF-8 must not crash |
| SFO-12 | :345-358 | matrix `core.autocrlf` ∈ {true,false,input} × crlf_before × crlf_after; staged `1\n2\n`, worktree `3\n4\n\n` | `git write-tree` then `git diff-index <tree> --exit-code` == 0 (worktree matches index) | — |
| SFO-13 | :361-373 | autocrlf true/input; staged `1\r\n2\r\n3\r\n`, worktree `1\n2\n3\n` | no crash (git reports diff exit 1 but empty patch → treat as no diff) | — |
| SFO-14 | :376-378 | `apply.whitespace=error` + crlf case | works (`git apply --whitespace=nowarn`, `staged_files_only.py:26-27`) | — |
| SFO-15 | :381-392 | committed CRLF with autocrlf=false, then autocrlf=true + trailing CRLF lines | worktree matches index | — |
| SFO-16 | :395-403 | `git add --intent-to-add foo` | no diff; `[WARNING] Unstaged intent-to-add files detected.` (`staged_files_only.py:39`) | still intent-to-add |
| SFO-17 | :406-449 | 3 staged files modified; file `1` chmod 000 (posix) | — | snp exits with error ``pre-commit failed to diff -- perhaps due to permissions?\n\ncommand: …\nreturn code: 128\nstdout: (none)\nstderr:\n    error: open("1"): Permission denied\n    fatal: cannot hash 1`` and **all unstaged changes still present** |

Also from install tests: INS-12 (`commit -am` with unstaged file, #322) and INS-31 (post-checkout + stash recursion guard).

### 5.4 xargs / parallelism / lang_base (`tests/xargs_test.py`, `tests/lang_base_test.py`)

Algorithm (normative, `xargs.py:41-117`, `lang_base.py:135-171`):
- `cpu_count()` = `len(sched_getaffinity(0))`, else `multiprocessing.cpu_count()`, else 1.
- `target_concurrency()` = 1 if `PRE_COMMIT_NO_CONCURRENCY` set (any value), else 2 if `TRAVIS` set, else `cpu_count()`.
- `run_xargs`: `require_serial` → jobs 1, **no shuffle**; else `file_args = _shuffled(file_args)` with `random.Random(); seed(1542676187, version=1); shuffle(list)` and jobs = target_concurrency.
- `_environ_size(env)` = `8*len(env) + Σ(len(k)+len(v)+2)` (bytes).
- `_get_platform_max_length()`: posix → `clamp(SC_ARG_MAX − 2048 − _environ_size(), 2**12, 2**17)`; nt → `2**15 − 2048`; other → `2**12`. Evaluated **once at import** as the default `_max_length` of `xargs()`.
- `_command_length(*cmd)`: `' '.join(cmd)` encoded in fs encoding (bytes) on posix; UTF-16 code units on win32.
- `partition(cmd, varargs, target_concurrency, _max_length)`: `max_args = max(4, ceil(len(varargs)/target_concurrency))`; greedy fill while `total + len(arg)+1 <= max_length and len(cur) < max_args`; empty-partition overflow → `ArgumentTooLongError(arg)`; always returns ≥1 partition (`(cmd,)` for no varargs).
- Windows `.bat/.cmd` (case-insensitive): `max_length = 8192 − len(cmd.exe path) − len(' /c ') − 1024`.
- `xargs()`: `threads = min(len(partitions), target_concurrency)`; results **in partition order**; retcode = the one with the largest `abs()` (first wins on ties); stdout = concatenation (stderr merged into stdout per partition, `stderr=STDOUT`); `color=True` → pty per partition.

| ID | Source | Input | Expected |
|---|---|---|---|
| XRG-01 | xargs_test.py:16-47 | affinity 345 / no affinity & cpu_count 123 / NotImplemented | 345 / 123 / 1 |
| XRG-02 | :50-61 | env `{}`, `{x:1}`, `{x:12}`, `{x:1,y:2}` | 0, 12, 13, 24 |
| XRG-03 | :78-83 | `partition(('cmd',),(),1)` / `(('cmd',),('foo',),1)` | `(('cmd',),)` / `(('cmd','foo'),)` |
| XRG-04 | :86-105 | cmd `ninechars`, args `.....`, `....`, `..........`, `.....`, `......`, max 21 | `(ninechars,.....,....)`, `(ninechars,..........)`, `(ninechars,.....)`, `(ninechars,......)` |
| XRG-05 | :108-127 | `😑`×5 win32 max 21 → 1 partition (UTF-16: 2 units each); linux max 31 → 1 partition (4 bytes each); `😑`×10 linux max 20 → ArgumentTooLong | — |
| XRG-06 | :130-154 | `('foo',)`, 22×`A`, concurrency 4, max 50 → 6,6,6,4; 10×`A` → 4,4,2 | — |
| XRG-07 | :157-159 | `partition(('aaaaa',),('aaaaa',),1,_max_length=10)` | ArgumentTooLong |
| XRG-08 | :162-165 | `xargs(('echo',),('hello','world'))` | (0, `hello world\n`) |
| XRG-09 | :168-182 | `bash -c 'exit $1' --` with max_length = len(cmd)+3 (one arg per partition): args `0` / `0,1` / `0,5,1` | 0 / 1 / 5 (max abs) |
| XRG-10 | :185-191 | `bash -c 'kill -9 $$' --` | -9 (posix). snp: map signal termination to `-signum` like Python's `returncode` (`ExitStatusExt::signal`) |
| XRG-11 | :194-210 | 5× `sleep 0.5 && echo $$` concurrency 5, one per partition | 5 distinct lines, elapsed < 2.5 s |
| XRG-12 | :224-231 | env propagation `PRE_COMMIT_TEST_VAR` | output contains value |
| XRG-13 | :234-242 | color=True | child sees stdout as tty (`True\n`) (pty) |
| XRG-14 | :245-251 | win: `t.bat`/`t.cmd`/`T.CMD` with 8192 `x` args | ret 0 (win only) |
| XRG-15 | lang_base_test.py:125-135 | cpu 4; env `PRE_COMMIT_NO_CONCURRENCY`/`TRAVIS`/none | 1 / 2 / 4 |
| XRG-16 | :138-141 | `_shuffled([str(i) for i in range(10)])` | `['4','0','5','1','8','6','2','3','7','9']` |
| XRG-17 | :144-151 | `run_xargs(('echo',), 0..9, require_serial=True)` | `0 1 2 3 4 5 6 7 8 9` |
| XRG-18 | :154-166 | `basic_run_hook(prefix, 'echo hi', ['hello'], ['file']*3)` | (0, `hi hello file file file\n`) |
| XRG-19 | :169-178 | `hook_cmd('echo hi', ())`; `hook_cmd('pre-commit hazmat cd a echo -- b', ())` | `('echo','hi')`; `(sys.executable,'-m','pre_commit.commands.hazmat','cd','a','echo','--','b')` → snp must intercept entries starting with `pre-commit hazmat` and run its own hazmat implementation in-process or via `current_exe() hazmat …` |
| XRG-20 | :33-62 | `exe_exists('ruby')`: not found → False; `/usr/bin/ruby` → True; `/foo/shims/ruby` → False (regex `[/\\]shims[/\\]`); under home dir → False; `commonpath` ValueError → True; home == `/` → True | — |
| XRG-21 | :73-82 | setup cmd writes `\x81\xfe` to stderr and exits 1 | error raised, no Unicode crash |
| XRG-22 | :85-108 | `environment_dir(prefix,'langenv','default')` → `<prefix>/langenv-default`; `assert_version_default('lang','1.2.3')` msg ``for now, pre-commit requires system-installed lang -- you selected `language_version: 1.2.3` ``; `assert_no_additional_deps('lang',['hmmm'])` msg ``for now, pre-commit does not support additional_dependencies for lang -- you selected `additional_dependencies: ['hmmm']` `` | — |

Python shuffle test vectors (seed 1542676187, generated this session with CPython 3.12 `random`, indices of `range(n)`):

| n | shuffled |
|---|---|
| 1 | [0] |
| 2 | [0, 1] |
| 3 | [0, 1, 2] |
| 4 | [1, 0, 2, 3] |
| 5 | [1, 0, 2, 3, 4] |
| 6 | [0, 2, 5, 1, 3, 4] |
| 7 | [0, 2, 5, 1, 6, 3, 4] |
| 8 | [0, 5, 3, 6, 2, 1, 4, 7] |
| 10 | [4, 0, 5, 1, 8, 6, 2, 3, 7, 9] |
| 16 | [14, 6, 1, 4, 0, 8, 11, 2, 13, 7, 10, 12, 5, 3, 9, 15] |
| 20 | [4, 9, 5, 14, 0, 13, 12, 11, 1, 17, 2, 8, 3, 7, 19, 16, 10, 6, 15, 18] |

First 8 `getrandbits(32)` after seeding: `2472326011, 2061721674, 2625518588, 881996946, 1343846257, 1419225438, 2810596541, 3862474281`. Algorithm: MT19937 `init_by_array([1542676187])`; `shuffle`: `for i in reversed(range(1, n)): j = randbelow(i+1); swap(i, j)`; `randbelow(m)`: `k = m.bit_length(); r = getrandbits(k); while r >= m: r = getrandbits(k)`; `getrandbits(k<=32)` = `genrand_uint32() >> (32-k)`; for `k>32` Python concatenates 32-bit words little-endian (only for >4·10⁹ files; implement anyway or cap).

### 5.5 install / uninstall / install-hooks / init-templatedir (`tests/commands/install_uninstall_test.py`, `init_templatedir_test.py`)

Key constants (`install_uninstall.py:23-32`): CURRENT_HASH `138fd403232d2ddd5efb44317e38bf03`; PRIOR_HASHES `4d9958c90bc262f47553e2c073f14cfe`, `d8ee923c46731b42cd95cc869add4062`, `49fd668cb42069aa1b6048464be5d395`, `79f09a650522a87b0da915d0d983b2de`, `e358c9dae00eac5d06b38dfdb1e33a8c`. "Ours" = file exists and **contains any hash as bytes** (`is_our_script`, :56-61). Template markers `# start templated\n` / `# end templated\n`. Installed file observed (4.6.2):

```bash
#!/usr/bin/env bash
# File generated by pre-commit: https://pre-commit.com
# ID: 138fd403232d2ddd5efb44317e38bf03

# start templated
INSTALL_PYTHON=/abs/path/to/venv/bin/python
ARGS=(hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit)
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
(`--skip-on-missing-config` appended to `ARGS` when `--allow-missing-config`; Windows prepends `#!/bin/sh\n`, `install_uninstall.py:97-108`; `INSTALL_PYTHON` uses `shlex.quote`, `ARGS` uses `shlex.join`.)

Common regexes used by assertions (copy into snp tests, `FILES_CHANGED` handles macOS):
```
FILES_CHANGED = ( 1 file changed, 0 insertions\(\+\), 0 deletions\(-\)\n| 0 files changed\n)
NORMAL_PRE_COMMIT_RUN  = ^\[INFO\] Initializing environment for .+\.\nBash hook\.+Passed\n\[master [a-f0-9]{7}\] commit!\n{FILES_CHANGED} create mode 100644 foo\n$
FAILING_PRE_COMMIT_RUN = ^\[INFO\] Initializing environment for .+\.\nFailing hook\.+Failed\n- hook id: failing_hook\n- exit code: 1\n\nFail\nfoo\n\n$
EXISTING_COMMIT_RUN    = ^legacy hook\n\[master [a-f0-9]{7}\] commit!\n{FILES_CHANGED} create mode 100644 baz\n$
FAIL_OLD_HOOK          = fail!\n\[INFO\] Initializing environment for .+\.\nBash hook\.+Passed\n
PRE_INSTALLED          = Bash hook\.+Passed\n\[master [a-f0-9]{7}\] commit!\n{FILES_CHANGED} create mode 100644 foo\n$
```
(`git commit` output arrives on stderr+stdout merged; hook output under git goes to **stderr**, observed.)

| ID | Source | Setup | Action | Expected | Needs |
|---|---|---|---|---|---|
| INS-01 | :31-58 | — | resolve hook types | explicit `[pre-push]`; `None`→`[pre-commit]`; config `default_install_hook_types: [pre-push]`→that; `[]`→`[]` (installs nothing); unparsable config `[`→`[pre-commit]` | — U |
| INS-02 | :61-72 | — | is-our-script | `setup.py` false; template true; file with `PRIOR_HASHES[0]` true | — U |
| INS-03 | :75-97 | `in_git_dir` | `install` (pre-commit, then pre-push); `install -t pre-commit -t pre-push` then `uninstall -t …` | ret 0; hooks executable (`os.X_OK`); both removed by uninstall | — I,D |
| INS-04 | :83-88 | `.git/hooks` deleted | `install` | dir recreated, hook exists | — I,D |
| INS-05 | :100-102 | `git config --local core.hooksPath hooks` | `install` | ret 1; `[ERROR] Cowardly refusing to install hooks with \`core.hooksPath\` set.\nhint: \`git config --unset-all core.hooksPath\`` | — I,D |
| INS-06 | :105-109 | `.git/hooks/pre-commit` → dangling symlink `/fake/baz` | `install` | ret 0, hook exists (lexists → moved to `.legacy`) | posix I,D |
| INS-07 | :112-121 | — | `uninstall` when absent / after install | ret 0 / file removed; prints `pre-commit uninstalled` | — I,D |
| INS-08 | :154-200 | consuming repo `script_hooks_repo`; variants: config moved to `custom.yaml` + `install -c custom.yaml`; repo is a submodule of parent; git worktree (`branch -m notmaster; worktree add <path> -b master`) | `install`; touch+add `foo`; `git commit` | ret 0, matches NORMAL_PRE_COMMIT_RUN (hooks installed into **common git dir** `git rev-parse --git-common-dir`/hooks for worktrees) | — I,D |
| INS-09 | :203-217 | committed `unstaged` file then modified (unstaged) | install; commit `-a`? (`git_commit` uses `-a`) | ret 0 (#322) | — I |
| INS-10 | :220-234 | branch merge `--no-ff --no-commit -m ☃` | `git commit --no-edit` | no crash | — I |
| INS-11 | :237-245, :367-381 | — | install twice (± legacy) | idempotent; legacy still runs first | — I,D |
| INS-12 | :265-296 | hook file `INSTALL_PYTHON="/dne"`; PATH without pre-commit; minimal env | `git commit` | ret 1; output exactly ``\`pre-commit\` not found.  Did you forget to activate your virtualenv?\n`` (snp's own template must have an equivalent fallback; exact string = open question) | — I |
| INS-13 | :311-318 | `failing_hook_repo` | install, commit | ret 1, FAILING_PRE_COMMIT_RUN | — I,D |
| INS-14 | :321-354 | pre-existing `.git/hooks/pre-commit` = `#!/usr/bin/env bash\necho legacy hook\n` (+x) | commit (legacy only) → install (no `-f`) → commit | EXISTING_COMMIT_RUN; install prints `Running in migration mode with existing hooks at .git/hooks/pre-commit.legacy\nUse -f to use only pre-commit.` then `pre-commit installed at .git/hooks/pre-commit`; commit output = `legacy hook\n` + NORMAL_PRE_COMMIT_RUN | — I,D |
| INS-15 | :357-364 | legacy, install, write legacy again, install | — | ret 0 (#1010) | — I |
| INS-16 | :384-394 | existing hook with bytes `\xa0\xef\x12\xf2` | install | ret 0 (binary-safe hash search) | — I,D |
| INS-17 | :404-418 | legacy `echo "fail!"; exit 1` | install, commit | ret 1; output matches FAIL_OLD_HOOK (pre-commit still runs after legacy failure; exit = legacy_ret \| run_ret, `hook_impl.py:269-275`) | — I,D |
| INS-18 | :421-443 | ± legacy | `install -f` | legacy deleted; NORMAL run | — I,D |
| INS-19 | :446-458 | legacy | install, uninstall | prints `pre-commit uninstalled`, `Restored previous hooks to .git/hooks/pre-commit`; commit = EXISTING_COMMIT_RUN | — I,D |
| INS-20 | :461-480 | hook file = template with CURRENT_HASH replaced by PRIOR_HASHES[-1] | install | treated as ours (no `.legacy`); NORMAL run | — I |
| INS-21 | :483-490 | foreign `.git/hooks/pre-commit` (`echo 1`) | uninstall | ret 0, file kept | — I,D |
| INS-22 | :501-523 | — | `install --install-hooks` / `install` + `install-hooks` | commit output = PRE_INSTALLED (no `[INFO] Initializing…` line; env already present) | — I |
| INS-23 | :526-550 | PATH without pre-commit, HOME real | commit | NORMAL (hook uses absolute interpreter path) — snp: template embeds absolute `snp` path | — I |
| INS-24 | :561-627 | upstream `failing_hook_repo` cloned; `install -t pre-push`; commits `foo`, `zzz` | `git push origin HEAD:new_branch` | ret 1; `Failing hook`, `Failed`, `foo zzz` (both files of both commits), `hook id: failing_hook`; passing variant ret 0; force push without fetch ret 0; new `origin` remote unrelated → ret 0 | — I,D |
| INS-25 | :630-654 | local hook `bash -c "echo remote: $PRE_COMMIT_REMOTE_NAME"` verbose; remote renamed `origin2` | push to origin2 | `\nremote: origin2\n` in output | — I,D |
| INS-26 | :657-666 | nothing new | push twice | second output exactly `Everything up-to-date\n`, ret 0 (no hook output: `_pre_push_ns` returns None) | — I,D |
| INS-27 | :669-693 | legacy pre-push reading `read lr ls rr rs` and asserting non-empty | push | line1 `legacy`, line3 starts `Bash hook` ends `Passed` (stdin forwarded to legacy and reused) | — I,D |
| INS-28 | :696-740 | `commit_msg_repo` | install `-t commit-msg`; commit | fail: output exactly `Must have "Signed off by:"...............................................Failed\n- hook id: must-have-signoff\n- exit code: 1\n`; pass with msg `Hi\nSigned off by: me, lol`; legacy commit-msg receives `$1` and prints `legacy` first | — I,D |
| INS-29 | :743-922 | local `system` hooks with `always_run`, `verbose`, `entry: touch <stage>.tmp`; stages post-commit / post-merge (diamond merge) / pre-rebase / post-rewrite (`commit --amend`) / post-checkout (`bash -c "echo ${PRE_COMMIT_TO_REF}"` + meta identity) | install `-t <stage>`; trigger git op | marker file created only after install; post-checkout stderr contains new HEAD sha and **not** `some_file` (no filenames for post-checkout) | — I,D |
| INS-30 | :925-953 | post-checkout hook `language: fail` always_run; pre-commit also installed; staged `file` + unstaged change | `git commit` (no `-a`) | ret 0 — stash/restore must not trigger post-checkout (`_PRE_COMMIT_SKIP_POST_CHECKOUT` set around internal `git checkout`) | — I,D |
| INS-31 | :956-1012 | prepare-commit-msg repos | install `-t prepare-commit-msg`; commit `-m Hi` | fail output exactly `Add "Signed off by:".....................................................Failed\n- hook id: add-signoff\n- exit code: 1\n`; pass: `.git/COMMIT_EDITMSG` contains `Signed off by: `; legacy prints `legacy` first | — I,D |
| INS-32 | :1015-1038 | `install -t pre-merge-commit`; feature branch commit | `git merge --no-ff --no-edit feature` | ret 0; `^\[INFO\] Initializing environment for .+\nBash hook\.+Passed\nMerge made by the '(ort\|recursive)' strategy.\n foo \| 0\n 1 file changed, 0 insertions\(\+\), 0 deletions\(-\)\n create mode 100644 foo\n$` | — I,D |
| INS-33 | :1041-1091 | config removed; install `-f` without/with `--allow-missing-config`; env `PRE_COMMIT_ALLOW_NO_CONFIG=1` | commit | ret 1 / ret 0 with ``\`.pre-commit-config.yaml\` config file not found. Skipping \`pre-commit\`.`` / ret 0 same text | — I,D |
| INS-34 | :1094-1104 | config `default_install_hook_types: [pre-commit, pre-push]` | `install`, `uninstall` (no `-t`) | both installed/removed | — I,D |
| ITD-01 | init_templatedir_test.py:118-139 | target `<tmp>/tmpl`, `init.templateDir` unset | `init-templatedir <tmpl> -t pre-commit` | lines: `pre-commit installed at …`, `[WARNING] \`init.templateDir\` not set to the target directory`, `[WARNING] maybe \`git config --global init.templateDir …`; then `GIT_TEMPLATE_DIR=<tmpl>` new consuming repo → commit runs `Bash hook....` | — I,D |
| ITD-02 | :142-184 | `init.templateDir` == target (also via `~/templatedir` + expanduser) | same | exactly 1 line | — I |
| ITD-03 | :156-169 | `HOME=<tmp>`, target `.` | same | 3 lines | — I |
| ITD-04 | :187-195 | `core.hooksPath` set locally | same | still installs `<tmpl>/hooks/pre-commit` (no refusal when explicit git dir) | — I |
| ITD-05 | :198-241 + main_test.py:404-428 | default `skip_on_missing_config=True` for init-templatedir; `--no-allow-missing-config` | commit in fresh repo without config | ret 0 + `Skipping \`pre-commit\`.` / ret 1 + `No .pre-commit-config.yaml file was found` | — I,D |

### 5.6 hook-impl per hook type (`tests/commands/hook_impl_test.py`)

| ID | Source | Setup / input | Action | Expected |
|---|---|---|---|---|
| HKI-01 | :22-58 | config exists / missing (skip=False) / missing (skip=True) / missing + `PRE_COMMIT_ALLOW_NO_CONFIG=1` | `_validate_config`/`_run_ns` (U) | ok / exit **1** stdout `No DNE.yaml file was found\n- To temporarily silence this, run \`PRE_COMMIT_ALLOW_NO_CONFIG=1 git ...\`\n- To permanently silence this, install pre-commit with the --allow-missing-config option\n- To uninstall pre-commit run \`pre-commit uninstall\`\n` / exit **= legacy retv (123)** stdout ``\`DNE.yaml\` config file not found. Skipping \`pre-commit\`.\n`` / exit 0 same text |
| HKI-02 | :61-97 | `<hook_dir>/<type>.legacy` states | `_run_legacy` (U) | legacy missing → (0, b''); `hook_dir=None` (git 2.54+) → (0, stdin); legacy `pre-commit.legacy` prints `hi arg1 arg2`, exit 1 → (1, b''); pre-push reads all stdin and returns it (legacy gets it as input); recursion (`PRE_COMMIT_RUNNING_LEGACY` set) → SystemExit with `bug: pre-commit's script is installed in migration mode\nrun \`pre-commit install -f --hook-type {hook_type}\` to fix this\n\nPlease report this bug at https://github.com/pre-commit/pre-commit/issues` |
| HKI-03 | :99-119 | arg counts OK: pre-commit 0, pre-merge-commit 0, pre-push 2, commit-msg 1, post-commit 0, post-merge 1, pre-rebase 1–2, post-checkout 3, post-rewrite 1, prepare-commit-msg 1–3 | `_check_args_length` (U) | no error |
| HKI-04 | :122-153 | wrong counts | `_check_args_length` (U) | `hook-impl for pre-commit expected 0 arguments but got 2: ['run', '--all-files']`; `hook-impl for commit-msg expected 1 argument but got 0: []` (singular!); `hook-impl for prepare-commit-msg expected 1, 2, or 3 arguments but got 0: []`; `hook-impl for pre-rebase expected 1 or 2 arguments but got 0: []` (Python list repr `['a', 'b']`; exit code 1, message on **stderr** via SystemExit(str)) |
| HKI-05 | :156-253 | namespace mapping | `_validate_config`/`_run_ns` (U) | pre-commit/post-commit/pre-merge-commit: stage only; pre-rebase (`main`,`topic`) → upstream/branch, 1 arg → branch None; commit-msg → filename; prepare-commit-msg 1/2/3 args → filename, source, object name; post-merge `1` → is_squash_merge; post-rewrite `amend` → rewrite_command; post-checkout `a b c` → from_ref, to_ref, checkout_type; color passthrough |
| HKI-06 | :256-347 | `push_example`: src repo 1 commit; clone + 1 commit; stdin lines `<local_ref> <local_sha> <remote_ref> <remote_sha>` (parsed with `rsplit(maxsplit=3)` so local ref may contain spaces) | `_validate_config`/`_run_ns` (U) | updating branch → from=remote_sha (src_head), to=local_sha, all_files False; new branch (remote Z40) → from = parent of first commit not on `--remotes=origin` (`rev-list <sha> --topo-order --reverse --not --remotes=<name>`), to=local; new branch at already-pushed rev → **None** (nothing to push → no run); ref with whitespace `HEAD^{/ }` → None; orphan branch whose first ancestor is a root commit → `all_files=True`; delete (`(delete) Z40 refs/heads/b <sha>`) → None |
| HKI-07 | :350-367 | delete-branch stdin + config | `hook-impl --hook-type pre-push` | ret 0, **no output** |
| HKI-08 | :370-386 | `sample_local_config` in git dir, nothing staged | `hook-impl --hook-type pre-commit` | ret 0, output exactly `Block if "DO NOT COMMIT" is found....................(no files to check)Skipped\n` |
| HKI-09 | main_test.py:392-395 | — | `hook-impl --hook-type pre-commit` | dispatches (4.6.0: `--hook-type` required; `--hook-dir` optional) |

Hook-type × git-trigger matrix for the integration suite (one end-to-end test each, I+D):

| hook type | git trigger | args from git | stdin | files passed to hooks |
|---|---|---|---|---|
| pre-commit | `git commit` | — | — | staged (`diff --staged --name-only --diff-filter=ACMRTUXB -z`) or conflicted during merge |
| pre-merge-commit | `git merge --no-ff` | — | — | staged |
| prepare-commit-msg | `git commit` | msgfile [source [sha]] | — | `(msgfile,)` |
| commit-msg | `git commit` | msgfile | — | `(msgfile,)` |
| post-commit | `git commit` | — | — | none |
| post-merge | `git merge`/`pull` | squash flag | — | none |
| post-checkout | `git checkout`/`switch`/`clone` | prev new flag | — | none |
| post-rewrite | `commit --amend`, `rebase` | `amend`/`rebase` | rewritten list (ignored) | none |
| pre-rebase | `git rebase` | upstream [branch] | — | none |
| pre-push | `git push` | remote url | ref lines | `diff --name-only from...to` (or all files for root pushes) |

### 5.7 git helpers (`tests/git_test.py`)

| ID | Source | Setup | Expected |
|---|---|---|---|
| GIT-01 | :13-40 | repo root / deeper dir / inside `.git/objects` / parent of repo / exactly `.git` | root returned for first two; **FatalError** for the rest (snp: `git failed. Is it installed, and are you in a Git repository directory?`) |
| GIT-02 | :43-81 | bare clone + `worktree add foo`; worktree in `.git/trees/foo` | root = worktree path; `get_git_dir` = `src/.git/worktrees/worktree`, common dir = `src/.git` (#1972) |
| GIT-03 | :84-89 | `git rm --cached` committed file | staged files = [] (deletions excluded) |
| GIT-04 | :92-108 | merge conflict (incl. in submodule); cherry-pick conflict | in-merge-conflict true for merge/submodule (`MERGE_MSG` + `MERGE_HEAD` in git dir), **false for cherry-pick** |
| GIT-05 | :117-150 | resolved conflict + `other_file` added; unstaged edits; a file named `HEAD` | conflicted files = `{conflict_file, other_file}`; unstaged ignored; `{conflict_file, HEAD}` (needs `--` separator) |
| GIT-06 | :153-166 | MERGE_MSG `Merge branch 'foo' into bar\n\nConflicts:\n\tconflict_file\n` (+`\tother_conflict_file\n`) | parsed tab-indented lines after `Conflicts:` |
| GIT-07 | :169-198 | `HEAD^..HEAD` adds a.txt b.txt; reversed → []; disparate histories (orphan) → `['b.txt']` | uses `git diff --name-only --no-ext-diff -z old...new` (three-dot); on error (no merge base) retries `old..new` (`git.py:158-167`) |
| GIT-14 | git.py:96-133 | merge state | in-conflict ⇔ both `MERGE_MSG` and `MERGE_HEAD` exist in the (worktree) git dir; conflicted files = tab-prefixed lines of `MERGE_MSG` (also `#\t` form) ∪ `git diff --name-only --no-ext-diff -z -m <write-tree> HEAD MERGE_HEAD --`; logs `[INFO] Checking merge-conflict files only.` |
| GIT-08 | :201-211 | zsplit | `'foo\0bar\0'`→[foo,bar]; `'foo\0'`→[foo]; `''`→[]; `'foo'`→[foo] |
| GIT-09 | :214-254 | file `интервью` | all/staged/changed/conflicted return the UTF-8 name unescaped (always `-z`, never `core.quotePath` output) |
| GIT-10 | :228-236 | merge conflict | `get_all_files` sorted == `[.pre-commit-config.yaml, bar_only_file, conflict_file, foo_only_file, placeholder]` (no duplicates, `--deduplicate`) |
| GIT-11 | :257-272 | `add --intent-to-add a`; staged rename `a→b` + ITA `c` | ITA files = `['a']` / `['c']` — computed with `git diff --no-ext-diff --ignore-submodules --diff-filter=A --name-only -z` (worktree vs index; ITA entries show as Added; the staged rename must not appear) (`git.py:146-151`) |
| GIT-12 | :275-299 | env with `GIT_DIR`, `GIT_EXEC_PATH`, `GIT_SSH`, `GIT_SSH_COMMAND`, `GIT_CONFIG_KEY_0..`, `GIT_CONFIG_COUNT`, `http_proxy` | `no_git_env` drops `GIT_DIR` only from that set (keeps EXEC_PATH, SSH, SSH_COMMAND, CONFIG_KEY_*/VALUE_*/COUNT, non-GIT vars) (`git.py:27-48`) |
| GIT-13 | :302-304 | `init_repo(tmp, remote='dne')` | no `.git/hooks` dir created (uses empty template) |

### 5.8 store (`tests/store_test.py`)

| ID | Source | Expected |
|---|---|---|
| STO-01 | :44-66 | default dir `realpath(~/.cache/pre-commit)`; `XDG_CACHE_HOME=/tmp/fakehome` → `/tmp/fakehome/pre-commit`; `PRE_COMMIT_HOME=/tmp/pre_commit_home` → realpath of it (precedence PRE_COMMIT_HOME > XDG_CACHE_HOME > ~/.cache) |
| STO-02 | :69-79 | store dir created with `README` containing `This directory is maintained by the pre-commit project.` and `Learn more: https://github.com/pre-commit/pre-commit` |
| STO-03 | :82-105 | clone(path, rev): first log `Initializing environment for <repo>.`; returned dir inside store, basename starts with `repo`, HEAD==rev, DB row `(repo, ref, path)` |
| STO-04 | :108-171 | manifest with deprecated stages → second log record: ``repo `{path}` uses deprecated stage names (commit, push, merge-commit) which will be removed in a future version.  Hint: often `pre-commit autoupdate --repo {shlex.quote(path)}` will fix this.  if it does not -- consider reporting an issue to that repo.`` (dedup, first-appearance order); warned **only on first clone**; none for new names |
| STO-05 | :174-184 | clone `/i_dont_exist_lol` fails; error mentions the path; no leftover `repo*` dir |
| STO-06 | :187-196 | DB row exists → returned without cloning |
| STO-07 | :199-231 | shallow fetch fails → full fetch fallback; same result |
| STO-08 | :234-245 | rev = tag only reachable from a deleted branch → clone works |
| STO-09 | :248-258 | dir without DB → DB created; re-open idempotent |
| STO-10 | :261-263 | `db_repo_name('repo', ())`=`repo`; `('b','a','c')` → `repo:b,a,c` (**order preserved, not sorted**) |
| STO-11 | :275-295 | `mark_config_used('f')` stores absolute path; idempotent; nonexistent path ignored; missing `configs` table (pre-1.14) recreated |
| STO-12 | :298-319 | read-only store dir (posix) → `readonly` true; `mark_config_used` is a no-op |
| STO-13 | :322-343 | repo with nested submodule → clone checks out submodules recursively |

If snp keeps its own store format (open question §10), port STO-* as behavior tests (via CLI + `gc` + directory inspection) instead of SQL assertions, and add **interop tests** if snp reads/writes pre-commit's `db.db` schema (`repos(repo TEXT, ref TEXT, path TEXT, PRIMARY KEY(repo, ref))`, `configs(path TEXT PRIMARY KEY)`).

### 5.9 repository / hook resolution (`tests/repository_test.py`)

| ID | Source | Expected | Needs |
|---|---|---|---|
| REP-01 | :83-89 | `not_found_exe` → (1, ``Executable `i-dont-exist-lol` not found``) | — |
| REP-02 | :92-120 | script hook argv: `['bar']`→`bar\nHello World\n`; `['foo bar','baz']` with manifest args → `arg: hello\narg: world\narg: foo bar\narg: baz\n`; config args `hi {1}`, `I'm {a} problem` verbatim (no format/brace expansion) | — |
| REP-03 | :123-140 | interleaved `0..5` stdout/stderr in order (one pipe); with color: `stdin: False\nstdout: True\nstderr: True\n` | pty |
| REP-04 | :143-156 | hook cwd = repo root (`pwd -L` equals `cd <root> && pwd`) | — |
| REP-05 | :159-163 | 15000 args | — |
| REP-06 | :166-198, :218-302 | python env: deps roll-forward isolation (mccabe), `.install_state_v1/v2` compat, reinstall logs 4 records then 0, Ctrl-C during install and cleanup → recovered, invalidated venv rebuilt, very long cwd | tc:python (net for mccabe) |
| REP-07 | :201-215 | `Unexpected key(s) present on local => too-much: foo, hello` | — |
| REP-08 | :305-314 | config `files` overrides manifest | — |
| REP-09 | :323-338 | rev = tag `v1.1` for two repos | — |
| REP-10 | :341-375 | local python repo with deps → language_version resolved (≠ `default`), output `['filename']\nHello World\n`; `default_language_version` applies unless hook sets it | tc:python |
| REP-11 | :378-392 | `default_stages` applies unless hook sets `stages` | — |
| REP-12 | :395-406 | unknown id → exit 1, `[ERROR]` ``\`i-dont-exist\` is not present in repository file://{path}.  Typo? Perhaps it is introduced in a newer version?  Often \`pre-commit autoupdate\` fixes this.`` | — |
| REP-13 | :409-451 | resolved manifest hook defaults: `alias ''`, `always_run False`, `args []`, `description ''`, `exclude '^$'`, `exclude_types []`, `files ''`, `language 'unsupported_script'`, `language_version 'default'`, `log_file ''`, `minimum_pre_commit_version '0'`, `pass_filenames True`, `require_serial False`, `stages` = all 11 in order `[commit-msg, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg, manual]`, `types ['file']`, `types_or []`, `verbose False`, `fail_fast False` | — U |
| REP-14 | :454-497 | `language: unsupported` + `language_version` → exit 1 ``The hook `system-hook` specifies `language_version` but is using language `unsupported` which does not install an environment.  Perhaps you meant to use a specific language?``; same with `additional_dependencies` | — |
| REP-15 | :500-508 | argv `('i have spaces', 'and"\'quotes', '$and !this')` reaches the process unchanged (no shell) | — |
| REP-16 | :511-519 | entry `pre-commit hazmat ignore-exit-code <python> -c 'raise SystemExit(sys.argv[1:])'` + files f1 f2 → (0, `['f1', 'f2']\n`) | — (>=4.5) |

### 5.10 languages (`tests/languages/*_test.py`)

Hermetic (portable as unit tests with mocked environment probes) vs toolchain/network tests:

| Language | Hermetic tests to port (U) | Toolchain / network tests (I, gated) | Local availability |
|---|---|---|---|
| fail | `fail_test.py:7-14`: entry `watch out for`, files `bunnies` → (1, `watch out for\n\nbunnies\n`) | — | n/a |
| pygrep | `pygrep_test.py:24-144`: see table below | — | n/a (in-process) |
| unsupported / system | `echo hello hello world` → (0, `hello hello world\n`) | — | n/a |
| unsupported_script / script | `main` script → same | — | n/a |
| python | `_read_pyvenv_cfg` (comments/blank lines skipped; `k = v` and `k=v`; non-UTF-8 value `hello john.š`) :21-37; default-version matrix :66-103 (cpython `python3.12` exe found → `python3.12`; exe `python3` with `python3.12` found → `python3.12`; pypy exe `python` with `pypy3.12` → `pypy3.12`; only `pypy3` → `pypy3`); `norm_version('~/.pyenv/…')` expands `~`; `norm_version('default')` → None; `_sys_executable_matches` true for `python3.9`,`python3`,`python` on 3.9, false for `notpython`,`python3.x`; `_find_by_sys_executable` table :135-148 | health checks :160-282 (messages `virtualenv python version did not match created version:\n- actual version: <<error retrieving version from {exe}>>\n- expected version: {v}\n`, `base executable python version does not match created version:\n- base-executable version: …`, `pyvenv.cfg does not exist (old virtualenv?)`, ``created virtualenv's pyvenv.cfg is missing `version_info` ``); hooks via setup.py | tc:python3.12/3.13 + virtualenv (`venv462` has virtualenv 21.14.5; `uv` 0.8.8 available) |
| node | default version: linux + node&npm found → `system`; not found → `default`; win32 → `default` always | health `` `node --version` returned 127 `` after removal; nodeenv versions (net); deps lodash (net); npm build script | node v24.12.0 present |
| golang | default `system` iff `exe_exists('go')`; `_infer_go_version('1.19.4')` passthrough | `default` resolution (net, go.dev), module build (net, BurntSushi/toml), versioned 1.21.1 download (net), `GOBIN` set, toolchain switching error `go.mod requires go >= 1.23.1` | go present |
| rust | default `system` when `rustup` probe returns 0; `default` when 127; `rust-toolchain.toml` with bogus channel still `system` | rustup bootstrap 1.56.0 (net, slow); `cli:shellharden:4.2.0` deps (net); lib deps | rustup/cargo present |
| ruby | default `system` iff ruby+gem found; vendored archive root mode 0o755 | rbenv/ruby-build (net), gems (net) | ruby present (gem?) |
| docker | `get_docker_user()` → `()` when no getuid; rootless JSON `{"SecurityOptions": ["name=rootless",…]}` or `{"host": {"security": {"rootless": true}}}` → `()`; non-rootless (incl. `SecurityOptions: null`, other engine JSON, exit 1) → `(-u, uid:gid)`; container id from `/proc/self/mountinfo` samples (docker cgroup v1/v2, podman v1/v2 → `c33988ec7651ebc867cb24755eaf637a6734088bc7eef59d5799293a9e5450f7`; non-docker → None; missing file → None); `_get_docker_path` bind translation (`/project`→`/opt/my_code`, `/project/test/something`→`/opt/my_code/test/something`, `/projectSuffix/...` unchanged, many binds, Windows `c:\folder\…`→`c:\users\user\…`, docker-in-docker inspect failure → unchanged) — copy the 4 mountinfo fixtures from `docker_test.py:17-180` verbatim into snp test data | build+run `ubuntu:22.04` (net), mount permissions | docker present |
| docker_image | — | `--entrypoint echo ubuntu:22.04` args; color tty output `\x1b[01;31m\x1b[Kroot\x1b[m\x1b[K:x:0:\n` vs `root:x:0:\n` | docker present |
| r | entry parsing: `Rscript <file>` / `Rscript -e <expr>`; errors `You can supply at most one expression.`, ``entry must start with `Rscript`.``; `_rscript_exec` relative to `R_HOME` else `Rscript` | renv install (net) | R present |
| conda | `_conda_exe` selection by env (`PRE_COMMIT_USE_MAMBA`/`PRE_COMMIT_USE_MICROMAMBA`) | env create (net) | missing |
| coursier | error ``expected .pre-commit-channel dir or additional_dependencies`` | cs (net) | missing |
| dart | — | pub (net) | dart present |
| dotnet | — | build (net) | missing |
| haskell | error `Expected .cabal files or additional_dependencies` | cabal (net) | missing |
| julia | — | depot (net) | missing |
| lua | — | luarocks (net), posix only | missing |
| perl | — | cpan (net) | perl present |
| swift | — | swift build, posix only | missing |

pygrep cases (files: f1 `foo\nbar\n`, f2 `[INFO] hi\n`, f3 `with'quotes\n`, f4 `foo\npattern\nbar\n`, f5 `[INFO] hi\npattern\nbar` (no trailing newline), f6 `pattern\nbarwith'foo\n`, f7 `hello'hi\nworld\n`, f8 `foo\nbar\nbaz\n`, f9 `[WARN] hi\n`):

| args | ret | stdout |
|---|---|---|
| `baz f1 f2 f3` | 0 | `` |
| `foo f1 f2 f3` | 1 | `f1:1:foo\n` |
| `bar …` | 1 | `f1:2:bar\n` |
| `(?i)\[info\] …` | 1 | `f2:1:[INFO] hi\n` |
| `h'q …` | 1 | `f3:1:with'quotes\n` |
| `pattern\nbar f4 f5 f6 --negate` | 1 | `f4\nf5\nf6\n` |
| `foo f4 f5 f6 --negate` | 1 | `f5\n` |
| `pattern f4 f5 f6 --negate` | 0 | `` |
| `baz f4 f5 f6 --negate --multiline` | 1 | `f4\nf5\nf6\n` |
| `foo\npattern f4 f5 f6 --negate --multiline` | 1 | `f5\nf6\n` |
| `pattern\nbar f4 f5 f6 --negate --multiline` | 0 | `` |
| `--ignore-case info f1 f2 f3` | 1 | `f2:1:[INFO] hi\n` |
| `--multiline foo\nbar f1 f2 f3` (regex `foo\nbar`) | 1 | `f1:1:foo\nbar\n` |
| `--multiline ar …` | 1 | `f1:2:bar\n` |
| `--multiline o.*bar …` (DOTALL) | 1 | `f1:1:foo\nbar\n` |
| `--multiline foo$.*bar …` (MULTILINE) | 1 | `f1:1:foo\nbar\n` |
| via language runner `ello` on f7 f8 f9 | 1 | `f7:1:hello'hi\n` |
| `nope` / `foo'bar` / `^\[INFO\]` on f7 f8 f9 | 0 | `` |

(`pygrep` operates on bytes; output line is `filename:lineno:` + matched line bytes; multiline prints whole match span starting at its line.)

### 5.11 autoupdate (`tests/commands/autoupdate_test.py`)

Algorithm notes: per repo (excluding local/meta) a temp repo `git init` + `remote add origin <repo>`, `config extensions.partialClone true`, `fetch origin HEAD --quiet --filter=blob:none --tags`; rev = `describe FETCH_HEAD --tags --abbrev=0` followed by "best candidate tag" selection (prefer a tag containing `.` among tags pointing at the same commit) when `tags_only` (the **default**; `--bleeding-edge` sets `tags_only=False`, `main.py:226,389`), or `describe FETCH_HEAD --tags --exact` with `--bleeding-edge`; if `describe` fails → `rev-parse FETCH_HEAD`; `-j/--jobs` default 1 (0 = cpu count, capped at number of repos); freeze → sha with `  # frozen: <tag>` comment; output per repo `[<repo>] updating <old> -> <new>` / `[<repo>] updating <old> -> <tag> (frozen)` / `[<repo>] already up to date!`, results printed in **completion order** (deterministic only with `--jobs 1`, the default); file rewritten only if something changed (`autoupdate.py:170-221`). Rev line regex `^(\s+)rev:(\s*)(['"]?)([^\s#]+)(.*)(\r?\n)$` applied to `str.splitlines(True)` lines; if the number of matching lines != number of repos, the file is **re-dumped** through PyYAML (`yaml_dump(yaml_load(original))`) and retried (`autoupdate.py:116-138`). New rev rendered with `yaml_dump({'rev': rev}, default_style=<captured quote>)` (quotes added when the plain scalar would not round-trip, see §7.5.6 vectors).

| ID | Source | Setup | Expected |
|---|---|---|---|
| AUP-01 | :63-66 | `{'repo': 'repo/path', 'rev': 'v1.2.3'}` | RevInfo(repo, rev, None) |
| AUP-02 | :68-139 | up-to-date repo; out-of-date; default branch renamed `dev`; tag `v1.2.3` on newest commit (tags_only False → picks tag); tags_only with commits after tag → `v1.2.3`; tags `v1.2.3` and `latest` on same commit → prefers `v1.2.3` (version-like tag); only `latest` → `latest`; freeze tag → rev = sha, frozen `v1.2.3`; freeze when already sha → frozen None | as stated |
| AUP-03 | :142-154 | config at HEAD | ret 0, file **byte-identical** |
| AUP-04 | :157-181 | config pinned at a rev where `.pre-commit-hooks.yaml` was absent (renamed `nope.yaml`) | ret 0, updated to new HEAD |
| AUP-05 | :184-235 | `fmt = 'repos:\n-   repo: {}\n    rev: {}\n    hooks:\n    -   id: foo\n'`; with global `core.useBuiltinFSMonitor=true` in `$HOME/.gitconfig`; pure-python dumper; two repos only one stale | exact rewritten text; only the stale rev changes |
| AUP-06 | :238-283 | `--repo file://<path>` with a local repo also present; `--repo dne --repo foo` | updated, `local` preserved; ret 1, file unchanged, stdout exactly `repos (dne, foo) were not found in .pre-commit-config.yaml\n` (sorted) (>=4.6.1) |
| AUP-07 | :286-301 | `    rev: {}  # definitely the version I want!\n` + comment line + `args: [foo, bar, baz]` | comments and flow lists preserved byte-exact |
| AUP-08 | :304-340 | mixed `\n`/`\r\n` line endings (read & write) | untouched when up to date; when updated, each line keeps its own ending |
| AUP-09 | :343-372 | flow-style config `repos: [{repo: …, rev: …, hooks: [# A comment!\n {id: foo}]}]` | formatting lost: output exactly `repos:\n-   repo: {path}\n    rev: {head}\n    hooks:\n    -   id: foo\n` |
| AUP-10 | :375-396 | tagged | `v1.2.3` written; `--freeze` → `rev: <sha>  # frozen: v1.2.3`; un-freeze run → `rev: v1.2.3\n` (frozen comment removed) |
| AUP-11 | :399-408 | `--tags-only` | `v1.2.3` |
| AUP-12 | :411-422 | upstream deleted all files (no manifest at HEAD) | ret 1, old rev kept; message `[<repo>] ` + manifest error |
| AUP-13 | :425-448 | hook `foo` renamed to `bar` upstream | ret 1, file unchanged, message ``[<repo>] Cannot update because the update target is missing these hooks: foo`` |
| AUP-14 | :451-473 | local-only config; local first then stale remote | ret 0; local repo dict unchanged |
| AUP-15 | :475-489 | meta-only config | ret 0, byte-identical |
| AUP-16 | :492-514 | old list-format config | ret 0; migrated to `repos:` form; stdout exactly `Configuration has been migrated.\n` (migrate runs first, quiet only for "already migrated") |
| AUP-17 | :517-534 | `rev: "{rev}"` and `rev: '{rev}'` | quoting style preserved |

All AUP tests use `file://` repos → **hermetic** (set `GIT_ALLOW_PROTOCOL=file`, `protocol.file.allow=always`).

### 5.12 migrate-config & yaml_rewrite (`tests/commands/migrate_config_test.py`, `tests/yaml_rewrite_test.py`)

All parametrized over libyaml (`c`) and pure-Python loaders — results must be identical (snp has one loader).

| ID | Source | Input | Output | stdout |
|---|---|---|---|---|
| MIG-01 | :26-49 | top-level list `-   repo: local\n    hooks:\n    -   id: foo\n …` | `repos:\n` prepended, rest unchanged | `Configuration has been migrated.\n` |
| MIG-02 | :52-79 | `# comment\n\n---\n-   repo: local…` | header lines (`#…`, `---`, blank) kept, `repos:` inserted after them | — |
| MIG-03 | :82-105 | flow list `[{\n    repo: local,\n …}]` (no trailing newline) | `repos:\n    [{\n        repo: local,\n …    }]` (whole rest indented 4 spaces because the unindented trial does not parse) | — |
| MIG-04 | :108-124 | already `repos:` form | unchanged | `Configuration is already migrated.\n` |
| MIG-05 | :127-171 | `sha: v1.2.0` keys (YAML and JSON-style `"sha": "v1.2.0"`) | key renamed `rev` keeping quotes | — |
| MIG-06 | :174-229 | `language: python_venv` / `"python_venv"` | `python` / `"python"`; other hooks untouched | — |
| MIG-07 | :232-271 | `default_stages: [commit, push, merge-commit, commit-msg]`; hook `stages: ["commit", "push", "merge-commit", "commit-msg"]` | `[pre-commit, pre-push, pre-merge-commit, commit-msg]` (quotes preserved) | — |
| MIG-08 | :274-281 | `[` | error: `InvalidConfigError` text starts `\n==> File .pre-commit-config.yaml\n=====> ` | exit 1 via error handler |
| MIG-09 | yaml_rewrite_test.py:1-47 | matchers `SequenceItem`, `MappingKey`, `MappingValue` | only scalar nodes yielded (`- name: [not, foo]` skipped); non-map/non-seq input yields nothing | — |

### 5.13 try-repo (`tests/commands/try_repo_test.py`)

Output frame: optional `[WARNING] Creating temporary repo with uncommitted changes...`, then `'='*79`, `Using config:`, `'='*79`, the YAML config (`yaml_dump`, hooks sorted by id), `'='*79`, then normal run output. Store is a **temporary directory** (fresh per invocation) → `[INFO] Initializing environment for <repo>.` lines (tests strip `\[INFO\].+\n`).

| ID | Source | Setup | Expected |
|---|---|---|---|
| TRY-01 | :45-71 | repo `modified_file_returns_zero_repo`; consumer dir with staged empty `test-file`; `--verbose`, monotonic patched to 0 | config `repos:\n-   repo: .+\n    rev: .+\n    hooks:\n    -   id: bash_hook\n    -   id: bash_hook2\n    -   id: bash_hook3\n`; rest exactly: `Bash hook............................................(no files to check)Skipped\n- hook id: bash_hook\nBash hook................................................................Passed\n- hook id: bash_hook2\n- duration: 0s\n\ntest-file\n\nBash hook............................................(no files to check)Skipped\n- hook id: bash_hook3\n` |
| TRY-02 | :74-89 | `try-repo <repo> bash_hook --verbose` | config lists only `bash_hook` (no manifest read); one Skipped line + `- hook id: bash_hook` |
| TRY-03 | :92-107 | relative repo path; bare repo path (`<repo>/.git`) | ret 0 |
| TRY-04 | :110-119 | `--ref <old sha>` | config contains that sha |
| TRY-05 | :122-143 | uncommitted manifest change (name `modified name!`) | starts `[WARNING] Creating temporary repo with uncommitted changes...\n`; repo path ends `shadow-repo`; rest exactly `modified name!...........................................................Passed\n` |
| TRY-06 | :146-155 | staged-but-uncommitted files in the hook repo | ret 0 (shadow repo includes staged files) |
| TRY-07 | main_test.py:301-310 | from subdir `try-repo ../foo` | repo path rewritten relative to toplevel (`foo`) |

### 5.14 gc / clean (`tests/commands/gc_test.py`, `clean_test.py`)

| ID | Source | Expected |
|---|---|---|
| GC-01 | gc_test.py:194-214 | config used at old rev, install-hooks, autoupdate, install-hooks → 2 repos, 1 config; `gc` → 1 repo; last line `1 repo(s) removed.`; after deleting config file → `gc` removes config + repo: `1 repo(s) removed.` |
| GC-02 | :217-241 | repo never cloned / meta-only / local-only config → `0 repo(s) removed.` |
| GC-03 | :244-266 | local `language: python` hook creates a store repo entry; kept while config exists |
| GC-04 | :269-289 | config references a hook id that does not exist → gc does not crash, keeps repo |
| GC-05 | :292-300 | invalid config `{'i am': 'invalid'}` marked used → config row deleted, `0 repo(s) removed.` |
| GC-06 | :303-322 | cloned repo whose manifest was deleted → repo removed (`1 repo(s) removed.`) |
| GC-07 | :325-330 | pre-1.14 DB without `configs` table → `0 repo(s) removed.\n` |
| GC-08 | clean_test.py:341-365 | `clean` removes store dir **and** legacy `~/.pre-commit`; prints `Cleaned <dir>.` per existing dir; idempotent |

### 5.15 meta hooks (`tests/meta_hooks/*`)

| ID | Source | Config | Expected |
|---|---|---|---|
| MET-01 | check_hooks_apply_test.py:7-96 | meta `check-useless-excludes` with `exclude: .pre-commit-config.yaml` / `files: foo` / `types: [python]` / `exclude_types: [yaml]` | ret 1; stdout contains `check-useless-excludes does not apply to this repository` |
| MET-02 | :99-140 | local hooks: script `files: \.yaml$`; `language: fail` `files: changelog/.*(?<!\.rst)$` (**negative lookbehind**); `always_run: true` with `files: ^$` | ret 0, no output (fail-language and always_run hooks exempt) |
| MET-03 | check_useless_excludes_test.py:9-27 | top-level `exclude: foo` | ret 1, `The global exclude pattern 'foo' does not match any files` |
| MET-04 | :30-50 | hook `exclude: foo` | ret 1, `The exclude pattern 'foo' for check-useless-excludes does not match any files` |
| MET-05 | :53-79 | `exclude: .pre-commit-config.yaml` + `types: [python]` | ret 1 (exclude evaluated after type filtering) |
| MET-06 | :82-120 | no excludes / matching exclude | ret 0, no output |
| MET-07 | :123-137 | committed broken symlink excluded by hook | ret 0 (posix) |
| MET-08 | identity_test.py:1-8 | `identity a b c` | ret 0, `a\nb\nc\n` |

Meta hook definitions (`clientlib.py:376-398`): `check-hooks-apply` (name `Check hooks apply to the repository`, `files: ^\.pre\-commit\-config\.yaml$`), `check-useless-excludes` (name `Check for useless excludes`, same files), `identity` (name `identity`, `verbose: true`); all `language: unsupported`, **not** `require_serial` → identity's file order is the **shuffled** order (§5.4). snp implements them in-process but must reproduce the output byte-for-byte.

### 5.16 output / color / logging

| ID | Source | Expected |
|---|---|---|
| OUT-01 | output_test.py:6-11 | `write('hello world')` writes UTF-8 bytes, flushes |
| OUT-02 | color_test.py:12-21 | `format_color('foo', GREEN, True)` = `\x1b[42mfoo\x1b[m`; False → `foo` |
| OUT-03 | :23-61 | `use_color`: `never`→F; `always`→T; `auto`: **stderr** not a tty → F; tty + color support + TERM unset → T; no support → F; `TERM=dumb` → F; invalid value → error (CLI exit 2, `invalid use_color value: '<v>'`) |
| OUT-04 | logging_handler_test.py:1-23 | WARNING colored: `\x1b[43;30m[WARNING]\x1b[m hi\n`; uncolored `[WARNING] hi\n`; ERROR uses `\x1b[41m`; INFO/DEBUG uncolored; all log lines go to **stdout** |
| OUT-05 | colors (`color.py:59-64`) | RED `\x1b[41m`, GREEN `\x1b[42m`, YELLOW `\x1b[43;30m`, TURQUOISE `\x1b[46;30m`, SUBTLE `\x1b[2m`, NORMAL `\x1b[m`. Default from `PRE_COMMIT_COLOR` else `auto`. Windows: tries enabling VT processing on stderr; failure → no color |

### 5.17 error handler & CLI (`tests/error_handler_test.py`, `tests/main_test.py`)

| ID | Source | Expected |
|---|---|---|
| ERR-01 | error_handler_test.py:25-107 | no exception → nothing; `FatalError` → msg `An error has occurred`, exit **1**; other exception → `An unexpected error has occurred`, exit **3**; KeyboardInterrupt → `Interrupted (^C)`, exit **130** |
| ERR-02 | :110-153 | stdout: `{msg}: {ExcType}: {text}\nCheck the log at {PRE_COMMIT_HOME}/pre-commit.log\n`; log file (overwritten each time) = `### version information\n\n\`\`\`\npre-commit version: X.Y.Z\ngit --version: git version …\nsys.version:\n    …\nsys.executable: …\nos.name: …\nsys.platform: …\n\`\`\`\n\n### error information\n\n\`\`\`\n{msg}: {Exc}: {text}\n\`\`\`\n\n\`\`\`\n{traceback}\`\`\`\n` (snp: same skeleton; replace python-specific keys — open question) |
| ERR-03 | :156-175 | non-ASCII message, non-UTF-8 bytes in CalledProcessError, `__str__` raising → still exits cleanly (snp: never panic while formatting errors; lossy-decode) |
| ERR-04 | :178-194 | real process, `ValueError("☃")` → exit 3; last two lines `An unexpected error has occurred: ValueError: ☃` / `Check the log at <home>/pre-commit.log` |
| ERR-05 | :197-220 | store dir read-only → stdout starts `An unexpected error has occurred: ValueError: ohai\nFailed to write to log at ` and the log body is printed to stdout instead |
| CLI-01 | main_test.py:247-299 | `_adjust_args_and_chdir`: not in git → FatalError; at root: config/files unchanged; from `foo/`: `-c cfg.yaml` → `foo/cfg.yaml`, files → `foo/f1`,`foo/f2`, `--commit-msg-filename t.txt` → `foo/t.txt`; non-existing relative config (`.pre-commit-config.yaml` default) **not** rewritten; Windows `subst` drive works |
| CLI-02 | :312-378 | commands: autoupdate, clean, gc, install, install-hooks, migrate-config, run, sample-config, uninstall, validate-config, validate-manifest, plus help, hook-impl, hazmat, try-repo, init-templatedir (argparse choice list order: `autoupdate,clean,gc,hazmat,init-templatedir,install,install-hooks,migrate-config,run,sample-config,try-repo,uninstall,validate-config,validate-manifest,help,hook-impl`) |
| CLI-03 | :349-371,431-442 | `--help`, `help` (= `--help`), `help run` (= `run --help`) exit 0, even outside a git repo |
| CLI-04 | :381-389 | `hazmat cd subdir -- cmd -- f1 f2` → tool `cd`, subdir `subdir`, cmd `['cmd','--','f1','f2']` |
| CLI-05 | :445-455 | `main([])` outside repo → `An error has occurred: FatalError: git failed. Is it installed, and are you in a Git repository directory?` + `Check the log at …` |
| CLI-06 | :458-461 | `run --hook-stage commit` → stage normalized to `pre-commit` (also `push`→`pre-push`, `merge-commit`→`pre-merge-commit`) |
| CLI-07 | observed | unknown option → exit 2, usage + `pre-commit: error: unrecognized arguments: --nope` on **stderr**; `-a` with `--files` → exit 2 `argument --files: not allowed with argument --all-files/-a`; `--version` → stdout `pre-commit 4.6.2` exit 0 (Tier B for wording, Tier A for exit code + stream) |

### 5.18 hazmat, sample-config, util, parse_shebang, envcontext, prefix

| ID | Source | Expected |
|---|---|---|
| HAZ-01 | hazmat_test.py:12-34 | `_cmd_filenames`: no `--` → exit msg ``hazmat entry must end with `--` ``; `hello world --` → cmd (hello, world), files (); last `--` splits: `hello -- arg -- f1 f2` → cmd `hello -- arg`, files f1 f2 |
| HAZ-02 | :37-41 | `cd subdir cmd -- subdir/1 not-subdir/2` → exit msg `unexpected file without prefix='subdir/': not-subdir/2` |
| HAZ-03 | :48-67 | `cd subdir <cmd> -- subdir/a subdir/b` → cwd = subdir, files `a`,`b`; stderr empty |
| HAZ-04 | :70-77 | `ignore-exit-code <cmd raising SystemExit("bye")>` → ret 0, stderr `bye\n` |
| HAZ-05 | :80-99 | `n1 <cmd> -- foo bar baz` → one invocation per file (`['foo']\n['bar']\n['baz']\n`); any failure → ret 1 |
| VAL-04 | sample_config_test.py:1-21 | `sample-config` stdout exactly: `# See https://pre-commit.com for more information\n# See https://pre-commit.com/hooks.html for more hooks\nrepos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v3.2.0\n    hooks:\n    -   id: trailing-whitespace\n    -   id: end-of-file-fixer\n    -   id: check-yaml\n    -   id: check-added-large-files\n` |
| UTL-01 | util_test.py:322-341 | CalledProcessError str: `command: ('exe',)\nreturn code: 1\nstdout:\n    output\nstderr:\n    errors` / `stdout: (none)\nstderr: (none)` (Python tuple repr of argv; indentation 4 spaces per line) |
| UTL-02 | :344-376 | `clean_path_on_failure` removes the path on any error incl. SystemExit; keeps on success |
| UTL-03 | :379-401 | running `dne` → (1, ``Executable `dne` not found``); executable file **without shebang** → ret 1, output ends with `\n` (no OSError crash) |
| UTL-04 | :404-412 | rmtree handles read-only directories (Go module cache) |
| UTL-05 | parse_shebang_test.py:125-257 | `parse_filename('herp derp derp')` → (); `#!/usr/bin/env echo` → (`echo`,); `find_executable` absolute / on PATH / missing → None / found after PATH prepend; `PATHEXT` (`.exe;.myext`) lets `run` resolve `run.myext`; `normexe` errors: ``Executable `i-dont-exist-lol` not found``, ``Executable `./i-dont-exist-lol` not found``, ``Executable `./exe` is not executable`` (posix), ``Executable `./exe` is a directory``; `normalize_cmd`: absolute passthrough, PATH lookup, shebang prepended (`(interp, path)`), PATH+shebang+env (`/usr/bin/env echo` → `(<abs echo>, <abs run>)`) |
| UTL-06 | envcontext_test.py:13-103 | patch semantics: add / override / UNSET removes / `Var('PATH')` templating with default / values computed from the **pre-patch** env (`herp: 'foo: ' + Var('foo')` sees old `foo`) / restored on exception |
| UTL-07 | prefix_test.py:20-46 | `Prefix('foo').path('')`=`foo`; `('foo','bar')`=`foo/bar`; `('foo/bar','../baz')`=`foo/baz` (normpath); `('./','bar')`=`bar`; `('./','')`=`.`; absolute second arg wins; `star('.txt')` lists matching basenames |

---

## 6. Observed output contract (pre-commit 4.6.2, this session) — what the harness compares

1. **All user-facing output of `pre-commit` goes to stdout** (hook status lines, `[INFO]/[WARNING]/[ERROR]` log lines, hook output, `An error has occurred…`). stderr is used only by argparse (usage + `error:`; exit 2) and by Python `SystemExit("msg")` paths (hook-impl arg errors, hazmat errors; exit 1). Under git hooks, git redirects the hook's stdout to **stderr**.
2. Hook subprocess output is captured with **stderr merged into stdout** (one pipe, ordering preserved); with color on, a pty is used for stdout+stderr and stdin is not a tty.
3. Exit codes: 0 success; 1 hook failure / expected error (`FatalError`, `InvalidConfigError`, validation, `[ERROR]` pre-checks); 2 CLI usage error (argparse); 3 unexpected error; 130 interrupted.
4. `- duration: Xs` uses `round(seconds, 2) or 0` rendered with Python float repr (`0s`, `0.03s`, `1.0s`, `0.77s`).
5. Store side effects: `PRE_COMMIT_HOME/{README, db.db, pre-commit.log (only after an error), patch<epoch>-<pid> (left behind after stash), repo<random>/}`.
6. Message strings that embed the tool name (`pre-commit installed at …`, ``Running in migration mode … Use -f to use only pre-commit.``, ``Cowardly refusing…``, ``pre-commit hook(s) made changes.``, ``reproduce locally with: `pre-commit run --all-files` ``, ``To run `pre-commit` as part of git workflow, use `pre-commit install`.``, ``pre-commit version N is required but version X is installed.  Perhaps run `pip install --upgrade pre-commit`.``, ``Hint: `pre-commit autoupdate` often fixes this.``, ``run: `pre-commit migrate-config` to automatically fix this.``, ``pre-commit normalizes slashes…``, ``Skipping `pre-commit`.``, ``To uninstall pre-commit run `pre-commit uninstall` ``, ``pre-commit failed to diff -- perhaps due to permissions?``) — see Open Question Q1.
7. YAML error bodies come from libyaml (e.g. `while parsing a flow node\ndid not find expected node content\n  in "<unicode string>", line 2, column 1`, `mapping values are not allowed in this context\n  in "<unicode string>", line 6, column 34`) and differ between libyaml and pure-Python PyYAML even within pre-commit → **Tier B**: compare the `==> File <path>` header and the presence of `=====> `, exit code 1; body free-form.
8. `'{pattern}' is not a valid python regex` uses Python `repr()` quoting (observed: `=====> '(?<n>foo)' is not a valid python regex`, exit 1) — Python 3.12 rejects `(?<name>…)`.

---

## 7. snp test architecture

### 7.1 Layers

| Layer | Purpose | Tooling | Runs |
|---|---|---|---|
| L0 static | lints, `#![forbid(unsafe_code)]`, `clippy::pedantic` subset, `unwrap_used/expect_used/panic/indexing_slicing` deny in non-test code, deny.toml | clippy, rustfmt, cargo-deny 0.x, cargo-audit, cargo-machete/udeps | every push |
| L1 unit | pure functions (formatting, partitioning, shuffle, regex translation, YAML typing/rewrite, schema validation, arg parsing, pre-push parsing) | `#[test]`, `rstest` 0.27, `proptest` 1.11, golden JSON vectors | every push |
| L2 integration | the `snp` binary against temp git repos (ports of §5) | `assert_cmd` 2.2, `assert_fs` 1.1, `predicates` 3.1, `insta` 1.49 (+`insta-cmd` 0.7 optional), `snp-testkit` | every push (hermetic subset) |
| L3 differential | same scenario through pre-commit 4.6.2 and snp, compare | `snp-difftest` (xtask), `similar` 3.2 for diffs | every push (curated hermetic set, ~150 scenarios); nightly (generated, thousands) |
| L4 property/fuzz | invariants + Python parity, crash-freedom | proptest, cargo-fuzz (nightly) or bolero 0.13 | push (short), nightly (long) |
| L5 toolchain/network | language backends, real remote repos | gated tests `net_*`, `tc_<lang>_*` | nightly + on-demand label |
| L6 platform | Linux/macOS/Windows × git versions | CI matrix | push (Linux), PR (all) |
| L7 performance | hyperfine e2e, divan micro, RSS | hyperfine 1.2x, divan 0.1.21 / criterion 0.8 | nightly + PR comment |

### 7.2 Unit tests per crate (suggested crate split — align names with the architecture spec)

| Crate | Must-have unit tests |
|---|---|
| `snp-cli` | clap definitions reproduce argparse semantics: `--files` is greedy (`nargs='*'` swallows following positionals, RUN-54); `-a`/`--files` mutually exclusive (exit 2); `--color` validated also when coming from `PRE_COMMIT_COLOR` (RUN-59); stage aliases (`commit`→`pre-commit`, `push`→`pre-push`, `merge-commit`→`pre-merge-commit`, CLI-06); `help`/`help <cmd>`; no subcommand ⇒ `run` (RUN-60); `_adjust_args_and_chdir` path rewriting (CLI-01) as a pure function over (cwd, toplevel, args) |
| `snp-config` | every CFG-* row; error-chain rendering (`\n==> At Config()\n==> At key: …\n=====> …`); warnings text; stage migration; `minimum_pre_commit_version` ordering; meta-hook NotAllowed; `language` alias normalization; Python `repr()` of strings in messages (`it's` → `"it's"`, `a\b` → `'a\\b'`, non-printables escaped) |
| `snp-yaml` | PyYAML (YAML 1.1) implicit typing exactly as in the §7.5.6 typing table (bools `yes/no/on/off/true/false` in lower/Title/UPPER casing only; ints `0b…`, `0x…`, leading-`0` octal (`0o17` is a **string**), `_` separators, sexagesimal `1:20`; floats need a `.` and a *signed* exponent (`1.5e3` is a string, `1.5e+3` a float), `-.5` is a string; `.inf/.nan`; `~`/`null`/empty → null; dates/timestamps; the `=` value-key tag is a constructor **error**), duplicate keys last-wins, merge keys `<<`, anchors/aliases (with alias-expansion limit — billion-laughs guard), spans for every scalar; rev-line rewrite (AUP-05..17); migrate rewrites (MIG-01..09); PyYAML-compatible dumper subset (`indent=4`, `default_flow_style=False`, `sort_keys=False`, sequence-in-mapping indentation `-   `) for AUP-09/try-repo/fixtures |
| `snp-regex` | Python-`re` compatibility layer (§7.5.3): accepts/rejects exactly what CPython 3.12 `re.compile` does for the tested corpus; `search` semantics incl. `$` before trailing `\n`, `\Z`, literal `{`, `[[`, lookbehind, backrefs, inline flags `(?i)(?m)(?s)(?x)`, `(?P<n>)`/`(?P=n)` |
| `snp-git` | output parsing: `-z` splitting (GIT-08), MERGE_MSG parsing (GIT-06), conflicted-files union, pre-push stdin parsing (`rsplit(maxsplit=3)` semantics), `no_git_env` filter (GIT-12), version detection |
| `snp-filter` | include/exclude (`search`), types/types_or/exclude_types set logic (`tags ⊇ types && (types_or empty \|\| tags ∩ types_or ≠ ∅) && tags ∩ exclude_types = ∅`), lexists filtering (broken symlinks kept), Windows backslash normalization, memoized identify; `check-hooks-apply`/`check-useless-excludes` logic |
| `snp-exec` | `partition` (XRG-03..07 + property tests), Python shuffle (XRG-16 + §5.4 vectors), `target_concurrency` env logic, retcode-by-max-abs, signal → negative code, ordered concatenation, `_environ_size`, platform max length; `shlex.split` of `entry` (+ `hook_cmd` hazmat interception XRG-19); shebang parsing & `normexe` messages (UTL-05) |
| `snp-run` | `_start_msg`/`_full_msg`/`_compute_cols` with East-Asian width (RUN-01..03); duration formatting; SKIP parsing (RUN-04); the per-hook state machine (skip → no-files → run → status → subtle lines → output block) as a pure renderer over events; diff-before/after modification detection; fail-fast decision; env exports (RUN-21) |
| `snp-stash` (or inside run) | patch file naming, apply/rollback sequence, intent-to-add handling, error message for failed diff (SFO-17) — mostly integration-tested |
| `snp-store` | default dir resolution order (STO-01), README text, `db_repo_name` (STO-10), mark-config-used semantics, read-only detection, crash-safe install (temp dir + rename + marker written last), locking |
| `snp-hooks` (in-process meta/pygrep/hazmat) | MET-*, pygrep table, HAZ-* |
| `snp-languages` | per-language pure logic from §5.10 (default-version selection with injected `which`/probe results, pyvenv.cfg parsing, health-check messages, docker mountinfo parsing & bind-path translation with the 4 verbatim fixtures, rootless JSON detection, R entry parsing) |

Unit-test rules: no network, no global env mutation (inject an `Env` trait / `BTreeMap<OsString, OsString>`), no `std::env::set_current_dir` (pass cwd explicitly), deterministic clocks (`Clock` trait), deterministic CPU count (inject).

### 7.3 Integration tests (`crates/snp/tests/it/*.rs`, one test binary)

#### 7.3.1 Layout
One integration-test target `tests/it/main.rs` with modules mirroring §5 (`run.rs`, `stash.rs`, `xargs.rs`, `install.rs`, `hook_impl.rs`, `git.rs`, `store.rs`, `repository.rs`, `autoupdate.rs`, `migrate.rs`, `try_repo.rs`, `gc_clean.rs`, `meta.rs`, `output.rs`, `cli.rs`, `hazmat.rs`, `languages/{python,node,go,rust,ruby,docker,...}.rs`). One binary = one link step (big compile-time win); nextest still isolates each test in its own process. Test names carry gating prefixes: `net_`, `tc_python_`, `tc_node_`, `tc_go_`, `tc_rust_`, `tc_ruby_`, `tc_docker_`, `tc_r_`, `tc_dart_`, `tc_perl_`, `pty_`, `posix_`, `win_`, `slow_`.

#### 7.3.2 Hermetic environment (`snp-testkit::TestEnv`)
Every spawned process (snp, git, pre-commit) gets:

| Var | Value | Why |
|---|---|---|
| (cleared env) | `Command::env_clear()` then whitelist `PATH`, `SYSTEMROOT`, `PATHEXT`, `COMSPEC`, `WINDIR`, `TEMP`/`TMP` (win) | no leakage of `SKIP`, `PRE_COMMIT_*`, `GIT_*`, `VIRTUAL_ENV`, `RUST_LOG` |
| `HOME` (+`USERPROFILE` on win) | `<root>/home` | user gitconfig/`~/.pre-commit` legacy dir |
| `XDG_CACHE_HOME` | `<root>/home/.cache` | store default |
| `PRE_COMMIT_HOME` | `<root>/store` | store |
| `GIT_CONFIG_NOSYSTEM` | `1` | ignore `/etc/gitconfig` |
| `GIT_CONFIG_GLOBAL` | `<root>/home/.gitconfig` with `[init] defaultBranch = master`, `[user] name = test / email = test@example.com`, `[commit] gpgsign = false`, `[tag] gpgsign = false`, `[core] autocrlf = false / fsmonitor = false`, `[protocol "file"] allow = always`, `[advice] detachedHead = false`, `[safe] directory = *` (Windows CI) | user's `core.pager=delta`, `merge.conflictstyle=zdiff3` etc. must not leak |
| `GIT_TEMPLATE_DIR` | `<root>/git-template` (empty) | no sample hooks (`conftest.py:230-234`) |
| `GIT_AUTHOR_NAME/EMAIL`, `GIT_COMMITTER_NAME/EMAIL` | `test` / `test@example.com` | as pre-commit's pytest env |
| `GIT_AUTHOR_DATE`, `GIT_COMMITTER_DATE` | `2020-01-01T00:00:00Z` (+1s per commit via testkit counter) | reproducible SHAs (verified: identical trees+dates ⇒ identical SHAs) |
| `GIT_ALLOW_PROTOCOL` | `file` (plus `https` for `net_` tests) | as pre-commit's pytest env |
| `TMPDIR` | `<root>/tmp` | try-repo/autoupdate temp dirs land under a known prefix |
| `LC_ALL`/`LANG` | `C.UTF-8` | git message language, sort order |
| `TZ` | `UTC` | — |
| `TERM` | unset (or `dumb` in pty tests that must not color) | color auto |
| `VIRTUALENV_NO_DOWNLOAD` | `1` | as pre-commit's pytest env |
| `PRE_COMMIT_NO_CONCURRENCY` | per test: unset (default) or `1` | both modes covered |

`<root>` = `tempfile::TempDir` created under `CARGO_TARGET_TMPDIR` (or `$SNP_TEST_TMPDIR`), **canonicalized** (macOS `/var`→`/private/var`, Windows 8.3 short names) before use. A CI guard compares `stat` of the real `~/.cache/pre-commit` and `~/.cache/snp` before/after the run (equivalent of `dont_write_to_home_directory`).

#### 7.3.3 Builder API (sketch)
```rust
let env = TestEnv::new();                                     // hermetic root
let hooks = env.fixture_repo("failing_hook_repo");            // copy+commit (§3)
let repo = env.git_repo("work")
    .config(Config::remote(&hooks).all_hooks())               // file://, rev=HEAD
    .file("foo.py", "")
    .stage(["foo.py"])
    .commit_config();
let out = env.snp(&repo).args(["run"]).env("SKIP", "x").output();
assert_eq!(out.status.code(), Some(1));
snp_snapshot!(env, out, @r"
exit_code: 1
----- stdout -----
[INFO] Initializing environment for file://[ROOT]/repos/failing_hook_repo.
Failing hook.............................................................Failed
- hook id: failing_hook
- exit code: 1

Fail
foo.py

----- stderr -----
");
```
`Fixtures` also provides: `merge_conflict()`, `conflicting_submodule()`, `commit_msg_repo()`, `prepare_commit_msg_repo(failing: bool)`, `push_example()` (HKI-06), `consuming_repo(src)`, `out_of_date(tagged: bool)`, `hook_disappearing()`, `img_staged()`, `sub_staged()` (SFO-08).

#### 7.3.4 Snapshot normalization (insta `filters`, applied in order)

| Pattern (regex) | Replacement | Reason |
|---|---|---|
| canonical `<root>` path, and its forms with `\\`, `\`, `/` separators and `file://` prefix | `[ROOT]` | temp paths |
| `<root>/store` | `[STORE]` | |
| `patch\d{9,11}-\d+` | `patch[TIME]-[PID]` | stash file name |
| `- duration: \d+(\.\d+)?s` | `- duration: [D]s` | timing (only when the test does not inject a clock) |
| `\b[0-9a-f]{40}\b` | `[SHA]` | only if dates are not fixed for that repo |
| `\[master [0-9a-f]{7}\]` / `\[master \(root-commit\) [0-9a-f]{7}\]` | `[master [SHORT]]` | git commit summary |
| `(?m)^Merge made by the '(ort\|recursive)' strategy\.$` | `Merge made by the '[STRATEGY]' strategy.` | git version |
| `( 1 file changed, 0 insertions\(\+\), 0 deletions\(-\)\| 0 files changed)` | `[FILES_CHANGED]` | macOS |
| `repo[a-z0-9_]{8}` | `repo[RAND]` | store clone dirs |
| `tmp[a-z0-9_]{8}` | `tmp[RAND]` | try-repo/autoupdate temp dirs |
| `\r\n` | `\n` | Windows |
| snp/pre-commit version strings | `[VERSION]` | |

Prefer **inline snapshots** (`@r"..."`) for anything ≤ 30 lines; file snapshots for longer (in `tests/it/snapshots/`). CI runs with `INSTA_UPDATE=no` (`CI` env makes insta fail on mismatch). Review locally: `cargo insta test --test-runner nextest --review` (requires `cargo install cargo-insta --locked`; not installed locally).

#### 7.3.5 Probe hook binary `snp-probe`
A tiny Rust bin target in `snp-testkit` (`env!("CARGO_BIN_EXE_snp-probe")` path written into configs as `entry:` — `shlex.quote`d, since `entry` is shell-split; identical absolute path for both tools in difftest), deterministic and cross-platform. Subcommands (all print to stdout, one item per line, raw bytes):

| Subcommand | Behavior | Reveals |
|---|---|---|
| `args` | print each argv element (after the subcommand) on its own line | argv fidelity, file order, `args:` handling |
| `count` | print `len(argv)` | xargs partitioning (one line per partition) |
| `env NAME...` | print `NAME=value` or `NAME unset` | env exports (RUN-21) |
| `cwd` | print canonical cwd | hook cwd (REP-04) |
| `tty` | print `stdin: <bool>\nstdout: <bool>\nstderr: <bool>` | pty/color (REP-03) |
| `exit N` | exit with N (or `signal N` on posix → kill self) | retcode handling (XRG-09/10) |
| `append TEXT -- FILES…` / `write TEXT -- FILES…` / `truncate -- FILES…` | mutate files | modification detection, stash conflicts |
| `bytes HEX` | write raw bytes to stdout | non-UTF-8 passthrough (RUN-11) |
| `interleave` | alternate writes to stdout/stderr with flushes | ordering (REP-03) |
| `sleep MS` | sleep | concurrency (XRG-11), timeouts |
| `stdin` | echo stdin | pre-push stdin forwarding |
| `fail-on NAME` | exit 1 if any file arg equals NAME | n1/partition behaviors |

Use bash fixtures from §3 only where the original test semantics depend on bash (`script` language, shebang handling); everything else uses `snp-probe` so Windows runs the same scenarios.

### 7.4 Differential harness (`snp-difftest`)

#### 7.4.1 Goals
Prove, scenario by scenario, that `snp` and pre-commit 4.6.2 produce the same observable behavior: stdout, stderr, exit code, working-tree bytes and modes, index, refs, hook files' *behavior*, config-file rewrites, and store effects (logically).

#### 7.4.2 Oracle provisioning
`cargo xtask oracle install` creates `target/oracle/pc-4.6.2/` with `uv venv --python 3.12` (fallback `python3 -m venv`) and `uv pip install pre-commit==4.6.2 identify==2.6.20 cfgv==3.5.0 PyYAML==6.0.3 virtualenv==21.14.5 nodeenv==1.11.0` (lockfile committed: `xtask/oracle/requirements-4.6.2.txt` with hashes, `--require-hashes`). Optional second oracle `pc-4.1.0`. Optional `prek==0.5.5` (PyPI) for benchmarks only. Paths are passed explicitly (`--oracle-bin`, `--snp-bin`); `PATH` lookups of `pre-commit`/`snp` are forbidden (stale `~/.cargo/bin/snp` v1.0.0 exists locally).

#### 7.4.3 Scenario format (`tests/differential/scenarios/**/*.yaml`)
```yaml
name: run/modified-file-returns-zero
since: "4.1.0"            # oracle version gate (skip if oracle older)
requires: {git: ">=2.31", net: false, tools: [], os: [linux, macos, windows], pty: false}
repos:                     # hook repos to materialize (fixtures from §3 or inline)
  hooks: {fixture: modified_file_returns_zero_repo, tags: []}
work:
  files: {foo.py: "", bar.py: ""}          # bytes allowed via !!binary
  exec: []                                 # files to chmod +x
  symlinks: {}                             # posix only
  config: |
    repos:
    -   repo: ${repo:hooks}
        rev: ${rev:hooks}
        hooks: [{id: bash_hook}, {id: bash_hook2}, {id: bash_hook3}]
  commit: [".pre-commit-config.yaml"]      # committed before steps
  stage: [foo.py, bar.py]
  unstaged: {}                             # post-staging worktree edits
  git_config: {}                           # repo-local config (e.g. core.autocrlf)
steps:
  - tool: [run]                            # argv after the tool name
    env: {}
    stdin: ""
    tty: false                             # run under a pty
  - git: [commit, -m, msg]                 # after `tool: [install]`, exercises hook-impl
compare:
  stdout: exact            # exact | lines-unordered | tierB | ignore
  stderr: exact
  exit: exact
  tree: true               # worktree bytes + exec bit + symlink targets (excl .git)
  index: true              # `git ls-files -s` (+ `--debug` not used)
  refs: true               # `git for-each-ref`, HEAD, stash list
  hooks_dir: behavior      # behavior | ignore  (file bytes always differ: INSTALL_PYTHON vs snp path)
  config_file: exact       # .pre-commit-config.yaml bytes after the step
  store: logical           # logical | ignore
```

#### 7.4.4 Execution protocol (per scenario)
1. Choose a fixed absolute root `R = $DIFF_ROOT/<scenario-hash>` (same for both tools).
2. For tool `T ∈ {oracle, snp}` **sequentially**: `rm -rf R`; materialize repos+work at `R` with the hermetic env of §7.3.2 (identical fixed `GIT_*_DATE` ⇒ identical SHAs); run steps, invoking `T`'s binary for `tool:` steps; for `git:` steps the hook scripts installed by `T` call back into `T`; capture `(stdout, stderr, exit)` per step; snapshot `tree/index/refs/config_file/store` after each step into `R.T.snapshot/`.
3. Because both runs use the same `R`, `PRE_COMMIT_HOME`, `HOME`, `TMPDIR`, absolute paths in output (`file://R/repos/hooks`, `Check the log at R/store/pre-commit.log`) are identical and need no rewriting.
4. Normalize (table below), compare, report.
5. Concurrency: different scenarios run in parallel (different `R`); the two tool runs of one scenario are sequential. Pin CPU affinity for both (`taskset -c 0-3` on Linux) so `cpu_count()` (affinity-based in Python) is identical — see §9 (Rust `available_parallelism` also honours cgroup quotas; Python does not).

#### 7.4.5 Normalization (applied to both sides)

| Item | Rule | Tier |
|---|---|---|
| durations | `- duration: <float>s` → `- duration: [D]s` | C |
| stash patch name | `patch\d+-\d+` → `patch[TP]` | C |
| `pre-commit.log` | compare only section headers (`### version information`, `### error information`), the first error line, and the presence of a fenced traceback/backtrace block | B |
| argparse vs clap usage/help | stderr: require exit code 2 + a line matching `error: ` (+ same offending option name); stdout of `--help`: ignore | B |
| YAML syntax error bodies | keep `\n==> File <p>\n` header, `=====> ` prefix; drop the body | B |
| version strings | `pre-commit \d+\.\d+\.\d+` (from `--version`, log, `minimum_pre_commit_version` messages) → `[VERSION]` — unless snp reports a pre-commit-compatible version (Q2) | B |
| tool-name strings | none if snp prints pre-commit-compatible text (Q1); otherwise a fixed substitution table snp-text→pre-commit-text | A |
| env-install chatter | lines from virtualenv/npm/go/cargo output during installs → ignore between `[INFO] Installing environment for …` and the next status line | C |
| hook scripts in `.git/hooks` | not byte-compared; behavior compared via subsequent `git:` steps; additionally assert snp's script contains a pre-commit hash marker (Q3) | — |
| store | compare logical model: set of `(repo, ref)` cloned, set of configs used, presence of env dirs per (repo, ref, language, version, deps) | B |
| autoupdate line order | exact when `--jobs 1` (default); `lines-unordered` when `-j>1` (results printed in completion order) | A/B |

#### 7.4.6 Scenario sources
1. **Curated ports** of every §5 row tagged **D** (~150 scenarios). Hermetic; run on every push.
2. **Generated** (nightly): a proptest-driven generator producing:
   - file trees: 0–200 files, names from a pool including spaces, unicode (`интервью`, `啊`), leading `-`, `#`, `[`, newline-free; types: text, binary (NUL), executable with/without shebang (`#!/usr/bin/env python3`, `#!/bin/bash`), empty, symlink (posix), broken symlink, submodule (rare);
   - git states: staged/unstaged/both, intent-to-add, deleted-staged, renamed, merge conflict (resolved/unresolved), detached HEAD;
   - configs: random top-level `files`/`exclude`/`fail_fast`/`default_stages`/`default_language_version`, 1–15 hooks across `local` (system/unsupported/script/pygrep/fail), `meta` and `file://` fixture repos; per-hook random `files`/`exclude` (from a Python-compatible regex grammar), `types`/`types_or`/`exclude_types` (from identify's tag list), `stages`, `always_run`, `pass_filenames`, `require_serial`, `verbose`, `fail_fast`, `alias`, `args` (spaces, quotes, `{}`, non-ASCII), `log_file`;
   - invocations: `run` with random flags (`-a`, `--files …`, `--hook-stage`, hook id/alias, `--from-ref/--to-ref`, `--show-diff-on-failure`, `--verbose`, `--color always|never`), env `SKIP`, `PRE_COMMIT_NO_CONCURRENCY`; `install -t …` + git operations; `autoupdate` against generated `file://` repos with random tags/commits; `migrate-config` on generated legacy configs; `validate-config` on mutated configs.
   - entries use `snp-probe` subcommands so outputs are deterministic.
3. **Corpus**: configs of real projects (pre-commit's own `.pre-commit-config.yaml`, pre-commit-hooks 6.0.0 manifest (34 hooks), prek's, plus a curated list of popular repos fetched once and committed) for `validate-config`, `migrate-config`, and `autoupdate --freeze` rewrite (with `rev:` rewritten to local `file://` mirrors so it stays hermetic).

#### 7.4.7 Divergence handling
On mismatch: write `target/difftest/<scenario>/{oracle,snp}/{stdout,stderr,exit,tree.json,...}`, a unified diff (`similar`), and a self-contained `repro.sh` (materializes the scenario, runs both tools). Generated scenarios are **shrunk** (proptest shrinking over the scenario model; fallback delta-debugging over hooks/files/steps) before reporting. A committed `tests/differential/known-divergences.yaml` lists accepted differences (each with reason + issue link); the harness fails on any unlisted divergence and on any listed divergence that no longer reproduces (to keep the list honest).

#### 7.4.8 Commands
```
cargo xtask oracle install                   # build target/oracle/pc-4.6.2
cargo build --release -p snp                 # harness uses the release binary by default
cargo xtask difftest --suite curated         # push CI
cargo xtask difftest --suite generated --cases 2000 --seed $SEED   # nightly
cargo xtask difftest --scenario run/modified-file-returns-zero --keep
```

### 7.5 Property tests (proptest 1.11; `PROPTEST_CASES` 256 on push, 10 000 nightly; regressions committed under `proptest-regressions/`)

Python-parity oracle strategy: `cargo xtask oracle vectors` runs `target/oracle/pc-4.6.2/bin/python -I xtask/oracle/gen_vectors.py` (stdlib + PyYAML only; writes `crates/*/tests/vectors/*.json`, committed). Unit tests replay the vectors (no Python in CI). Nightly runs a **live** mode: proptest generates inputs, a long-lived Python worker (`python -I worker.py`, JSON lines over stdin/stdout) answers, results compared.

#### 7.5.1 xargs partitioning (`snp-exec`)
- Model test: a direct port of `xargs.partition` (`xargs.py:76-117`) as a reference model inside the test; random `cmd` (1–5 args), `varargs` (0–500 args of 0–300 bytes incl. multibyte UTF-8), `target_concurrency` 1–64, `max_length` 10–2¹⁷; assert snp == model, including `ArgumentTooLong` cases.
- Invariants: concatenation of partitions' tails == input (order preserved); every partition starts with `cmd`; `len(tail) <= max(4, ceil(n/jobs))`; `cmd_len + Σ(len(arg)+1) + 1 <= max_length` per partition (bytes; UTF-16 units on Windows); no empty tail unless n == 0; number of partitions ≥ ceil(n / max_args).
- Executor invariants (with `snp-probe count/args`): output == concatenation in partition order regardless of completion order (`sleep` randomization); retcode == max-abs over partitions (first on ties).

#### 7.5.2 Shuffle parity
Known-answer tests: §5.4 table + first 8 `getrandbits(32)` + vectors for n ∈ {0..64, 100, 1000, 5000} (generated). Property: result is a permutation; deterministic; `require_serial` ⇒ identity.

#### 7.5.3 Regex parity (`snp-regex`)
Observed divergences between CPython 3.12 `re` and Rust engines (this session, regex 1.13.1 / fancy-regex 0.19.2):

| Pattern | Subject | Python `re.search` | `regex` crate | `fancy-regex` |
|---|---|---|---|---|
| `a{` / `x{1,` | same text | match (literal brace) | compile error | match |
| `(?<n>foo)` | `foo` | **compile error** (3.12) | match | match |
| `\.py$` | `foo.py\n` | **match** (`$` matches before final `\n`) | no match | no match |
| `\.py\Z` | `foo.py` | match | compile error | match |
| `changelog/.*(?<!\.rst)$` | `changelog/x.md` | match (used in MET-02) | compile error (no look-around) | match |
| `[[:alpha:]]` | `a` / `:]` | no match / **match** (nested-set FutureWarning; `[[:alpha:]` + `]`) | POSIX class | POSIX class |
| `\e` | — | compile error | compile error | accepted |
| `(a)\1` | `aa` | match | compile error | match |
| `[a-\d]` | — | compile error | compile error | compile error |
| `dir[\/].*\.py`, `\/`, `\-` | — | OK | OK | OK |

Therefore: snp needs its own Python-`re` **parser/validator** (reject exactly what CPython rejects → `'<repr>' is not a valid python regex`) that **translates** to the backing engine (fancy-regex, or PCRE2 whose `$` default semantics match Python): `$` (non-multiline) → `(?=\n?\z)`, `\Z` → `\z`, literal-brace cases escaped, nested-set `[[` handled as Python does, `(?P<n>…)`/`(?P=n)` mapped, `\d\w\s\b` Unicode by default (Python str patterns), `(?a)` ASCII flag, `(?x)` verbose, `(?L)` rejected, possessive/atomic only if Python ≥ 3.11 semantics are targeted.
Tests: (a) committed corpus of ≥ 2 000 (pattern, subject, expected) triples generated by the oracle from a grammar (literals incl. regex metachars, classes, ranges, anchors, quantifiers incl. lazy, groups, alternation, look-around, backrefs, inline flags, escapes) × subjects (generated paths: nested dirs, dots, unicode, trailing `\n`, empty); (b) all `files`/`exclude` patterns found in the config corpus; (c) property: translation never panics and compile errors map to the Python error/no-error outcome; (d) catastrophic-backtracking guard: fancy-regex backtrack limit → treat as error with a clear message (pre-commit would hang; document as intentional divergence).

#### 7.5.4 Filter semantics
Property: `filter(files, include, exclude)` == `[f for f in files if search(include, f) and not search(exclude, f)]` with order preserved, using the regex engine already proven in 7.5.3; types filter laws (`types` superset, `types_or` intersect-nonempty-or-empty, `exclude_types` disjoint) against `file-identify` tags; Classifier idempotence; Windows path normalization `\`→`/` only on Windows.

#### 7.5.5 YAML rewrite invariants (autoupdate + migrate-config)
Generator: configs rendered in random but valid styles (block/flow, 2/4-space indents, `-   ` vs `- `, comments before/after/inline, blank lines, CRLF/LF/mixed endings, quoted/unquoted revs, document markers `---`, BOM?, trailing whitespace) from a random config model. Properties:
1. Parse(rewrite(src)) == expected semantic transform (revs replaced / stages renamed / `sha`→`rev` / `python_venv`→`python` / list→`repos:` map).
2. Bytes outside the rewritten scalar spans are unchanged (diff confined to `rev:` lines; for migrate, to the touched scalars and the inserted `repos:` header + optional 4-space indentation).
3. Idempotence: rewrite(rewrite(x)) == rewrite(x); migrate on already-migrated → `Configuration is already migrated.` and no write (mtime unchanged).
4. Line endings: each line keeps its own `\r\n`/`\n`.
5. Fallback path: when rev lines cannot be located (`count != repos`), output == PyYAML dump format (vectors from oracle).
6. Python `str.splitlines(True)` parity: line boundaries `\n`, `\r`, `\r\n`, `\v`, `\f`, `\x1c`, `\x1d`, `\x1e`, `\x85`, `\u2028`, `\u2029` (a form feed inside a comment changes which line indices match `REV_LINE_RE` in pre-commit).
7. Differential (nightly): same inputs through `pre-commit autoupdate`/`migrate-config` with `file://` repos → byte-identical files.

#### 7.5.6 YAML 1.1 scalar typing & PyYAML quoting parity
Vectors (from oracle) of `yaml.load` type for ~500 scalars, and of `yaml_dump({'rev': s}, default_style=q)` for q ∈ {plain, `'`, `"`}. Examples generated this session (PyYAML 6.0.3, CSafeDumper):

| s | plain | `"` style | `'` style |
|---|---|---|---|
| `v1.2.3` | `v1.2.3` | `"v1.2.3"` | `'v1.2.3'` |
| `1.10` | `'1.10'` | `"1.10"` | `'1.10'` |
| `2` | `'2'` | `"2"` | `'2'` |
| `yes` / `on` / `True` / `null` / `~` | quoted `'…'` | `"…"` | `'…'` |
| `y` / `n` | `y` / `n` (not booleans in PyYAML) | | |
| `1:20` (sexagesimal) / `0x1F` / `0777` / `1_000` / `.inf` / `2020-01-01` | quoted `'…'` | | |
| `1e3` | `1e3` (PyYAML float needs a `.`; exponent must be signed) | | |
| `abc#def` | `abc#def` | | |
| `a: b` / `@at` / `` `b `` / `%p` / `!t` / `*a` / `&a` / `` (empty) / `v1.0 ` (trailing space) | quoted `'…'` | | |
| `-x` | `-x` | | |
| `'q` | `'''q'` | `"'q"` | `'''q'` |
| `"dq` | `'"dq'` | `"\"dq"` | `'"dq'` |
| `deadbeef` | `deadbeef` | | |

PyYAML load typing vectors (CSafeLoader, observed this session):

| scalar | type | | scalar | type |
|---|---|---|---|---|
| `0o17` | str | | `0b101` | int 5 |
| `017` | int 15 | | `0x1F` | int 31 |
| `1_000` | int 1000 | | `1:20` | int 80 |
| `190:20:30.15` | float 685230.15 | | `1.` | float 1.0 |
| `1.5e3` / `1e3` | **str** | | `1.5e+3` | float 1500.0 |
| `-.5` | **str** | | `0.1_0` | float 0.1 |
| `+.inf` / `.NaN` | float | | `~` / `Null` / (empty) | null |
| `yes` / `On` / `TRUE` / `Off` / `NO` | bool | | `Y` / `n` / `tRUE` | str |
| `2020-01-01` | date | | `2020-01-01 10:00:00` | datetime |
| `=` | **error**: `could not determine a constructor for the tag 'tag:yaml.org,2002:value'` (validate-config: `==> File …` + `=====> could not determine a constructor …\n  in "<unicode string>", line 2, column 8`, exit 1) | | | |

A date/datetime where a string is expected yields `Expected string got date` (cfgv uses `type(v).__name__`).

#### 7.5.7 Text-width, number and string formatting parity
- `_len_cjk`: East-Asian width (`W`,`F` → 2; `A`,`H`,`N`,`Na` → 1; **combining marks count 1**, unlike `unicode-width`). Vectors over all assigned code points from the oracle's `unicodedata` (CPython 3.12 = UCD 15.0.0; 3.13 = 15.1.0 — pick and document one). Property: dots count never negative (saturating), line width == cols when name fits.
- Duration: `round(d, 2) or 0` + Python float `repr` (`0`→`0`, `0.03`, `1.0` (not `1`), `0.125`→`0.12` (round-half-even on the binary value), `2.675`→`2.67`). Vectors for 10 000 random f64 in [0, 1000).
- Python `repr(str)` (used in `'…' is not a valid python regex`, `Expected one of … but got: 'invalid'`, `At Repository(repo='local')`): quote selection, `\\`, `\n`, `\t`, `\x7f`, non-printable → `\xNN`/`\uNNNN`; vectors.
- `shlex.split` (entry parsing, POSIX mode, no comments) and `shlex.quote`/`shlex.join` (hook template) vectors incl. quotes, backslashes, `$`, unicode, empty strings.
- Python list repr in `hook-impl … got 2: ['run', '--all-files']`.

#### 7.5.8 Small parsers
SKIP parsing (RUN-04); pre-push stdin lines (`rsplit(maxsplit=3)`: arbitrary whitespace runs, local ref containing spaces, missing fields → error not panic); hook-impl arg counts (HKI-03/04); `PRE_COMMIT_COLOR` values; envcontext patch/restore laws (UTL-06); `Prefix.path` normpath laws (UTL-07); MERGE_MSG parsing.

### 7.6 Fuzzing (crash-freedom; no panics on any input)
`cargo +nightly fuzz` (install `cargo install cargo-fuzz --locked`, 0.13.2) or `bolero` 0.13.7 (also runs as a proptest on stable). Targets: `config_load_validate` (bytes → YAML → schema; seeds: all configs in corpus + §3 manifests), `manifest_load`, `yaml_rewrite_autoupdate`, `yaml_migrate`, `py_regex_compile` (translator), `shlex_split`, `pre_push_stdin`, `merge_msg_parse`, `shebang_parse`, `git_z_split`, `pyvenv_cfg_parse`, `docker_mountinfo_parse`. Assertions: no panic, no OOM (`-rss_limit_mb=512`), YAML alias-bomb bounded (< 100 ms, < 64 MB), deep nesting bounded (recursion limit → error).

### 7.7 PTY tests (`pty_*`, posix only)
Using `expectrl` 0.9 (or `portable-pty` 0.9): (1) `snp run` with stderr attached to a pty and `TERM=xterm-256color` → colored output (auto mode keys off **stderr** isatty); `TERM=dumb` → plain; (2) `--color always` → hook child sees stdout/stderr as tty and stdin not (`snp-probe tty` → `stdin: False\nstdout: True\nstderr: True`); (3) docker_image color behavior (tc_docker). Windows: pre-commit does not use a pty (xfail) — snp must fall back to pipes; test that `--color always` on Windows still colors status lines.

### 7.8 Interoperability tests (`interop_*`, need the oracle)
| Case | Expectation |
|---|---|
| `pre-commit install` then `snp install` | snp recognizes the hash (CURRENT or PRIOR) → overwrites, no `.legacy`, no migration message |
| `snp install` then `pre-commit uninstall` | removed only if snp's template embeds a pre-commit hash marker (Q3) |
| `pre-commit install` with an existing foreign hook (→ `.legacy`), then `snp install`/`snp uninstall` | legacy preserved/restored exactly like pre-commit |
| config rewritten by `snp autoupdate`/`migrate-config` | `pre-commit validate-config` accepts it and vice versa |
| hook entries invoking `pre-commit hazmat …` / `pre-commit` subcommands inside hooks | work when only snp is installed (requires a `pre-commit` shim or entry rewriting, Q4) |
| git ≥ 2.54 config hooks: `git config hook.pc.command "snp hook-impl --hook-type pre-commit"` + `hook.pc.event pre-commit` | runs without `--hook-dir`, no legacy handling (HKI-02) |
| shared `PRE_COMMIT_HOME` between pre-commit and snp | Q5: either compatible store or snp refuses/uses a distinct subdir; test whichever is decided |

### 7.9 Real-world corpus & network tests (`net_*`)
- `net_pre_commit_hooks_e2e`: config using `https://github.com/pre-commit/pre-commit-hooks` at `v6.0.0` (34 hooks, python language: needs python + virtualenv + PyPI) on a generated worktree; run oracle and snp; compare outputs (env-install chatter Tier C).
- `net_autoupdate_github`: autoupdate against 3 pinned public repos in a throwaway config (compare chosen revs, not timing).
- Language backends with real toolchains (§5.10 "toolchain" column). Local machine has: python3.12/3.13, uv, node v24.12.0, go, ruby, perl, dart, R/Rscript, java, docker, rustup/cargo; missing: dotnet, swift, conda, julia, lua, cabal/ghc, coursier, podman, valgrind, heaptrack, hyperfine, prek, lefthook, shellcheck.

### 7.10 Platform & git-version matrix
| Axis | Values | Notes |
|---|---|---|
| OS | ubuntu-latest (x86_64 glibc), ubuntu arm64, macOS arm64, windows-latest | macOS: `/private` canonicalization, case-insensitive APFS (path filters, `check-case-conflict`), `0 files changed` git message; Windows: hooks get `#!/bin/sh` shebang, executed by Git-for-Windows bash; `PATHEXT`; backslash normalization; command-line limit `2**15-2048` UTF-16 units, `.bat/.cmd` 8192; default `core.autocrlf=true` in Git for Windows installers (hermetic gitconfig forces false except in CRLF tests); no pty; symlink tests skipped |
| git | minimum supported (decide; `--deduplicate` needs 2.31, `--filter=blob:none` partial clone 2.19+), distro (2.43), latest stable, 2.54+ (config-based hooks) | Linux CI uses container images per git version |
| libc | glibc + musl static build | identical test results expected |
| Python oracle | CPython 3.12 (vectors), 3.13 (UCD 15.1 delta check) | |

### 7.11 Benchmarks

#### 7.11.1 Tools & commands
- hyperfine is **not installed**: `cargo install hyperfine --locked` (latest 1.21.0, released 2026-10-06; prek's published numbers used 1.20.0). prek: `uv tool install prek==0.5.5` (or `cargo install --locked prek`). pre-commit oracle: `target/oracle/pc-4.6.2/bin/pre-commit`.
- Template (no shell overhead, warmup, many runs, JSON export):
```
hyperfine -N -w 5 -r 50 --export-json bench/<name>.json --export-markdown bench/<name>.md \
  -n pre-commit "$PC run" -n prek "$PREK run" -n snp "$SNP run"
```
Cold variants: `--prepare 'rm -rf <store>'`. With `-N` there is no shell, so pass env via `env PRE_COMMIT_HOME=<store> <cmd>` and use absolute paths (or drop `-N` and subtract shell overhead).
- Memory: `/usr/bin/time -v <cmd>` → "Maximum resident set size"; repeat 10×, take median. (valgrind/heaptrack/dhat not installed; `dhat` 0.3.3 crate optional for allocation profiling behind a feature flag.)
- Micro: `divan` 0.1.21 (`cargo bench -p snp-exec --bench partition`) or criterion 0.8.2; CPU profiles with `perf` (installed) + `cargo flamegraph` optional.

#### 7.11.2 Scenarios (all repos generated by `cargo xtask bench setup`, deterministic)
| ID | Scenario | What it isolates |
|---|---|---|
| B1 | `--version` | process start-up |
| B2 | `run noop-01` (1 local system hook `true`, `always_run`, `pass_filenames: false`) | fixed framework cost |
| B3 | `run` with 10 such hooks | per-hook dispatch + diff checks (10× `git diff`) |
| B4 | `run --all-files`, 1 hook `true` with filenames, repos of 1k / 10k / 50k files | file listing, regex filtering, identify, partitioning, spawn count |
| B5 | B4 + `types: [python]` / `types_or` / `exclude_types` hooks | identify cost (shebang reads for executables) |
| B6 | 13 pre-commit-hooks hooks (python, warm envs) on 960 files (prek's corpus shape) | realistic run, warm |
| B7 | B6 cold: fresh store (`--prepare 'rm -rf …'`), network | env install (report separately; not a CI gate) |
| B8 | staged + large unstaged diff (10 MB patch) | stash/restore |
| B9 | `git commit` through installed hook (hook-impl path) | end-to-end user latency |
| B10 | `validate-config` on a 1 000-hook config; `migrate-config` & `autoupdate --freeze` with 50 local `file://` repos (`-j 1` and `-j 8`) | YAML + git fetch fan-out |
| B11 | `gc` with 500 cloned repos / 200 configs | store scan |
| B12 | peak RSS for B4(50k) and B6 | memory |

#### 7.11.3 Budgets (proposed; pre-commit baselines from §0.2)
- B1 ≤ 3 ms (pre-commit 172 ms); B2 ≤ 10 ms (234 ms); B3 ≤ 25 ms (243 ms); B4(50k) ≤ 5× faster than pre-commit; B12 snp peak RSS ≤ 30 MB for 50k files.
- Never slower than pre-commit or prek on any scenario at p50 (CI gate: fail if snp median > oracle median, or regresses > 5 % vs `main` with non-overlapping 95 % CIs).

### 7.12 CI pipeline & commands

```
# toolchain (rust-toolchain.toml pins 1.98.0; local default is 1.95.0!)
rustup run 1.98.0 cargo --version
# lint
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo deny check && cargo audit
cargo machete            # or: cargo +nightly udeps
# tests (cargo-nextest 0.9.133 installed locally; latest 0.9.146)
cargo nextest run --workspace --locked                                  # hermetic default
cargo nextest run --workspace --profile nightly                         # nightly: + net_/tc_/interop_
cargo nextest run --workspace --profile nightly -E 'test(/^tc_python_/)'   # one toolchain
PRE_COMMIT_NO_CONCURRENCY=1 cargo nextest run --workspace -E 'test(/run|xargs/)'    # serial mode pass
cargo test --doc --workspace                                            # nextest does not run doctests
# snapshots
cargo insta test --test-runner nextest --check
# coverage (cargo-llvm-cov installed)
cargo llvm-cov nextest --workspace --lcov --output-path lcov.info
# mutation testing (not installed: cargo install cargo-mutants --locked)
cargo mutants --test-tool nextest -p snp-exec -p snp-run -p snp-config
# differential
cargo xtask oracle install && cargo xtask difftest --suite curated
# fuzz smoke (nightly)
cargo +nightly fuzz run config_load_validate -- -max_total_time=60
```

`.config/nextest.toml` essentials:
```toml
[profile.default]
# hermetic by default: network/toolchain tests are opt-in (override with -E or --ignore-default-filter)
default-filter = 'not (test(/^net_/) | test(/^tc_/) | test(/^interop_/))'
slow-timeout = { period = "30s", terminate-after = 4 }
retries = 0                      # flakiness is a bug
fail-fast = false

[profile.ci]
junit = { path = "junit.xml" }

[profile.nightly]
default-filter = 'all()'

[test-groups]
serial-chmod = { max-threads = 1 }   # tests that chmod store dirs / depend on umask
docker = { max-threads = 2 }

[[profile.default.overrides]]
filter = 'test(/^tc_docker_/)'
test-group = 'docker'
```
(Alternative if the nextest version lacks `default-filter`: mark gated tests `#[ignore = "net"]` and run them with `--run-ignored only`.)

---

## 8. Local tooling inventory (checked this session)

| Tool | Status | Command / note |
|---|---|---|
| cargo-nextest | 0.9.133 installed | `cargo nextest run` |
| cargo-llvm-cov, cargo-tarpaulin | installed | coverage |
| cargo-deny, cargo-audit, cargo-machete, cargo-udeps | installed | supply chain / unused deps |
| cargo-miri | binary present | limited value with `forbid(unsafe_code)`; can run pure crates under miri on nightly |
| cargo-insta | **missing** | `cargo install cargo-insta --locked` (1.49.0) |
| hyperfine | **missing** | `cargo install hyperfine --locked` |
| cargo-fuzz, cargo-mutants | **missing** | `cargo install cargo-fuzz --locked`, `cargo install cargo-mutants --locked` |
| prek | **missing** | `uv tool install prek==0.5.5` |
| perf, /usr/bin/time | installed | profiling / RSS |
| valgrind, heaptrack | missing | (gungraun/iai-callgrind need valgrind) |
| uv 0.8.8, pipx, python3.12, python3.13 | installed | oracle provisioning |
| docker | installed | docker language tests |
| node v24.12.0, go, ruby, perl, dart, R, java | installed | toolchain tests |
| git 2.43.0 | installed | below 2.54 (no config-based hooks locally) |
| Rust 1.98.0 | installed, **not default** | pin via `rust-toolchain.toml` |

Latest crate versions (crates.io, 2026-10-07): assert_cmd 2.2.2, assert_fs 1.1.4, predicates 3.1.4, insta 1.49.0, insta-cmd 0.7.0, proptest 1.11.0, tempfile 3.27.0, criterion 0.8.2, divan 0.1.21, expectrl 0.9.0, portable-pty 0.9.0, rexpect 0.7.1, similar 3.2.0, snapbox 1.2.2, trycmd 1.2.1, serial_test 4.0.1, rstest 0.27.0, test-case 3.4.0, libtest-mimic 0.8.2, datatest-stable 0.3.3, regex 1.13.1, fancy-regex 0.19.2, pcre2 0.2.11, bolero 0.13.7, arbitrary 1.5.0, which 8.0.6, fs-err 3.3.2, cfgv 0.2.0, file-identify 0.4.0 (user's crates; file-identify has a `python_compatibility_test.rs`; python `identify-cli` in the oracle venv can serve as its differential oracle: `identify-cli x.py` → `["file", "non-executable", "python", "text"]`, `--filename-only` → `["python", "text"]`).

---

## 9. Implementation notes for snp (test-driven, Rust-oriented)

### 9.1 Seams that make the catalog testable
- Inject `Env` (an immutable `BTreeMap<OsString, OsString>` snapshot taken once in `main`), `Clock` (monotonic, for `- duration:`), `CpuCount`, `Cwd`, and a `Spawner` trait (real: `std::process::Command`; fake: records argv/env/cwd and returns scripted `(code, bytes)`). Never call `std::env::set_var`/`set_current_dir` (both process-global; `set_var` is `unsafe` in edition 2024 — incompatible with `forbid(unsafe_code)` anyway).
- Build child environments explicitly (`Command::env_clear().envs(map)`), mirroring pre-commit's `envcontext` patch semantics (UTL-06) and `no_git_env` (GIT-12) for git subprocesses that clone/fetch.
- Render run output from a stream of typed events (`HookStarted{name}`, `HookFinished{status, retcode, files_modified, duration, output}` …) through a pure renderer; snapshot-test the renderer without processes.

### 9.2 Byte-exactness pitfalls (each needs a unit test)
| Pitfall | Python behavior | Rust trap |
|---|---|---|
| hook output strip | `bytes.strip()` removes `b' \t\n\r\x0b\x0c'` | `<[u8]>::trim_ascii` / `u8::is_ascii_whitespace` **do not** strip `\x0b` (VT) |
| SKIP / misc `str.strip()` | Unicode `isspace()` incl. `\x1c`–`\x1f`, `\x85`, `\xa0`, `\u2028` … | `char::is_whitespace` excludes `\x1c`–`\x1f` |
| `str.splitlines(True)` | 11 line boundaries (§7.5.5) | `str::lines()` only `\n`/`\r\n`, drops endings |
| dots in status line | `'.' * negative == ''` | `usize` underflow panic → use `saturating_sub` |
| width | `unicodedata.east_asian_width` (W/F=2, others 1, combining = 1) | `unicode-width` gives 0 for combining marks, 2 for some emoji presentation sequences |
| duration | `round(x, 2) or 0`, float `repr` (`1.0`, `0.12` for 0.125) | `format!("{}", 1.0f64)` = `1`; `(x*100.0).round()` rounds half away from zero |
| exit code display | Python `returncode` negative for signals (`-9`); on Windows large unsigned NTSTATUS values | `ExitStatus::code()` is `None` for signals (use `ExitStatusExt::signal` → negate); Windows `code()` returns negative `i32` → cast `as u32` for display |
| `repr()` in messages | `'…'` vs `"…"` quote selection, escapes | `{:?}` uses Rust escaping/double quotes |
| CalledProcessError text | `command: ('exe',)` (tuple repr; 1-tuple trailing comma), `stdout: (none)` when empty, else `stdout:\n    <indented>` | implement a `py_tuple_repr` |
| regex dialect | Python `re` (§7.5.3) | `regex` crate rejects look-around/backrefs/`\Z`/literal `{`; `$` semantics differ |
| YAML | PyYAML = YAML 1.1 typing, duplicate keys allowed, merge keys | most Rust YAML crates are 1.2 (`yes` is a string, `1:20` a string) → wrong validation results (CFG-25/26) |
| file order | `random.Random(1542676187).shuffle` when not `require_serial` | must ship an MT19937 + CPython `_randbelow` port (no `rand` crate semantics) |
| cpu count | `len(os.sched_getaffinity(0))` (ignores cgroup CPU quota) | `std::thread::available_parallelism` honours cgroup quotas → different partition sizes in containers; use affinity (e.g. `nix::sched::sched_getaffinity` or `core_affinity`) on Linux, `available_parallelism` elsewhere |
| argv limit | bytes of fs encoding (posix), UTF-16 units (win), env size subtracted once at start | compute once at start-up, same formula (§5.4) |
| `-z` paths | always `-z` + UTF-8 (`surrogateescape` in Python) | keep paths as `OsString`/bytes; never `core.quotePath` output |
| hook output channel | stderr merged into stdout via **one pipe** (ordering) | two pipes + threads reorder output (REP-03) |
| `--files` CLI | `nargs='*'` greedy | clap `num_args(0..)` + `allow_hyphen_values(false)`; positional `hook` must not be filled after `--files` |
| PRE_COMMIT_COLOR invalid | exit 2 even if `--color` not given | clap `default_value_t` from env must be validated |
| no subcommand | `run` | clap `args_conflicts_with_subcommands` + default to run |

### 9.3 Robustness requirements derived from the tests
- No panic on: non-UTF-8 hook output (RUN-11), non-UTF-8 legacy hook bytes (INS-16), latin-1 diffs and git error text (SFO-11), non-ASCII hook ids/args (RUN-26), exceptions whose messages cannot be formatted (ERR-03), read-only store (STO-12, ERR-05), dangling symlink at hook path (INS-06), missing `.git/hooks` (INS-04), executables without shebang (UTL-03), ^C during env install (REP-12: install into temp dir, write completion marker last, `rename` into place; on next run treat dirs without marker as absent).
- Stash must never lose user changes (SFO-17): write the patch file **before** touching the worktree, and keep it after restore (pre-commit leaves it in the store; tests may assert the file exists).
- `_PRE_COMMIT_SKIP_POST_CHECKOUT=1` must be set on the internal `git -c submodule.recurse=0 checkout -- .` (INS-30) and honored before config load (RUN-40).
- Hook scripts must locate the snp binary without PATH (INS-23) and print a clear message when it is gone (INS-12).
- `PRE_COMMIT_RUNNING_LEGACY` recursion guard (HKI-02) with the exact message.

### 9.4 Test-suite hygiene
- `#[cfg(test)]` helpers live in `snp-testkit` (dev-dependency only); no test-only code paths in the shipped binary except a hidden, documented `SNP_INTERNAL_*` clock override if needed — prefer injecting clocks at the library level and testing durations in unit tests; in integration tests normalize durations.
- Do **not** add a "sort filenames" test switch (prek has `PREK_INTERNAL__SORT_FILENAMES`, `prek/crates/prek/src/cli/run/filter.rs:579`): snp must reproduce pre-commit's deterministic shuffle, so outputs are already stable and identical to pre-commit's.
- Every bug fixed gets a regression test referencing the catalog ID or pre-commit issue number (pre-commit's tests cite `#186`, `#227`, `#322`, `#339`, `#397`, `#570`, `#881`, `#1010`, `#1042`, `#1173`, `#1418`, `#1972`, `#2865`).
- Keep the §5 IDs in test names (`fn run_10_failing_hook()`), and generate a coverage report of IDs ported vs total (`cargo xtask catalog-status` greps test names) — target 100 % of hermetic rows before 1.0.
- Lessons from prek (study only, no code reuse): one `TestEnv` with default insta filters for temp/home/exe paths, a `cmd_snapshot!` macro capturing `exit_code/stdout/stderr` (insta-cmd 0.7.0), `GIT_CONFIG_COUNT/KEY/VALUE` env to force `core.autocrlf=false` on git subprocesses, and a `remove_bin_from_path` helper for "tool not on PATH" tests (`prek/crates/prek/tests/common/mod.rs`).

### 9.5 Performance & memory test notes
- Benchmarks must use release builds with the shipping profile (`lto`, `codegen-units=1`, `panic` strategy as shipped) and the same binary the difftest used.
- Measure spawn counts: an integration test with a `Spawner` counter asserts upper bounds to catch accidental O(files) process spawns. pre-commit's `run` performs a constant set of git calls (`ls-files --unmerged`, `diff --quiet <config>`, `write-tree` + `diff-index` for the stash, the file-list query, one initial `git diff`) plus **one `git diff` per executed (non-skipped) hook** (`run.py:291-299`, `:202-203`) plus one spawn per xargs partition; snp's bound should be ≤ that.
- RSS regression: B12 checked nightly; fail if > 1.2× previous median.
- Avoid timing-based assertions in functional tests except XRG-11-style concurrency proofs (use generous bounds and `slow_` prefix).

---

## 10. Open questions

| # | Question | Why it matters for tests | Suggested default |
|---|---|---|---|
| Q1 | Does snp print pre-commit-branded strings (`pre-commit installed at …`, ``reproduce locally with: `pre-commit run --all-files` ``, ``Skipping `pre-commit`.`` …) verbatim, or its own branding? | determines whether differential comparison of these lines is Tier A or needs a substitution table; CI log parsers and user scripts may grep these strings | verbatim in compat mode (default when invoked as `pre-commit` via symlink/argv[0] or when `.pre-commit-config.yaml` is used); snp-branded only behind an explicit setting |
| Q2 | What version does snp report to `minimum_pre_commit_version` checks and in `--version` / log? | CFG-21, CFG-24 messages embed it; configs requiring e.g. `3.2.0` must pass | report a "pre-commit compatibility version" = 4.6.2 for checks; `--version` prints `snp X.Y.Z (pre-commit 4.6.2 compatible)` — Tier B in difftest |
| Q3 | Should snp's hook template embed pre-commit's CURRENT_HASH (`138fd403232d2ddd5efb44317e38bf03`) so `pre-commit uninstall`/`install` treat it as theirs? | interop tests §7.8; migration back to pre-commit | yes (also recognize all PRIOR_HASHES + snp's own marker) |
| Q4 | How are entries that call `pre-commit …` (e.g. `pre-commit hazmat …`, XRG-19/REP-16) handled when pre-commit is not installed? | REP-16, user configs in the wild | intercept `pre-commit hazmat` in `hook_cmd` like pre-commit does (it rewrites to `sys.executable -m pre_commit.commands.hazmat`); optionally install a `pre-commit` shim symlink |
| Q5 | Store compatibility: share `~/.cache/pre-commit` (and its `db.db` schema) or use a separate store? | STO-* port style, gc semantics, interop | separate default store (`~/.cache/snp`), but honor `PRE_COMMIT_HOME` exactly for compatibility; never write pre-commit's `db.db` |
| Q6 | Regex backend: fancy-regex + Python-syntax translator vs PCRE2 (C dependency, closer `$`/lookbehind semantics) | §7.5.3 corpus results; "no unsafe" policy (PCRE2 is FFI) | **DECIDED → 01 Addendum (gap-fill #5).** Own CPython-3.12 parser and translator. Tier 1 is `regex` (99.1 % of 4,646 real config patterns; `regex::bytes` for pygrep). Tier 2 is `fancy-regex` behind a `regex` prefilter. PCRE2 is rejected (FFI). The translated pipeline matched CPython on 4,644/4,644 corpus patterns and 18,342/18,382 fuzz patterns; the 40 misses are all IGNORECASE backrefs. |
| Q7 | Which Unicode version for East-Asian width (CPython 3.12 → UCD 15.0, 3.13 → 15.1)? | dot alignment for exotic hook names | follow the Python version used for the 4.6.2 oracle vectors (3.12 / UCD 15.0); document |
| Q8 | Minimum supported git version | GIT-10 (`--deduplicate` ≥ 2.31), partial clone flags in autoupdate | ≥ 2.31 (warn below); CI tests min + latest + 2.54 |
| Q9 | Exact replacements for Python-specific lines in `pre-commit.log` (`sys.version`, `sys.executable`) | ERR-02 Tier B rules | keep the skeleton and keys order; fill with snp/rustc/target triple equivalents |
| Q10 | Should snp emulate argparse's usage/error text byte-for-byte? | CLI-07, RUN-59 | no (Tier B); keep exit code 2 and the `error:` line semantics |
| Q11 | Catastrophic regex patterns: pre-commit can hang; should snp enforce a backtrack limit (behavioral divergence)? | regex property tests | yes, with explicit error; list in `known-divergences.yaml` |
| Q12 | Language-backend parity depth (which of the 23 languages ship in 1.0) | size of `tc_*` matrix and network tests | follow the languages spec; tests exist per §5.10 |
| Q13 | Windows `- exit code:` display for NTSTATUS-style codes (Python prints unsigned) | XRG/RUN output on Windows | print as unsigned to match |
| Q14 | Behavior when `TRAVIS` is set (pre-commit uses 2 jobs) — keep this legacy quirk? | XRG-15 and output partitioning in CI environments | keep for parity |
| Q15 | prek CLI compatibility (its `prek.toml`, workspace mode) — out of scope for the pre-commit drop-in proof? | benchmark-only use of prek | out of scope; prek used only as a benchmark reference |

---

## Appendix A. Porting checklist (counts)

| Area | Rows in §5 | Hermetic | Needs toolchain/network |
|---|---|---|---|
| Config/validation (CFG, VAL) | 33 | 33 | 0 |
| run (RUN) | 46 | 46 (RUN-23's python hook is SKIPped, so no install) | 0 |
| stash (SFO) | 17 | 17 (SFO-17 posix) | 0 |
| xargs/lang_base (XRG) | 22 | 21 | XRG-14 win |
| install/hook-impl/init-templatedir (INS, HKI, ITD) | 34 + 9 + 5 | all | 0 |
| git (GIT) | 14 | 14 | 0 |
| store (STO) | 13 | 13 | 0 |
| repository (REP) | 16 | 11 | REP-06, REP-10 (python) |
| languages (§5.10) | 145 upstream test functions (23 files) | ~60 pure-logic (mocked probes, parsers) | ~85 |
| autoupdate / migrate / try-repo / gc / meta / output / errors / CLI / hazmat / util | 17 + 9 + 7 + 8 + 8 + 5 + 5 + 7 + 5 + 7 | all | 0 |

## Appendix B. Source index (where each catalog area comes from)

`tests/clientlib_test.py` (607 lines) · `tests/commands/run_test.py` (1248) · `tests/staged_files_only_test.py` (449) · `tests/xargs_test.py` (251) · `tests/lang_base_test.py` (178) · `tests/commands/install_uninstall_test.py` (1104) · `tests/commands/hook_impl_test.py` (386) · `tests/commands/init_templatedir_test.py` (142) · `tests/git_test.py` (304) · `tests/store_test.py` (343) · `tests/repository_test.py` (519) · `tests/languages/*_test.py` (23 files) · `tests/commands/autoupdate_test.py` (534) · `tests/commands/migrate_config_test.py` (281) · `tests/yaml_rewrite_test.py` (47) · `tests/commands/try_repo_test.py` (155) · `tests/commands/gc_test.py` (175) · `tests/commands/clean_test.py` (35) · `tests/meta_hooks/*` (3 files) · `tests/output_test.py`, `tests/color_test.py`, `tests/logging_handler_test.py` · `tests/error_handler_test.py` (220) · `tests/main_test.py` (241) · `tests/commands/hazmat_test.py` (99) · `tests/commands/validate_{config,manifest}_test.py`, `sample_config_test.py` · `tests/util_test.py`, `parse_shebang_test.py`, `envcontext_test.py`, `prefix_test.py` · `tests/conftest.py` (234) · `testing/{fixtures,util,language_helpers,auto_namedtuple}.py`, `testing/resources/*` (13 repos + 3 images). Total pre-commit test code: 11 062 lines.
