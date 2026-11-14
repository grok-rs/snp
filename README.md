<p align="center">
  <picture>
    <source media="(prefers-reduced-motion: reduce)" srcset="docs/assets/logo.png">
    <img src="docs/assets/logo.gif" alt="snp — You Shell Not Pass: pixel art of a wizard guarding the bridge between a developer's terminal and main. Each git commit flies to him while the fmt, lint and test hooks run: badly formatted code is re-indented on the spot and passes; a commit with a lint error and a failing test is blocked with a slam of his staff and thrown back to the terminal; the fixed commit lands on main and the branch stays green." width="960">
  </picture>
</p>

# snp — You Shell Not Pass

> *"You shall not pass!"* — every git hook, guarding your commits.

**snp** is a Rust drop-in replacement for [pre-commit](https://pre-commit.com) 4.7.0: the same
`.pre-commit-config.yaml` and `.pre-commit-hooks.yaml`, the same commands, flags, output and exit codes, with optional
[lefthook](https://github.com/evilmartians/lefthook)-inspired extensions (priorities and parallel buckets, groups,
per-hook `env`, `stage_fixed`, conditions, timeouts, a local overlay config) and native fast paths for the most popular
hooks.

* **Compatible to the byte.** Invoked as `pre-commit`, snp prints exactly what pre-commit 4.7.0 prints for any config
  that uses no snp extension. Every deliberate difference is registered, justified (pre-commit crashes, loses data or
  has a security bug there, or an owner decision forces it) and guarded by a differential test:
  [DIVERGENCES](docs/user/divergences.md).
* **Fast.** One static binary, no Python start-up; pre-commit-hooks hooks run natively with identical output.
* **Safe.** No `unsafe` code, no panics on any input, a journaled stash of unstaged changes that survives crashes,
  verified downloads.
* **Side by side.** snp has its own cache (`~/.cache/snp`) and never touches pre-commit's; installing it changes
  nothing until you run `snp install`.

## Install

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/grok-rs/snp/releases/latest/download/snp-installer.sh | sh
# or: brew install grok-rs/tap/snp  |  cargo install --locked snp  |  pipx install snp-cli  |  uv tool install snp-cli
snp --version   # snp 2.0.1
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/grok-rs/snp/releases/latest/download/snp-installer.ps1 | iex"
```

Archives for Linux (glibc and static musl, x86_64 and arm64), macOS (Apple silicon and Intel) and Windows (x86_64 and
arm64) with checksums, SBOMs and build attestations are on the [releases page](https://github.com/grok-rs/snp/releases).
See [Installation](docs/user/installation.md).

## Use

```sh
snp doctor                    # which tool does git run today for each hook type?
snp install --install-hooks   # switch this repository's hook scripts to snp
snp run --all-files           # exactly `pre-commit run --all-files`
```

snp reads your `.pre-commit-config.yaml` as it is. A repository that uses snp only can name it `.snp.yaml` instead
(snp looks for `.snp.yaml`, `.snp.yml`, then `.pre-commit-config.yaml`): [Config file name](docs/user/configuration.md#config-file-name).

To keep scripts that call `pre-commit` working, create the opt-in alias: `snp self alias install`. Under that name snp
behaves as pre-commit 4.7.0 to the byte.

In GitHub Actions, replace `pre-commit/action` with `grok-rs/snp-action` (same `extra_args`) and change your cache
keys: [CI integration](docs/user/ci.md).

## Documentation

The user documentation lives in [`docs/user`](docs/user/README.md):

* [Installation](docs/user/installation.md)
* [Migrating from pre-commit](docs/user/migration.md) — checklist, hook scripts, template directories, rollback
* [CI integration](docs/user/ci.md) — snp-action, cache keys, GitLab/Azure/CircleCI, pre-commit.ci
* [Configuration](docs/user/configuration.md) — `.snp.yaml`, extension keys, `repo: builtin`, the local overlay
* [Troubleshooting](docs/user/troubleshooting.md) — `snp doctor`, logs, crash reports
* [CLI reference](docs/user/reference/cli.md) and [environment variables](docs/user/reference/environment.md)
* [Compatibility](docs/user/compatibility.md), [DIVERGENCES](docs/user/divergences.md) and
  [platform support](docs/user/platforms.md)

`cargo xtask docs` regenerates the CLI reference and the DIVERGENCES book and checks every link; the pages also build
with [mdBook](https://rust-lang.github.io/mdBook/) (`mdbook build docs/user`).

The design documents are the contract the implementation follows:

* [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — crates, public APIs, run pipeline, stash guard, store, testing
* [`docs/DECISIONS.md`](docs/DECISIONS.md) — architecture decision records
* [`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md) — parity policy, branding and the register of deliberate divergences
* [`docs/WBS.md`](docs/WBS.md) — work packages and waves
* [`docs/research/`](docs/research/) — the research specifications the design cites

## Building

The workspace depends on two sibling checkouts pinned in [`deps.lock`](deps.lock):
[`cfgv-rs`](https://github.com/grok-rs/cfgv-rs) and
[`file-identify`](https://github.com/grok-rs/file-identify) (branch `snp-support`), expected at
`../cfgv-rs` and `../file-identify`. The toolchain is pinned by `rust-toolchain.toml`.

```sh
.github/scripts/deps-lock.sh checkout   # clone the two siblings at the pinned commits (skips existing ones)
cargo build --workspace
cargo xtask help                        # list the build-time tasks
```

In a linked git worktree (for example an agent's `.claude/worktrees/<name>`), `../cfgv-rs` resolves next to the
worktree, not next to the main checkout. `.github/scripts/deps-lock.sh link` run inside the worktree symlinks the
main checkout's siblings there (for `.claude/worktrees/<name>`: `.claude/worktrees/cfgv-rs` and
`.claude/worktrees/file-identify`).

The gates every change must pass (the `ci` workflow runs them on Linux, macOS and Windows; the `platform` workflow
adds the full OS × git-version matrix):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-features --lib --bins -- -D warnings
cargo clippy --workspace --all-features --tests --benches --examples -- \
    -D warnings -A clippy::disallowed_macros -A clippy::arithmetic_side_effects
cargo +1.100.0 check --workspace --all-targets      # MSRV
cargo deny check && cargo machete
.github/scripts/deny-fixture.sh                     # deny.toml still bans regex/which/tokio in snp-pycompat
cargo xtask check-dag && cargo xtask check-unsafe && cargo xtask lint-guards
cargo xtask docs --check                            # generated docs current, no broken links
cargo build --workspace --bins --features snp/testing
cargo nextest run --workspace --features snp/testing
```

The differential tests need the pre-commit 4.7.0 oracles: `cargo xtask oracle install` (requires `uv`) creates
`target/oracle/pc470-py312` and `target/oracle/pc470-py313`.

## Releasing

Releases are cut by pushing a tag `vX.Y.Z` that matches the workspace version and has a `## [X.Y.Z]` section in
[`CHANGELOG.md`](CHANGELOG.md). The `release` workflow first runs `cargo xtask release gates` (ADR-061): `deps.lock`
pins exactly the user crates, the sibling checkouts are clean at the pinned commits, the snp checkout is clean, and
cfgv and file-identify are published on crates.io from exactly those commits (`.cargo_vcs_info.json`), and the
crates.io publish set (`snp` and its internal crates) is pinned to the release version (ADR-088), and the Verus and
Lean proofs re-verify the tagged commit, any proof-pending kernel being accepted by a `Proof pending: <kernel id>`
line in the release's CHANGELOG section (gate 7, ADR-096); `cargo publish --workspace --dry-run` then builds every
crate to be published. It builds the eight targets with cargo-dist
(auditable binaries, CycloneDX SBOMs, SHA-256 checksums, GitHub attestations, the Homebrew formula), the `snp-cli`
wheels with maturin, publishes the wheels to PyPI and the GitHub release, and then publishes the crates to crates.io
(`CARGO_REGISTRY_TOKEN`) and the formula to [grok-rs/homebrew-tap](https://github.com/grok-rs/homebrew-tap)
(`HOMEBREW_TAP_DEPLOY_KEY`); pre-releases skip PyPI, crates.io and Homebrew. A manual run (`workflow_dispatch`) with a tag must
be dispatched from that tag ("Use workflow from: Tags / vX.Y.Z"), never from a branch; with the default input
`dry-run` it builds everything and publishes nothing. `packaging/` holds the wheel project and the `snp-action`
composite action.

## License

MIT © Serhii Kaliuzhnyi
