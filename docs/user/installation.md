# Installation

snp is a single executable named `snp` (`snp.exe` on Windows). It needs `git` on `PATH` and nothing else: no Python,
no virtualenv. Language toolchains are needed only by the hooks that use them, exactly as with pre-commit.

Every channel installs **only `snp`**. None of them creates a `pre-commit` executable, so installing snp never
silently takes over an existing pre-commit setup; the `pre-commit` name is opt-in (see
[The `pre-commit` alias](#the-pre-commit-alias)).

After installing, check:

```console
$ snp --version
snp 2.0.1
```

## Release installers (Linux, macOS, Windows)

Each [GitHub release](https://github.com/grok-rs/snp/releases) has a shell installer and a PowerShell installer. They
download the archive for your platform, verify its SHA-256 checksum and place `snp` in `$CARGO_HOME/bin` (by default
`~/.cargo/bin`), which is usually already on `PATH`.

```sh
# Linux and macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/grok-rs/snp/releases/latest/download/snp-installer.sh | sh
```

```powershell
# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -c "irm https://github.com/grok-rs/snp/releases/latest/download/snp-installer.ps1 | iex"
```

To install a specific version replace `latest/download` with `download/v2.0.0`.

## Release archives

The release also carries one archive per target, each with a `.sha256` file, plus `sha256.sum` for all of them:

| Platform | Archive |
|---|---|
| Linux x86_64 (glibc) | `snp-x86_64-unknown-linux-gnu.tar.xz` |
| Linux arm64 (glibc) | `snp-aarch64-unknown-linux-gnu.tar.xz` |
| Linux x86_64 (static, musl) | `snp-x86_64-unknown-linux-musl.tar.xz` |
| Linux arm64 (static, musl) | `snp-aarch64-unknown-linux-musl.tar.xz` |
| macOS Apple silicon | `snp-aarch64-apple-darwin.tar.xz` |
| macOS Intel | `snp-x86_64-apple-darwin.tar.xz` |
| Windows x86_64 | `snp-x86_64-pc-windows-msvc.zip` |
| Windows arm64 | `snp-aarch64-pc-windows-msvc.zip` |

The musl builds are fully static and run on any Linux distribution (Alpine, distroless containers, old glibc). The
Windows builds link the C runtime statically: no Visual C++ redistributable is needed.

```sh
curl -LO https://github.com/grok-rs/snp/releases/download/v2.0.0/snp-x86_64-unknown-linux-gnu.tar.xz
curl -LO https://github.com/grok-rs/snp/releases/download/v2.0.0/snp-x86_64-unknown-linux-gnu.tar.xz.sha256
sha256sum -c snp-x86_64-unknown-linux-gnu.tar.xz.sha256
tar xf snp-x86_64-unknown-linux-gnu.tar.xz
install -m 755 snp-x86_64-unknown-linux-gnu/snp ~/.local/bin/snp
```

### Verifying a release

Besides the checksums, every release asset has a [GitHub artifact attestation](https://docs.github.com/actions/security-for-github-actions/using-artifact-attestations)
(build provenance) and every package a CycloneDX SBOM. The binaries are built with
[cargo-auditable](https://github.com/rust-secure-code/cargo-auditable), so the exact dependency list is embedded in
them and can be checked with `cargo audit bin`:

```sh
gh attestation verify snp-x86_64-unknown-linux-gnu.tar.xz --repo grok-rs/snp
cargo audit bin ~/.local/bin/snp
```

Releases are built from a clean checkout of a tagged commit together with the two user crates snp depends on (cfgv and
file-identify) at the commits pinned in `deps.lock`; the release workflow refuses to build when those crates are not
published on crates.io from exactly those commits.

## PyPI: `snp-cli`

The `snp-cli` distribution contains the `snp` binary only (no Python code), so it works wherever Python tools are
installed today:

```sh
pipx install snp-cli
uv tool install snp-cli
uvx --from snp-cli snp run --all-files     # one-off, without installing
```

The PyPI name `snp` belongs to an unrelated, empty placeholder: `pip install snp` and `uvx snp` do **not** install this
tool.

In tox, nox or `requirements-dev.txt` you can depend on `snp-cli` and call `snp`. Do not install `snp-cli` and
`pre-commit` into the same environment if you also create the alias there (see the
[migration guide](migration.md#scripts-and-tools-that-call-pre-commit)).

## Homebrew (macOS, Linux)

```sh
brew install grok-rs/tap/snp
brew upgrade snp
```

The formula lives in the [grok-rs/homebrew-tap](https://github.com/grok-rs/homebrew-tap) tap, which the release
workflow updates for every release (pre-releases excluded). It installs the release archive for your platform
(macOS Apple silicon and Intel, Linux glibc arm64 and x86_64) and only `snp`. Hook scripts installed from a Homebrew
snp embed the stable `$(brew --prefix)/bin/snp`, so `brew upgrade` keeps commits working
([Platforms](platforms.md)).

## crates.io: `cargo install snp`

```sh
cargo install --locked snp
cargo binstall snp      # cargo-binstall fetches the release archive instead of compiling
```

snp 2.x is published on crates.io as `snp` together with the internal crates it is built from (`snp-core`,
`snp-cli`, `snp-run`, ...; they exist only to build `snp` and have no stable API). `--locked` builds with the
dependency versions the release was tested with. `cargo install` compiles with Cargo's default release profile rather
than the release archives' (fat LTO, overflow checks), and needs Rust 1.100 or newer; the installers and archives above
are the recommended channel when you do not need a source build.

The crate name `snp` previously held the unrelated snp 1.x. If `~/.cargo/bin/snp --version` prints `1.x`, that is the
old crate: `cargo install --locked snp` replaces it with 2.x (or `cargo uninstall snp` removes it). `snp doctor` warns
when the first `snp` on `PATH` is not the one you are running.

## From source

The repository depends on two sibling checkouts pinned in `deps.lock`
([cfgv-rs](https://github.com/grok-rs/cfgv-rs) and [file-identify](https://github.com/grok-rs/file-identify)):

```sh
git clone https://github.com/grok-rs/snp && cd snp
.github/scripts/deps-lock.sh checkout     # clones ../cfgv-rs and ../file-identify at the pinned commits
cargo build --profile dist --locked -p snp
./target/dist/snp --version
```

The toolchain is pinned by `rust-toolchain.toml` (Rust 1.100.0, which is also the minimum supported version).

## Shell completions

```sh
snp completions bash > ~/.local/share/bash-completion/completions/snp
snp completions zsh  > "${fpath[1]}/_snp"
snp completions fish > ~/.config/fish/completions/snp.fish
snp completions powershell >> $PROFILE
```

## The `pre-commit` alias

Scripts, IDEs and Makefiles that call `pre-commit` keep calling Python pre-commit until you decide otherwise. To make
the name run snp:

```console
$ snp self alias install
Created /home/me/.cargo/bin/pre-commit -> /home/me/.cargo/bin/snp
$ snp self alias status
pre-commit -> /home/me/.cargo/bin/pre-commit (snp alias (this snp))
```

On Unix the alias is a symbolic link, on Windows a hard link `pre-commit.exe` (or a copy across volumes). An existing
`pre-commit` that is not snp is never replaced silently: `snp self alias install` refuses, and `--force` keeps it as
`pre-commit.snp-backup`, which `snp self alias uninstall` restores. When another `pre-commit` comes earlier on `PATH`
the command warns that typing `pre-commit` will still not run snp.

Invoked through the alias, snp is in the *compat brand*: byte-identical to pre-commit 4.7.0, snp-only commands and
options rejected. See [Compatibility](compatibility.md#brands).

On Windows a hard link goes stale when an updater replaces `snp.exe`; `snp self alias status` and `snp doctor` report
it, and `snp self alias install --force` refreshes it.

## Uninstalling

1. In each repository, switch the hook scripts back (`pre-commit install` with Python pre-commit) or remove them
   (`snp uninstall`); see [Rollback](migration.md#rollback).
2. `snp self alias uninstall` if you created the alias.
3. `snp clean` removes snp's cache (`~/.cache/snp` or `$SNP_HOME`).
4. Remove the binary: `rm ~/.cargo/bin/snp` (shell installer), `brew uninstall snp`, `cargo uninstall snp`,
   `pipx uninstall snp-cli` or `uv tool uninstall snp-cli`.
