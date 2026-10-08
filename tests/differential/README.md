# Differential tests (snp vs pre-commit 4.6.2)

The differential harness (`snp-difftest`, ARCHITECTURE §13.2, research 10 §7.4) runs every scenario against snp and
against the pre-commit 4.6.2 oracle, then compares stdout, stderr, exit codes, the working tree, the index, refs and
config rewrites after normalisation. Every scenario runs in both brands: the compat brand compares bytes with the
oracle directly; the snp brand compares with the oracle whose argparse `prog` is patched to `snp`, after
`brand_map.yaml` is applied to its non-argparse texts (ADR-058, COMPATIBILITY §3.3).

| Path | What | Owner |
|---|---|---|
| `scenarios/**/*.yaml` | the scenario corpus (schema: research 10 §7.4.3) | WP-35 |
| `brand_map.yaml` | snp-brand substitutions for non-argparse texts | WP-35 (created by WP-00) |
| `divergences/<ID>.yaml` | one scenario per register ID of COMPATIBILITY §5 that must keep diverging (`expect_divergence: <ID>`) | the first WP of the ID's register row (stubs created by WP-00) |

A divergence file starts as a stub (`status: stub`), which the harness skips. Its owner replaces it with a scenario
that reproduces the registered difference; the harness fails when an unregistered difference appears and when a
registered one stops reproducing. `docs/divergences/<ID>.md` holds the rationale and examples of the same ID.

## Oracles

```
cargo xtask oracle install   # target/oracle/pc462-py312 (CPython 3.12.3) and pc462-py313 (CPython >= 3.13.14)
```

Both venvs are created with `uv` from the hash lock `xtask/oracle/requirements-pc462.txt` and carry the
`pre-commit-snp-prog` launcher used for snp-brand runs. Paths are passed explicitly; `PATH` lookups of `pre-commit`
or `snp` are never used.

## Running

The harness and its `cargo xtask difftest` driver are WP-35's (wave 3); see research 10 §7.4.8 for the planned
commands.
