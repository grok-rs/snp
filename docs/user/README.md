# snp — You Shell Not Pass

**snp** is a drop-in replacement for [pre-commit](https://pre-commit.com) 4.6.2, written in Rust. It reads the same
`.pre-commit-config.yaml` and `.pre-commit-hooks.yaml`, accepts the same commands and options, and produces the same
output, exit codes and side effects. It also adds optional extensions inspired by
[lefthook](https://github.com/evilmartians/lefthook): hook priorities and parallel buckets, groups, per-hook `env`,
`stage_fixed`, conditions, timeouts and a local overlay config.

What you get:

* **Byte-for-byte compatibility.** For a config that uses no snp extension, invoked as `pre-commit` snp prints exactly
  what pre-commit 4.6.2 prints. Every deliberate difference is listed in the [DIVERGENCES](divergences.md) book and
  guarded by a differential test, and every one of them exists only because pre-commit crashes, loses data or has a
  security bug there, or because an owner decision forces it.
* **Speed.** One static binary with no Python start-up: `snp --version` takes a few milliseconds instead of ~170 ms, and
  a run of ten hooks with nothing staged has a 25 ms budget on Linux (pre-commit: ~240 ms). The most popular
  [pre-commit-hooks](https://github.com/pre-commit/pre-commit-hooks) hooks run natively (the *fast path*) with
  identical output.
* **Safety.** No `unsafe` code, no panic on any input, a journaled stash of unstaged changes that is restored even after
  a crash, verified downloads, contained deletion in the cache.
* **Its own cache.** snp never reads or writes pre-commit's cache (`~/.cache/pre-commit`); it keeps its own in
  `~/.cache/snp`, so both tools can be installed side by side.

## Getting started

```sh
# 1. install (see Installation for every channel)
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/grok-rs/snp/releases/latest/download/snp-installer.sh | sh
snp --version                  # snp 2.0.0

# 2. in a repository with a .pre-commit-config.yaml
snp doctor                     # which tool does git run today for each hook type?
snp install --install-hooks    # switch the hook scripts to snp and pre-build the environments
snp run --all-files            # same as `pre-commit run --all-files`
```

A repository without a config can start from `snp sample-config > .pre-commit-config.yaml`.

## Two names, two brands

The same binary behaves as one of two *brands*, chosen by the name it is invoked under:

* as **`snp`** (the default) it accepts every command and option, including the snp-only ones (`snp doctor`,
  `snp list`, `snp run --group …`), and names itself `snp` in its messages;
* as **`pre-commit`** (an alias you create with `snp self alias install`) it is pre-commit 4.6.2 to the byte: the
  snp-only commands and options are rejected exactly as pre-commit rejects unknown ones.

Installing snp therefore changes nothing for git, CI or scripts until you run `snp install` in a repository (or create
the alias). The [migration guide](migration.md) walks through the switch and the rollback.

## Where to go next

* [Installation](installation.md) — release installers, archives, PyPI, crates.io, building from source.
* [Migrating from pre-commit](migration.md) — the step-by-step checklist, hook scripts, template directories, scripts
  that call `pre-commit`, rollback.
* [CI integration](ci.md) — replacing `pre-commit/action`, cache keys, GitLab/Azure/CircleCI recipes, pre-commit.ci.
* [Configuration](configuration.md) — the config file, the extension keys, `repo: builtin` and the local overlay.
* [Troubleshooting](troubleshooting.md) — `snp doctor`, crash logs, common problems.
* Reference: [CLI](reference/cli.md), [environment variables](reference/environment.md),
  [compatibility](compatibility.md), [DIVERGENCES](divergences.md), [platform support](platforms.md).
