<p align="center">
  <picture>
    <source media="(prefers-reduced-motion: reduce)" srcset="docs/assets/logo.png">
    <img src="docs/assets/logo.gif" alt="snp — You Shell Not Pass: pixel art of a wizard guarding the bridge between a developer's terminal and main. Each git commit flies to him while the fmt, lint and test hooks run: badly formatted code is re-indented on the spot and passes; a commit with a lint error and a failing test is blocked with a slam of his staff and thrown back to the terminal; the fixed commit lands on main and the branch stays green." width="960">
  </picture>
</p>

# snp — You Shell Not Pass

> *"You shall not pass!"* — every git hook, guarding your commits.

<p>
  <a href="https://github.com/grok-rs/snp/releases"><img alt="Latest release" src="https://img.shields.io/github/v/release/grok-rs/snp?label=release&color=f2c14e"></a>
  <a href="https://github.com/grok-rs/snp/actions/workflows/ci.yml"><img alt="CI status" src="https://github.com/grok-rs/snp/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://crates.io/crates/snp"><img alt="crates.io version of snp" src="https://img.shields.io/crates/v/snp?label=crates.io&color=f2c14e"></a>
  <a href="https://pypi.org/project/snp-cli/"><img alt="PyPI version of snp-cli" src="https://img.shields.io/pypi/v/snp-cli?label=PyPI%20snp-cli&color=f2c14e"></a>
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-f2c14e"></a>
</p>

**snp** is a drop-in replacement for [pre-commit](https://pre-commit.com) 4.7.0, written in Rust. It reads the same
`.pre-commit-config.yaml` and `.pre-commit-hooks.yaml`, accepts the same commands and flags, and produces the same
output and exit codes. On top of that it runs the most popular
[pre-commit-hooks](https://github.com/pre-commit/pre-commit-hooks) hooks natively, without starting Python, and offers
optional [lefthook](https://github.com/evilmartians/lefthook)-style extensions: priorities and parallel buckets,
groups, per-hook `env`, `stage_fixed`, conditions, timeouts and a local overlay config.

<sub>The latest release, v2.0.0, targets pre-commit 4.6.2; `main` and the next release, 2.0.1, target 4.7.0.</sub>

## Why snp

* **Fast.** One static binary with no Python start-up. pre-commit-hooks hooks at tags v3.0.0 to v6.0.0 run natively
  with identical output: no clone, no virtualenv. In CI, a `git commit` through the installed hook takes 11.5 ms
  against pre-commit's 121 ms, and a first run of 13 pre-commit-hooks hooks on a fresh cache 25 ms against 2.4 s
  ([Performance](#performance)).
* **Drop-in.** Invoked as `pre-commit` (an opt-in alias), snp prints exactly what pre-commit 4.7.0 prints for any
  config that uses no snp extension. Every deliberate difference is registered, justified and guarded by a test
  ([Compatibility](#compatibility)).
* **Safe with your unstaged work.** Before the hooks run, unstaged changes go into a stash that is written durably
  and journaled; if a run is killed before it puts them back, the next run says where they are and prints the command
  that restores them. snp has no `unsafe` code, does not panic on any input, and verifies its downloads.
* **Side by side.** snp keeps its own cache (`~/.cache/snp`) and never touches pre-commit's. Installing it changes
  nothing until you run `snp install` in a repository.

## Install

```sh
# Linux and macOS: release installer (verifies the checksum, installs to ~/.cargo/bin)
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/grok-rs/snp/releases/latest/download/snp-installer.sh | sh

brew install grok-rs/tap/snp        # Homebrew (macOS, Linux)
cargo binstall snp                  # release archive through cargo-binstall
cargo install --locked snp          # build from crates.io (Rust 1.100+)
pipx install snp-cli                # PyPI: the binary only (also: uv tool install snp-cli)
```

```powershell
# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -c "irm https://github.com/grok-rs/snp/releases/latest/download/snp-installer.ps1 | iex"
```

The PyPI package is `snp-cli`; `pip install snp` installs an unrelated placeholder. Archives for Linux (glibc and
static musl, x86_64 and arm64), macOS (Apple silicon and Intel) and Windows (x86_64 and arm64), with checksums, SBOMs
and build attestations, are on the [releases page](https://github.com/grok-rs/snp/releases). Every channel installs
only `snp`, never a `pre-commit` executable. See [Installation](docs/user/installation.md).

## Quickstart

In a repository that already has a `.pre-commit-config.yaml`:

```sh
snp doctor                    # which tool does git run today for each hook type?
snp install --install-hooks   # switch this repository's hook scripts to snp, build the hook environments
snp run --all-files           # exactly `pre-commit run --all-files`
git commit                    # the hooks now run through snp
```

No config yet? Start from `snp sample-config > .pre-commit-config.yaml` or copy one of the [examples](#examples).

* Scripts that call `pre-commit`: `snp self alias install` creates the opt-in `pre-commit` alias, under which snp is
  pre-commit 4.7.0 to the byte.
* GitHub Actions: replace `pre-commit/action` with `grok-rs/snp-action@v1` (same `extra_args`) and change your cache
  keys ([CI integration](docs/user/ci.md)).
* Going back: `pre-commit install` replaces snp's hook script in place ([Rollback](docs/user/migration.md#rollback)).

## Performance

Median wall time in milliseconds (lower is better), measured by the `bench` workflow's CI run on PR #76
(commit `6eaa0a08d3af`), all three tools in interleaved rounds on one GitHub-hosted Ubuntu runner:

| Scenario | snp | prek 0.5.11 | pre-commit 4.7.0 |
|---|---:|---:|---:|
| B1 `--version` | **0.72** | 1.39 | 90.50 |
| B2 run one no-op hook | **4.92** | 5.26 | 102.40 |
| B3 run 10 no-op hooks, nothing staged | **8.19** | 15.98 | 115.75 |
| B4 one hook on 50,000 files | **84.55** | 111.14 | 625.19 |
| B5 B4 with `types` filters | **78.34** | 106.91 | 602.74 |
| B6 13 pre-commit-hooks hooks, 960 files, warm cache | **15.76** | 19.35 | 649.52 |
| B7 B6 on a fresh cache | **24.61** | 687.02 | 2,404.36 |
| B8 stash a 10 MB unstaged change | **199.70** | 289.02 | 441.36 |
| B9 `git commit` through the installed hook | **11.51** | 20.46 | 120.68 |
| B10 `validate-config`, 1,000 hooks | **12.59** | 75.55 | 167.34 |
| B11 `gc`, 500 repos used by 200 configs | **7.38** | 9.23 | 145.81 |
| B12u 20 Unicode patterns, 20,000 non-ASCII paths | **233.65** | 326.48 | 832.71 |

These numbers come from one CI runner; absolute times differ from machine to machine, so compare tools within a run.
The [Performance](https://github.com/grok-rs/snp/wiki/Performance) wiki page describes the scenarios, the method
(hyperfine, hermetic fixtures, local mirrors, no network), where snp has been behind, and how to reproduce the
numbers; `bench/` holds the harness and the budgets CI enforces.

## Compatibility

snp's reference is pre-commit 4.7.0 with the dependencies it pins (cfgv 3.5.0, identify 2.6.20, PyYAML 6.0.3). Every
config pre-commit accepts is accepted with the same meaning, every one it rejects is rejected with the same message,
and the working tree, index, refs and stash list end up the same. The test corpus runs every scenario through snp and
the real pre-commit 4.7.0 and compares the bytes.

snp differs only where pre-commit crashes or hangs, loses or corrupts data, or has a security bug, or where an owner
decision forces it (its own cache, its own name, no `unsafe` code, performance, extensions that are active only when
used). Each difference has an ID, a rationale and a test that fails when it stops
reproducing:

* [Compatibility](docs/user/compatibility.md): the contract, the two brands (`snp` and `pre-commit`) and what is
  compared how;
* [DIVERGENCES](docs/user/divergences.md): every registered difference, by class;
* [`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md): the normative policy and register.

A config that uses snp extension keys stays usable by pre-commit, which only warns about the keys it does not know.
The exceptions are `repo: builtin` and extension *values* pre-commit validates, such as an unknown `language` or
`stages` value ([Sharing a config with pre-commit](docs/user/configuration.md#sharing-a-config-with-pre-commit)).
`snp validate-config --portable` lists what pre-commit would warn about.

## Examples

[`examples/`](examples/README.md) holds configs to copy, each with a README and each checked by the test suite:
[Python](examples/python/README.md), [Rust](examples/rust/README.md),
[JavaScript/TypeScript](examples/javascript-typescript/README.md), a [polyglot monorepo](examples/monorepo/README.md),
[native hooks only](examples/fast-native/README.md), [stages](examples/stages/README.md) (commit-msg, pre-push,
manual), [GitHub Actions](examples/ci-github-actions/README.md), [`repo: builtin`](examples/builtin/README.md) and the
[extension keys, migrated from lefthook](examples/extensions/README.md).

## Documentation

The user documentation lives in [`docs/user`](docs/user/README.md) (also buildable with
[mdBook](https://rust-lang.github.io/mdBook/): `mdbook build docs/user`):

* [Installation](docs/user/installation.md) and [platform support](docs/user/platforms.md)
* [Migrating from pre-commit](docs/user/migration.md): checklist, hook scripts, template directories, rollback,
  coming from prek or lefthook
* [CI integration](docs/user/ci.md): snp-action, cache keys, GitLab/Azure/CircleCI, pre-commit.ci
* [Configuration](docs/user/configuration.md): `.snp.yaml`, extension keys, the fast path, `repo: builtin`, the local
  overlay
* [Troubleshooting](docs/user/troubleshooting.md): `snp doctor`, logs, crash reports
* [CLI reference](docs/user/reference/cli.md) and [environment variables](docs/user/reference/environment.md)

The [wiki](https://github.com/grok-rs/snp/wiki) has a guided tour: getting started, how snp works inside, the fast
path and builtin hooks, and the performance record.

The design documents are the contract the implementation follows:
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) (crates, run pipeline, stash guard, store, testing),
[`docs/DECISIONS.md`](docs/DECISIONS.md) (architecture decision records), [`docs/WBS.md`](docs/WBS.md) (work
packages) and [`docs/research/`](docs/research/) (the research specifications the design cites).

## Contributing

Issues and pull requests are welcome on [GitHub](https://github.com/grok-rs/snp). A fresh clone builds on its own:
every crate is in the workspace, including `crates/snp-cfgv` (Python cfgv) and `crates/snp-identify` (Python
identify), which snp absorbed from the standalone `cfgv` and `file-identify` crates (ADR-116). The toolchain is
pinned by `rust-toolchain.toml`.

```sh
cargo build --workspace
cargo xtask help                        # list the build-time tasks
snp install --allow-missing-config      # optional: this repository's own commit hooks (cargo fmt, typos)
```

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
`target/oracle/pc470-py312` and `target/oracle/pc470-py313`. The same oracle regenerates the committed parity data of
the absorbed crates: `cargo xtask identify-tables` (`--check` to verify), `cargo xtask identify-vectors` and
`cargo xtask cfgv-vectors`. `cargo xtask docs` regenerates the CLI reference and the DIVERGENCES book.

<details>
<summary><b>Releasing</b></summary>

Releases are cut by pushing a tag `vX.Y.Z` that matches the workspace version and has a `## [X.Y.Z]` section in
[`CHANGELOG.md`](CHANGELOG.md). The `release` workflow first runs `cargo xtask release gates`: the snp checkout is clean, the
crates.io publish set (`snp` and its internal crates, `snp-cfgv` and `snp-identify` included) is pinned to the
release version (ADR-088), and the Verus and
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

</details>

## License

MIT © Serhii Kaliuzhnyi — see [LICENSE](LICENSE).
