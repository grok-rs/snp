# Changelog

All notable changes to snp are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and the project uses [Semantic Versioning](https://semver.org/). A release tag `vX.Y.Z` requires a `## [X.Y.Z]`
section (`cargo xtask release gates` checks it, and the GitHub release notes are that section).

## [Unreleased]

snp 2.0.0 is a complete rewrite: a drop-in replacement for pre-commit 4.6.2. It shares only the name with the
unrelated snp 1.x releases of the crate.

### Added

- Byte-for-byte compatibility with pre-commit 4.6.2 for configs, manifests, commands, options, output, exit codes and
  side effects, in two brands chosen by the invocation name: `pre-commit` (exactly pre-commit 4.6.2) and `snp` (the
  same, named `snp`, with the snp-only commands and options). Every deliberate difference is registered in the
  [DIVERGENCES](docs/user/divergences.md) book with a differential test.
- All pre-commit commands: `run`, `install`, `uninstall`, `install-hooks`, `init-templatedir`, `autoupdate`, `gc`,
  `clean`, `try-repo`, `validate-config`, `validate-manifest`, `migrate-config`, `sample-config`, `hazmat`,
  `hook-impl`, `help`.
- snp-only commands: `doctor [--fix] [--fast-path] [--stash]`, `list`, `config dump|schema`, `cache dir|size|gc|clean`,
  `migrate lefthook|builtin|upstream`, `self alias install|uninstall|status`, `self check`, `completions`.
- Config extension keys, all optional and ignored (with pre-commit's usual warning) by Python pre-commit: `priority`
  and root `priorities` (parallel buckets), `env`, `groups` (`--group`, `--require-group`, `--no-group`,
  `SNP_SKIP_GROUPS`), `stage_fixed`, `skip`/`only` conditions, `timeout`, `fail_text`, `root`, `use_stdin`,
  `fail_on_changes`, `interactive`, `minimum_snp_version`, `allow_disable`, `fast_path*`, `output`; `repo: builtin`;
  the local overlay `.pre-commit-config.local.yaml`.
- `interactive: true` hooks (lefthook's TTY passthrough, ADR-085): the hook reads the terminal (`/dev/tty` when git
  gives it `/dev/null`) and writes to it directly, runs alone, and is announced with a `name....(interactive)` line;
  without a terminal it runs captured after a one-line note, and `SNP_NO_TTY=1` opts out. `snp migrate lefthook`
  keeps `interactive` (still dropped with `--portable`) and substitutes positional templates inside double quotes
  without extra quotes (`test -n "{2}"` → `test -n "$PRE_COMMIT_COMMIT_MSG_SOURCE"`).
- `.snp.yaml` (or `.snp.yml`) as the config file name (ADR-087): without `-c`, every command reads `.snp.yaml`, then
  `.snp.yml`, then `.pre-commit-config.yaml` from the repository root (through the `pre-commit` alias
  `.pre-commit-config.yaml` keeps priority); `.snp.yaml` beside `.pre-commit-config.yaml` wins with a one-line note,
  both snp names are an error, the hook scripts embed the file found, and its local overlay is `.snp.local.yaml`.
  `snp migrate lefthook` writes `.snp.yaml` (`.pre-commit-config.yaml` with `--portable`).
- Native implementations of the pre-commit-hooks hooks (tags v3.0.0 to v6.0.0) and of pygrep hooks, used
  automatically when they are verified identical (the fast path).
- 21 language backends with verified toolchain downloads; Python environments created with uv by default.
- snp's own cache (`$SNP_HOME`, `$XDG_CACHE_HOME/snp` or `~/.cache/snp`) with versioned, hash-keyed entries; it never
  reads or writes pre-commit's cache.
- A journaled stash of unstaged changes that is recovered after a crash or `kill -9`.
- Hook scripts that recognise and replace pre-commit, prek and snp 1.x scripts in place, embed a stable path of snp and
  run under POSIX `sh` (BusyBox included).
- Releases for Linux (glibc and static musl, x86_64 and arm64), macOS (arm64 and x86_64) and Windows (MSVC with static
  CRT, x86_64 and arm64) with shell and PowerShell installers, SHA-256 checksums, CycloneDX SBOMs, auditable binaries
  and GitHub attestations; the `snp-cli` wheel on PyPI; the `snp-action` GitHub Action.
- snp 2.x on crates.io (ADR-088): `cargo install --locked snp` (and `cargo binstall snp`, which fetches the release
  archive) installs this release; the internal crates it is built from (`snp-core`, `snp-cli`, ...) are published
  with it, each requiring exactly the same version. The crate name `snp` previously held the unrelated snp 1.x.
- A Homebrew formula in the grok-rs/homebrew-tap tap: `brew install grok-rs/tap/snp` (macOS and Linux; `snp` only,
  no `pre-commit` alias).
- User documentation: installation, migration guide, CI integration, configuration reference, troubleshooting, the CLI
  reference generated from the parser tables and the DIVERGENCES book generated from the register.
