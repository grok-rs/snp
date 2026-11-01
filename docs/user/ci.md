# CI integration

In CI, `snp run --show-diff-on-failure --color=always --all-files` gives the same exit code and the same hook output as
the corresponding `pre-commit run`. snp needs no Python and no pip; it does not read `CI`, `GITHUB_ACTIONS` or similar
variables unless you use an extension that is defined in terms of them (`fail_on_changes: ci`, `{ci: true}`
conditions, the local overlay being ignored in CI).

Two things always change when you switch:

* **the command** — `snp` instead of `pre-commit` (or create the alias with `snp self alias install`);
* **the cache** — snp's cache is `~/.cache/snp` (or `$SNP_HOME`), not `~/.cache/pre-commit`, and **every cache key
  must change**. With an immutable cache (GitHub Actions, Azure Pipelines, CircleCI) a key that pre-commit already
  saved is restored on a hit and never saved again, so snp's cache would never be persisted and every run would be
  cold until the config changes.

## GitHub Actions

### `snp-action`

`grok-rs/snp-action` replaces `pre-commit/action` with a one-line change; `extra_args` has the same name and default:

```yaml
- uses: actions/checkout@v4
- uses: grok-rs/snp-action@v1
  # with:
  #   extra_args: --all-files      # default, as in pre-commit/action
  #   version: latest              # or an exact release such as 2.0.0
  #   cache: "true"
```

The action downloads the release archive for the runner, verifies its SHA-256 checksum against the release's
checksum file, adds `snp` to `PATH`, caches snp's cache directory under the key described below, and runs
`snp run --show-diff-on-failure --color=always ${extra_args}`. It works on Linux, macOS and Windows runners, x86_64 and
arm64.

### Without the action

```yaml
- uses: actions/checkout@v4
- name: Install snp
  run: curl --proto '=https' --tlsv1.2 -LsSf https://github.com/grok-rs/snp/releases/latest/download/snp-installer.sh | sh
- uses: actions/cache@v4
  with:
    path: ~/.cache/snp
    key: snp-1|${{ runner.os }}|${{ runner.arch }}|${{ env.pythonLocation }}|${{ hashFiles('.pre-commit-config.yaml') }}
- run: snp run --show-diff-on-failure --color=always --all-files
```

The cache key:

* `snp-1` is the version of the key scheme (the counterpart of `pre-commit-3` in `pre-commit/action`); it differs from
  every key pre-commit used, which is what makes the first snp run save its cache;
* `runner.os` and `runner.arch` are needed because hook environments contain platform binaries;
* `pythonLocation` matters only for Python hooks set up with `actions/setup-python` (empty otherwise), as with
  pre-commit;
* do **not** put the snp version into the key: snp's cache is versioned internally, so an upgrade never misuses old
  data, and a per-release key would only force cold caches.

## Other CI systems

| CI | pre-commit recipe | With snp |
|---|---|---|
| GitLab CI | `PRE_COMMIT_HOME: ${CI_PROJECT_DIR}/.cache/pre-commit` and `cache: paths: [${PRE_COMMIT_HOME}]` | set `SNP_HOME: ${CI_PROJECT_DIR}/.cache/snp` and cache that path (GitLab caches are re-uploaded, no key change needed); replace `pre-commit` with `snp` in `script:` |
| Azure Pipelines | `Cache@2` with key `pre-commit \| .pre-commit-config.yaml \| "$(PY)"`, path `$(Pipeline.Workspace)/pre-commit-cache` | set `SNP_HOME` to the cached path and add a token to the key, e.g. `snp-1 \| .pre-commit-config.yaml \| "$(PY)"` |
| CircleCI | `save_cache` of `~/.cache/pre-commit`, key `v1-pc-cache-{{ checksum ".pre-commit-config.yaml" }}` | cache `~/.cache/snp` and change the key prefix (`snp-1-…`) |
| Travis CI, AppVeyor | cache `$HOME/.cache/pre-commit` / `%USERPROFILE%\.cache\pre-commit` | cache `$HOME/.cache/snp` / `%USERPROFILE%\.cache\snp` |
| tox / nox | `deps = pre-commit`, `commands = pre-commit run --all-files` | `deps = snp-cli`, `commands = snp run --all-files`; add `SNP_HOME` to `passenv` if you set it |

Note that `PRE_COMMIT_HOME` does not move snp's cache: recipes that set it must set `SNP_HOME` as well (or instead).

## Containers and root

A CI container running as root against a cache volume owned by another user is refused (pre-commit would happily
write root-owned files into a user's cache); set `SNP_ALLOW_FOREIGN_STORE=1` when that is intended. The static musl
archives run in any Linux image, including Alpine and distroless ones.

## pre-commit.ci

[pre-commit.ci](https://pre-commit.ci) runs Python pre-commit on its own infrastructure; snp cannot be plugged into
it, and does not need to be. snp accepts and preserves the `ci:` section of the config and keeps it intact through
`snp autoupdate` and `snp migrate-config`, and its `autoupdate` rewrites `rev:` lines byte-identically to pre-commit's,
so local `snp autoupdate` runs and pre-commit.ci's weekly autoupdate commits do not ping-pong. `ci.skip` uses hook ids,
as `SKIP` does.

As long as pre-commit.ci (or a teammate) runs Python pre-commit, keep the config valid for it: no `repo: builtin`, and
extension keys only where a `[WARNING] Unexpected key(s) present …` line under pre-commit is acceptable (see
[Configuration](configuration.md#sharing-a-config-with-pre-commit)).
