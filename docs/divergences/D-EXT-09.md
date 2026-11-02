# D-EXT-09

the config file name: without `-c`, `.snp.yaml`/`.snp.yml` in the repository root are read (snp brand: before
`.pre-commit-config.yaml`, with the stderr note `snp: using .snp.yaml; ignoring .pre-commit-config.yaml (pass -c/--config
to choose)`; compat brand: only when `.pre-commit-config.yaml` is absent, silently); both snp names ⇒ `FatalError: both
.snp.yaml and .snp.yml exist; …`; their overlay is `.snp.local.y[a]ml`; `SNP_COMPAT_STRICT=1` turns it off (ADR-087)

* **Class:** EXT (COMPATIBILITY §5.7)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-EXT-09.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by ADR-087 (`snp_config::discover`, `snp_cli::ConfigArg`; the commands discover through
  `snp_run::common::resolve_config` and `snp_cmd::frame::git_frame`; tests in `crates/snp/tests/it/config_discovery.rs`).

## Register entry (COMPATIBILITY §5.7)

| Divergence |
|---|
| the config file name: without `-c`, `.snp.yaml`/`.snp.yml` in the repository root are read (snp brand: before `.pre-commit-config.yaml`, with the stderr note `snp: using .snp.yaml; ignoring .pre-commit-config.yaml (pass -c/--config to choose)`; compat brand: only when `.pre-commit-config.yaml` is absent, silently); both snp names ⇒ `FatalError: both .snp.yaml and .snp.yml exist; …`; their overlay is `.snp.local.y[a]ml`; `SNP_COMPAT_STRICT=1` turns it off (ADR-087) |

## Rationale

pre-commit reads `.pre-commit-config.yaml` and nothing else unless `-c` names another file. snp also accepts a file
named after itself, so a repository that uses snp's extensions (or was converted by `snp migrate lefthook`) can say so
in its file name. Without `-c`, every command that reads the project's config looks in the repository root:

| Present in the root | snp brand reads | compat brand (`pre-commit`) reads |
|---|---|---|
| `.pre-commit-config.yaml` only | `.pre-commit-config.yaml` | `.pre-commit-config.yaml` |
| `.snp.yaml` (or `.snp.yml`) only | `.snp.yaml` | `.snp.yaml` |
| `.snp.yaml` and `.pre-commit-config.yaml` | `.snp.yaml`, with one stderr note | `.pre-commit-config.yaml`, silently |
| `.snp.yaml` and `.snp.yml` | error (exit 1) | error, unless `.pre-commit-config.yaml` exists |
| none | `.pre-commit-config.yaml` (missing: pre-commit's error, plus D-DIAG-06) | `.pre-commit-config.yaml` (missing: pre-commit's error) |

`.pre-commit-config.yaml` keeps pre-commit's lookup (the invocation cwd first, then the root). The installed hook
scripts embed the file found (`--config=.snp.yaml`); the overlay beside `.snp.yaml` is `.snp.local.yaml` (then
`.snp.local.yml`); `snp migrate lefthook` writes `.snp.yaml` unless `--portable`. An explicit `-c` always wins.

Why it is safe: the compat brand changes only where pre-commit would fail (no `.pre-commit-config.yaml`), so a
repository that works with pre-commit gets pre-commit's bytes from the `pre-commit` alias; the snp brand reads an snp
name only when one exists, and says so when it hides a `.pre-commit-config.yaml`. `SNP_COMPAT_STRICT=1` restores
pre-commit's lookup.

## Examples

A repository with only `.snp.yaml`, `run --all-files`: pre-commit 4.7.0 prints `An error has occurred:
InvalidConfigError: ` / `=====> .pre-commit-config.yaml is not a file` (exit 1); snp, in both brands, runs the hooks of
`.snp.yaml`. With `.pre-commit-config.yaml` beside it, the snp brand prints on stderr:

```text
snp: using .snp.yaml; ignoring .pre-commit-config.yaml (pass -c/--config to choose)
```

The scenario `tests/differential/divergences/D-EXT-09.yaml` checks the `.snp.yaml`-only case against the oracle; the
other rows are pinned by `crates/snp-config/src/discover.rs` and `crates/snp/tests/it/config_discovery.rs`.
