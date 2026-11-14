# Changelog

All notable changes to snp are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and the project uses [Semantic Versioning](https://semver.org/). A release tag `vX.Y.Z` requires a `## [X.Y.Z]`
section (`cargo xtask release gates` checks it, and the GitHub release notes are that section). A kernel whose proof
is pending (docs/VERIFICATION.md §8.3) needs a line `Proof pending: <kernel id>` in that section (gate 7, ADR-096).

## [Unreleased]

Targets pre-commit 4.7.0 (was 4.6.2); the workspace version is 2.0.1 (ADR-089).

### Changed

- The compat brand is pre-commit 4.7.0: `pre-commit --version` prints `pre-commit 4.7.0`, `minimum_pre_commit_version`
  compares against 4.7.0 (a config requiring `4.7.0` is now accepted), and the `version … is installed` message and
  the crash log's `pre-commit version:` line name 4.7.0. pre-commit 4.7.0 differs from 4.6.2 only in
  `language: node` installs, which now run `npm install` without the `GIT_*` variables git exports to hooks
  (pre-commit#3774); snp already did, so no runtime behaviour changes.
- Divergence D-SEC-05 is narrowed to the pip/uv, cargo and gem installers and snp-owned repositories: the node
  installer is now parity with pre-commit.
- The differential-test oracles are `target/oracle/pc470-py312` and `pc470-py313` (`cargo xtask oracle install`, lock
  `xtask/oracle/requirements-pc470.txt`); rerun `cargo xtask oracle install` after updating.
- Windows: pygrep and the native hooks report a file they cannot open as Python's `open()` does
  (`[Errno 2] No such file or directory: 'x'`), not as a Win32 error (`[WinError 2] The system cannot find the file
  specified: 'x'`).
- Divergence D-MISC-09 is registered: a file pygrep cannot open prints the final line of Python's traceback, as snp
  has always done. No behaviour changes (ADR-090).
- `language: node` follows nodeenv 1.12.0 (was 1.11.0) for the `[nodeenv] node=` value of `setup.cfg`, `tox.ini` and
  `~/.nodeenvrc`: one pair of outer quotes is dropped (`node = "20"` selects node 20) and `%%` / `%(name)s` are
  interpolated as `configparser` does (ADR-092). The oracle lock pins `nodeenv==1.12.0`.

- Built with Rust 1.100.0; the minimum supported Rust version is 1.100 (was 1.98), so `cargo install snp` needs Rust
  1.100 or newer (ADR-094). The crash log's `sys.version` names rustc 1.100.0.

- Benchmarks (developer tooling, no runtime change): pull requests and pushes measure the base commit's snp, built
  against its own `deps.lock` crates and toolchain and primed into its own stores, in interleaved ABBA rounds with the
  head, and the regression gate pairs the rounds (`cargo xtask bench setup|run --base-snp`, ADR-099). Records are
  schema 2 (schema 1 is still read). `--counts` gates clones, `statx` and `newfstatat` base against head and shows the
  exec argv diff; B8's memory budget is snp's dhat heap peak (≤ B3's + 2 MB, hard, nightly) instead of the tree RSS,
  which is git's; the RSS column names the largest process of the tree; new soft nightly allocation ceilings; a
  `perf_patch_stream` perf-smoke test bounds the stash's resident growth on a 24 MB patch; B8's stash patches no longer
  pile up in the stores; prek's B11 store tracks its 200 configs (ADR-100).

### Fixed

- `cargo xtask bench setup` no longer fails with `destination path … already exists` when an interrupted clone left
  the pre-commit-hooks mirror directory without `HEAD`: it removes it and clones again.

- The stash no longer loses a working-tree change that `git diff` cannot see: a line a clean filter hides (for
  example `filter.<name>.clean = grep -v SECRET`) or an edit in an `--assume-unchanged` file was overwritten whenever
  another file had an unstaged change, because the stash checked out the whole tree (pre-commit 4.7.0 does the same;
  prek fixed it in j178/prek#2912). The stash now checks out only the files of its patch, and the conflict rollback
  those plus every file `git diff` shows changed, so hook fixes are still discarded as before. A failing stash
  checkout's `CalledProcessError` names the new command; recovery hints with too many files to spell out read them
  from a kept `<patch>.paths` list instead of restoring the whole worktree. New divergence D-DATA-19 (ADR-093).
  The path-limited checkout refuses a list holding an empty path or a path with a NUL byte with `ValueError` before
  calling git, since an empty `:(literal)` pathspec would select the whole tree, and the rollback list drops empty
  `git diff` records. Git-made lists never hold such a path, so no run changes in practice (D-DATA-19 amendment,
  ADR-095 K7).
- xargs partitioning no longer accepts an argument whose length overflows the command-length arithmetic: with a
  budget of `usize::MAX` the saturating sum let such an argument fit, where pre-commit's unbounded integers raise
  `ArgumentTooLongError`. The greedy loop is now the kernel `partition_lens`, which is proved to satisfy a spec transcribed
  from `xargs.partition` (ADR-095). Real budgets are at most 131072, so no run changes in practice.
- `gc` and `clean` no longer delete a stash patch that an unrestored stash journal still needs for recovery (possibly
  the only copy of unstaged work after a failed restore). The journal was searched for the patch's raw file name, so
  the patch was unprotected when the store's path is not valid UTF-8 (the journal stores it as `{"hex": …}`), when the
  journal was bigger than 1 MiB (a long intent-to-add list pushes the name past the read cap), or when the journal could
  not be read at all. Journals are now decoded with the stash's own codec and read in full; a journal that cannot be
  read or understood keeps every stash patch and `gc`/`clean` print `[WARNING] the stash journal <journal> cannot be
  read or understood; every stash patch is kept. Remove the journal once no snp run uses it and no patch it may name is
  needed.` on stderr. The `<patch>.*` files next to an unrestored patch (the `<patch>.now` backup the recovery hint
  suggests, the kept `.ita` and `.paths` lists) are kept with it. pre-commit's `gc` never looks at patches and its
  `clean` removes the whole store; divergences D-DATA-04 and D-DIAG-05 are updated.
- CI fuzzing: the fuzz shards run with a 64 MB AddressSanitizer quarantine (`ASAN_OPTIONS=quarantine_size_mb=64`,
  default 256 MB) to keep targets well below the 512 MB RSS cap they crept toward; leak detection, use-after-free
  detection and the §13.5 limits are unchanged.

### Security

- The embedded uv is 0.13.8 (was 0.12.23): uv 0.13.5 and later reject certificate chains through the compromised
  eMudhra intermediate CAs.
- The TLS trust store follows Mozilla's latest removals: webpki-roots and webpki-root-certs 1.0.12 and rustls-webpki
  0.103.17 (also in the fuzz workspace, which was still on rustls-webpki 0.103.15 without the eMudhra distrust).
- rustix 1.1.6 (RUSTSEC-2026-0486; snp calls none of the affected xattr functions).

## [2.0.0] - 2026-11-02

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
