# snp — You Shell Not Pass

> *"You shall not pass!"* — every git hook, guarding your commits.

**snp** is a Rust drop-in replacement for [pre-commit](https://pre-commit.com) 4.6.2: the same
`.pre-commit-config.yaml` and `.pre-commit-hooks.yaml`, the same commands, flags, output and exit codes,
with optional [lefthook](https://github.com/evilmartians/lefthook)-inspired extensions (priorities,
groups, per-hook `env`, `stage_fixed`, a local overlay config) and native fast paths for the most
popular hooks.

## Status

**Under construction.** The workspace skeleton (wave 0) is being built; nothing is usable yet.
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
cargo build --workspace
cargo xtask --help
```

## License

MIT © Serhii Kaliuzhnyi
