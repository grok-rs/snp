# Configuration

snp reads `.pre-commit-config.yaml` (or the file named by `-c/--config`) and the `.pre-commit-hooks.yaml` manifests of
hook repositories exactly as pre-commit 4.6.2 does: the same schema, the same defaults, the same validation messages,
the same `[WARNING]` lines. pre-commit's documentation of
[the config file](https://pre-commit.com/#plugins) and of
[creating new hooks](https://pre-commit.com/#creating-new-hooks) applies unchanged and is not repeated here.

```yaml
# .pre-commit-config.yaml — valid for pre-commit and snp alike
repos:
  - repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v6.0.0
    hooks:
      - id: trailing-whitespace
      - id: end-of-file-fixer
      - id: check-yaml
  - repo: local
    hooks:
      - id: pytest
        name: pytest
        entry: pytest -q
        language: system
        types: [python]
        pass_filenames: false
```

`snp validate-config` and `snp validate-manifest` check files without running anything; `snp config dump` prints the
merged configuration (defaults, manifest values and the local overlay applied) and `snp config schema` its JSON
schema, for editor integration.

On top of that, snp understands a small set of **extension keys**, all optional. Absent keys mean exact pre-commit
behaviour.

## Sharing a config with pre-commit

Under Python pre-commit an extension key only produces pre-commit's usual warning (for hook keys when the hooks run,
for root keys when the config is loaded) and changes nothing else:

```text
[WARNING] Unexpected key(s) present on local => pytest: priority
```

so a config that uses them stays usable by both tools. Two exceptions are not portable: `repo: builtin` (pre-commit
rejects it) and extension *values* that pre-commit validates (an unknown `language` or `stages` value is an error in
pre-commit).

* `snp validate-config --portable` also lists the snp extension keys that pre-commit would warn about.
* `snp validate-config --strict` also reports unknown hook keys (pre-commit only warns about them when it runs the
  hook).
* `SNP_COMPAT_STRICT=1` makes snp itself ignore every extension, exactly as pre-commit does — useful to check what a
  teammate on pre-commit sees.

Unknown keys, including `x-*` keys, keep pre-commit's warnings in snp too.

## Hook-level extension keys

| Key | Value | Default | Effect |
|---|---|---|---|
| `priority` | integer ≥ 0, or a name from the root `priorities` map | the hook's position in the config | Lower runs first. Hooks with equal priority form a *bucket* and run concurrently on the global job pool; a bucket finishes before the next starts. Output always stays in config order. |
| `env` | map of name → string | `{}` | Environment variables for the hook, applied after the language environment (yours win). Values are literal: no `$VAR` expansion. Also applies to the `run:` conditions of `skip`/`only`. |
| `groups` | list of names | `[]` | Select hooks with `snp run --group G` (repeatable, any of), `--require-group G` (repeatable, all of) and `--no-group G` (exclusion wins). `@ungrouped` selects hooks without groups. A `--group`/`--require-group` name that no hook of the config carries fails like an unknown hook id (``No hook in group `G` (known groups: …)``, exit 1). `SNP_SKIP_GROUPS=a,b` and the overlay's `skip_groups` exclude groups. Names must be non-empty, without whitespace, not starting with `@`. |
| `stage_fixed` | `true` (= `verify`), `verify`, `trust`, `false` | `false` | Only in the `pre-commit` stage with unstaged changes stashed: files the hook modified among its staged inputs are re-staged after a check that the patch applies. `verify` re-runs a hook that exited non-zero after fixing. The status becomes `Passed` with `- files were modified by this hook (stage_fixed: staged after the last hook)`. |
| `skip` / `only` | a condition or a list of conditions | — | Skip the hook (or run it only) when the condition holds; a skipped hook prints pre-commit's yellow `Skipped` line. See [Conditions](#conditions). |
| `timeout` | duration: `30s`, `1m30s`, `500ms` | none | Wall-clock limit for the whole hook. The hook runs in its own process group and gets TERM, then KILL after 2 s; it fails with `- hook timed out after <d>` and counts for `fail_fast`. At most 24 h. |
| `fail_text` | string | — | A line `- <fail_text>` appended to the failure block, e.g. a hint how to fix. |
| `root` | relative directory | — | The hook runs in `<repo>/<root>` and receives only the files under it, relative to it. A missing directory fails the hook with `- root directory '<r>' does not exist`. |
| `use_stdin` | boolean | `false` | The hook receives a copy of the git hook's stdin (e.g. the ref lines of `pre-push`); more than 64 MiB fails closed. |
| `fail_on_changes` | `always`, `never`, `ci`, `non-ci` (or `true`/`false`) | the root value | Whether modifying files fails the hook. `ci` fails only when `CI` is set to something other than empty, `0` or `false`. |

`interactive` and `follow` are reserved and rejected with "not available in this version".

```yaml
repos:
  - repo: local
    hooks:
      - id: lint
        name: lint
        entry: make lint
        language: system
        pass_filenames: false
        priority: fast            # alias from `priorities`
        groups: [quick]
      - id: format
        name: format
        entry: cargo fmt --
        language: system
        types: [rust]
        priority: fast            # same bucket as `lint`: both run concurrently
        stage_fixed: true         # re-stage what the formatter fixed
      - id: test
        name: test
        entry: cargo test
        language: system
        pass_filenames: false
        groups: [slow]
        timeout: 10m
        env: {RUST_BACKTRACE: "1"}
        skip: [merge, rebase]     # not while merging or rebasing
        fail_text: run `cargo test` locally to reproduce
priorities:
  fast: 0
```

### Conditions

A condition is one of:

| Condition | Holds when |
|---|---|
| `merge`, `rebase`, `merge-commit`, `cherry-pick`, `revert`, `bisect` | git is in that state |
| `{ref: <glob>}` | the current branch (short name) matches the glob; `*` also matches `/` |
| `{env: NAME}` | the variable is set and non-empty |
| `{ci: true}` / `{ci: false}` | `CI` is (not) set to something other than empty, `0` or `false` |
| `{run: <command>}` | the command (`sh -c`, in the repository root, 30 s limit, cached for the run) exits 0 |

Keys of one map must all hold (AND); items of a list are alternatives (OR):

```yaml
skip:
  - merge
  - {ref: "release/*", env: HOTFIX}   # on a release branch *and* HOTFIX set
```

## Root-level extension keys

| Key | Value | Default | Effect |
|---|---|---|---|
| `priorities` | map name → integer | `{}` | Names usable in `priority`. |
| `minimum_snp_version` | version | — | Older snp versions refuse the config: `snp {v} is required but snp {installed} is installed.` (exit 1). `minimum_pre_commit_version` keeps its pre-commit meaning (snp counts as 4.6.2). |
| `fail_on_changes` | as the hook key | `always` | Default for every hook. |
| `skip` / `only` | conditions | — | Conditions for the whole run, evaluated once. |
| `allow_disable` | boolean | `true` | `false` makes snp ignore the `SNP=0` kill switch (with a note on stderr). |
| `fast_path` | `off`, `exact`, `auto` | `auto` | The native fast path for pre-commit-hooks hooks (see below). |
| `fast_path_exclude` | list of hook ids | `[]` | Hooks never run natively. |
| `fast_path_coalesce` | boolean | `false` | Native hooks run once instead of once per command-line partition; faster on huge file sets, but partition-dependent output may then differ from pre-commit's. |
| `output` | `default`, `summary`, `quiet` | `default` | `summary` adds a footer with wall time, per-hook durations and counts; `quiet` prints only failure blocks. `SNP_OUTPUT` overrides it. |

## The fast path

For hooks of [pre-commit-hooks](https://github.com/pre-commit/pre-commit-hooks) (tags v3.0.0 to v6.0.0) snp runs a
native implementation instead of creating a Python environment, when the native port is verified identical for that
tag. Output, file modifications and exit codes are those of the Python hook; the `Initializing`/`Installing` lines are
printed as pre-commit prints them. Hooks with `additional_dependencies`, a changed `entry` or `language`, or options
the port does not cover are delegated to the real Python hook automatically. `snp doctor --fast-path` explains the
decision for every hook; `--no-fast-path`, `SNP_NO_FAST_PATH=1` or `fast_path: off` disable it.

## `repo: builtin`

```yaml
repos:
  - repo: builtin
    hooks:
      - id: trailing-whitespace
      - id: check-yaml
        args: [--allow-multiple-documents]
```

Hooks of pre-commit-hooks v6.0.0 (same ids, same defaults, with upstream bugs fixed and some extra `--check` flags),
always native, without `rev` and without a clone. Seven of its 34 ids have no builtin, because they inspect Python
source with Python's own parser: `check-ast`, `check-builtin-literals`, `check-byte-order-marker`,
`check-docstring-first`, `debug-statements`, `double-quote-string-fixer` and `fix-encoding-pragma`; keep them in a
`repo: https://github.com/pre-commit/pre-commit-hooks` block with a `rev`. `entry` and `additional_dependencies` are not allowed and `language`
must be absent or `system`. pre-commit does not understand `repo: builtin`: use it only in repositories that have
switched completely. `snp migrate builtin` converts pre-commit-hooks repos to it and `snp migrate upstream` converts
back; a block that uses one of the seven ids above (or sets `language`, `entry` or `additional_dependencies`) stays
upstream, and `snp migrate builtin` prints why (`pre-commit-hooks (line N) not converted: …`).

## Local overlay

A personal, untracked file next to the config adjusts hooks for one working copy without touching the shared config:
`.pre-commit-config.local.yaml` (or `.yml`) in the config's directory, or the file named by `SNP_LOCAL_CONFIG`.

```yaml
# .pre-commit-config.local.yaml — add it to .git/info/exclude
hooks:                         # keyed by hook alias, else id; an id matching several hooks applies to all of them
  pytest:
    skip: true                 # or any condition
  mypy:
    entry: "docker compose run --rm app {entry}"   # {entry} = the merged entry of the hook
    args: ["--strict"]         # replaces the args
    env: {MYPY_CACHE_DIR: /tmp/mypy}               # merged, the overlay wins
    priority: 10               # any hook-level key except id, alias and repo
skip_groups: [slow]
fail_on_changes: never
repos: []                      # appended after the config's repos
```

* An active overlay is announced once on stderr and marked in `snp config dump`.
* It is ignored, with a note on stderr, when the file is tracked by git or when `CI` is set, unless it is named by
  `SNP_LOCAL_CONFIG`. `--no-local-config` or `SNP_LOCAL_CONFIG=0` disable it.
* An overlay entry for a hook that does not exist warns: `snp: local config overrides unknown hook '<id>'`.
* Overlay entries are applied after the manifest and the config are merged, so `{entry}` is the final entry of remote
  and builtin hooks too. An overlay that sets `language`, `entry` or `additional_dependencies` takes the hook off the
  fast path, exactly as the same key in the config would.

## Environment variables

pre-commit's variables (`SKIP`, `PRE_COMMIT_COLOR`, `PRE_COMMIT_NO_CONCURRENCY`, `PRE_COMMIT_ALLOW_NO_CONFIG`, …) keep
their meaning. The snp-only variables are listed in [Environment variables](reference/environment.md).
