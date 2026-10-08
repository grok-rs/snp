# snp — You Shell Not Pass

> *"You shall not pass!"* — every git hook, guarding your commits.

**snp** is a Rust drop-in replacement for [pre-commit](https://pre-commit.com) 4.6.2: the same
`.pre-commit-config.yaml` and `.pre-commit-hooks.yaml`, the same commands, flags, output and exit codes,
with optional [lefthook](https://github.com/evilmartians/lefthook)-inspired extensions (priorities,
groups, per-hook `env`, `stage_fixed`, a local overlay config) and native fast paths for the most
popular hooks.

## Status

**Under construction.** The workspace skeleton (wave 0) is in place; the commands are not implemented yet.
Do not install snp over a working pre-commit setup.

## Documentation

The design is the contract the implementation follows:

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — crates, public APIs, run pipeline, stash guard, store, testing
- [`docs/DECISIONS.md`](docs/DECISIONS.md) — architecture decision records
- [`docs/COMPATIBILITY.md`](docs/COMPATIBILITY.md) — parity policy, branding and the register of deliberate divergences
- [`docs/WBS.md`](docs/WBS.md) — work packages and waves
- [`docs/research/`](docs/research/) — the research specifications the design cites

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

The gates every change must pass (the `ci` workflow runs them on Linux, macOS and Windows):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-features --lib --bins -- -D warnings
cargo clippy --workspace --all-features --tests --benches --examples -- \
    -D warnings -A clippy::disallowed_macros -A clippy::arithmetic_side_effects
cargo +1.98.0 check --workspace --all-targets       # MSRV
cargo deny check && cargo machete
.github/scripts/deny-fixture.sh                     # deny.toml still bans regex/which/tokio in snp-pycompat
cargo xtask check-dag && cargo xtask check-unsafe && cargo xtask lint-guards
cargo build --workspace --bins --features snp/testing
cargo nextest run --workspace --features snp/testing
```

The differential tests need the pre-commit 4.6.2 oracles: `cargo xtask oracle install` (requires `uv`) creates
`target/oracle/pc462-py312` and `target/oracle/pc462-py313`.

## License

MIT © Serhii Kaliuzhnyi
