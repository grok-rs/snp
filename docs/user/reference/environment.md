# Environment variables

Variables work the same under both names of the binary (`snp` and the `pre-commit` alias). Duration and size values
are parsed with an upper bound; an invalid value is an error that names the variable.

## pre-commit's variables (unchanged meaning)

| Variable | Meaning |
|---|---|
| `SKIP` | Comma-separated hook ids (or aliases) not to run. |
| `PRE_COMMIT_COLOR` | `auto`, `always` or `never`, as `--color`. |
| `TERM` | `TERM=dumb` disables colour under `auto`. |
| `PRE_COMMIT_NO_CONCURRENCY` | When set (to anything), hooks get `jobs = 1`, as in pre-commit. |
| `TRAVIS` | When set (and `PRE_COMMIT_NO_CONCURRENCY` is not), hooks get `jobs = 2`, as in pre-commit. |
| `PRE_COMMIT_ALLOW_NO_CONFIG` | Non-empty: a hook script run without a config file skips instead of failing, whatever it was installed with (the per-invocation form of `--allow-missing-config`; the error message suggests `PRE_COMMIT_ALLOW_NO_CONFIG=1 git ...`). |
| `PRE_COMMIT_RUNNING_LEGACY` | Set for a `.legacy` hook run in migration mode (also exported by snp). Also checked on entry: a hook script that finds it already set is a recursive migration-mode script and stops with `bug: pre-commit's script is installed in migration mode`. |
| `_PRE_COMMIT_SKIP_POST_CHECKOUT` | `post-checkout` runs do nothing (set while the stash of unstaged changes is restored, as in pre-commit). |
| `PRE_COMMIT_USE_MAMBA`, `PRE_COMMIT_USE_MICROMAMBA` | Select mamba/micromamba for `language: conda`. |
| `PATH`, `PATHEXT` | Executable lookup, with pre-commit's rules. |
| `HOME`, `USERPROFILE` | Home directory (`~`). |
| `XDG_CACHE_HOME` | Parent of the default cache directory (`$XDG_CACHE_HOME/snp`). |
| `COLUMNS` | Width of help, usage and error texts (argparse); status lines are sized from the hook names as in pre-commit, never from the terminal. |
| `PRE_COMMIT_HOME` | **Not** a location for snp: it is read only to refuse using pre-commit's cache. |
| `CI` | Read only by extensions (`fail_on_changes: ci`, `{ci: …}` conditions, the local overlay). |

Deliberately ignored, for parity with pre-commit: `NO_COLOR`, `FORCE_COLOR`, `CLICOLOR_FORCE`, `RUST_LOG`, `PREK_*`.
`PYTHON_CPU_COUNT` is ignored too; pre-commit also ignores it on Linux, but on macOS and Windows under Python 3.13 or
later it uses it as the CPU count (see [D-PLAT-10](../divergences.md#d-plat-10); `SNP_JOBS` sets snp's count).
`PYTHONWARNINGS` is passed to hooks unchanged.

Exported to hooks: exactly pre-commit's set, plus `_SNP_STASH_LOCK` while snp holds the stash of unstaged changes (a
nested `snp run` inside a hook then does not deadlock), and `SNP_RUNNING_LEGACY=1` next to `PRE_COMMIT_RUNNING_LEGACY=1`
for legacy hooks.

## snp's variables

### General

| Variable | Default | Meaning |
|---|---|---|
| `SNP_HOME` | `$XDG_CACHE_HOME/snp`, else `~/.cache/snp` | The cache directory. Relative values are resolved against the directory snp was started in. |
| `SNP` | — | `0` or `false`: hook scripts installed by snp do nothing (kill switch), unless the config sets `allow_disable: false`. |
| `SNP_BIN` | — | Hook scripts run this executable instead of the embedded snp path. |
| `SNP_ALLOW_NO_CONFIG` | — | Alias of `PRE_COMMIT_ALLOW_NO_CONFIG`. |
| `SNP_RUNNING_LEGACY` | — | Set for legacy hooks (with `PRE_COMMIT_RUNNING_LEGACY`). |
| `SNP_JOBS` | the CPU count (cgroup-aware) | Size of the job pool, as `-j/--jobs`. Partitioning of file lists always follows pre-commit's rules. |
| `SNP_SKIP_GROUPS` | — | Comma-separated hook groups to exclude (see `groups`). |
| `SNP_OUTPUT` | `default` | `default`, `summary` or `quiet`, overriding the root `output` key. |
| `SNP_NO_TTY` | — | `1`: `interactive` hooks run like any other hook — no terminal, output captured (lefthook's `no_tty`). |
| `SNP_COMPAT_STRICT` | — | `1`: ignore every snp extension key, the local overlay and `.snp.yaml`, exactly as pre-commit does. |
| `SNP_LOCAL_CONFIG` | `<config dir>/.pre-commit-config.local.yaml` (`.snp.local.yaml` beside `.snp.yaml`) | Path of the local overlay; empty or `0` disables it. |
| `SNP_NO_HINTS` | — | Non-empty: no `hint: … runs …, not snp` line on `run` (it only appears when stderr is a terminal anyway). |
| `SNP_NO_FAST_PATH` | — | `1`: never run pre-commit-hooks hooks natively (as `--no-fast-path`). |
| `SNP_ALLOW_OLD_GIT` | — | `1`: no warning for git older than 2.31. |
| `SNP_ALLOW_FOREIGN_STORE` | — | `1`: allow root to use a cache directory owned by another user. |
| `SNP_PYTHON_WARNINGS` | — | `0`: do not print the Python `FutureWarning` lines that pre-commit's regex engine would print. |

### Diagnostics

| Variable | Default | Meaning |
|---|---|---|
| `SNP_LOG` | off | `error`, `warn`, `info`, `debug` or `trace`: snp's own log, on stderr (never on stdout, never mixed into hook output). `debug` logs every decision (fast path, cache, scheduling). Environment values are printed only for an allow-list of variables. |
| `SNP_LOG_FILE` | — | Write the log to this file instead of stderr. |

### Stash of unstaged changes and processes

| Variable | Default | Meaning |
|---|---|---|
| `SNP_STASH_LOCK_TIMEOUT` | `60s` (≤ 1 h) | How long a run waits for another run's stash in the same worktree before failing. |
| `SNP_STASH_VERIFY` | — | `1`: after restoring, check that the patch round-trips (warning only). |
| `SNP_KILL_GRACE` | `2s` (≤ 60 s) | Time between interrupting hooks (Ctrl-C, `timeout`) and killing them. |
| `SNP_OUTPUT_BUDGET` | `64 MiB` | Memory for captured hook output across all running hooks; output beyond it spills to an unnamed temporary file. |
| `SNP_DIFF_MODE` | — | `git`: always detect hook modifications with `git diff`, like pre-commit (disables the stat-based shortcut). |
| `SNP_VERIFY_BASELINE` | — | `1`: cross-check the shortcut against a real `git diff` (CI self-test). |

### Downloads and toolchains

| Variable | Default | Meaning |
|---|---|---|
| `SNP_OFFLINE` | — | `1`: never access the network; anything that needs a download fails. |
| `SNP_PREFETCH` | — | `0`: do not clone hook repositories in the background ahead of time. |
| `SNP_UV` | — | Path of the `uv` executable for Python environments (else next to snp, else `uv` ≥ 0.8.0 on `PATH`, else a pinned, checksum-verified download). |
| `SNP_PYTHON_INSTALLER` | `auto` | `auto`, `uv` or `pip`: how Python hook environments are created. `pip` is the escape hatch for resolver differences. |
| `SNP_PYTHON_DOWNLOADS` | — | `never`: do not download a missing Python interpreter that a hook requests. |
| `SNP_PREFER_SYSTEM_TOOLCHAINS` | — | `1`: prefer installed toolchains over downloads. |
| `SNP_NODE_MIRROR` | `https://nodejs.org/dist` | Mirror for Node.js downloads; archives are still verified against `SHASUMS256.txt` from the TLS origin. |
| `SNP_NODE_SHASUMS_URL`, `SNP_GO_JSON_URL` | the official origins | https checksum manifests for mirrors. |
| `SNP_ALLOW_INSECURE_MIRROR` | — | `1`: allow an `http://` mirror; it then only serves bytes that the TLS checksum manifest verifies. |
| `SNP_DOCKER_ALWAYS_BUILD` | — | `1`: run `docker build` for `language: docker` hooks on every run, even when the image exists. |

Standard variables honoured for downloads: `HTTPS_PROXY`, `NO_PROXY`, `SSL_CERT_FILE`, `SSL_CERT_DIR`.
