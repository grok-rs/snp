# Divergences from pre-commit 4.7.0

snp reproduces pre-commit 4.7.0 byte for byte (COMPATIBILITY §2, K1–K6). Every deliberate difference has an ID in
the register of `docs/COMPATIBILITY.md` §5 and, per ID, two files (ADR-071):

| File | Content | Checked by |
|---|---|---|
| `docs/divergences/<ID>.md` | the rationale (why snp differs, why it is safe) and examples | `cargo xtask divergence-book --check` |
| `tests/differential/divergences/<ID>.yaml` | a differential scenario that must **keep diverging** (`expect_divergence: <ID>`, optional `expected:` checks per side), or `status: unit` with the `unit_tests:` that pin a difference the CLI cannot reach | `cargo xtask difftest --suite divergences`, `cargo xtask difftest register` |

WP-00 created both files as stubs. They belong to the **first** work package of the ID's register row (WBS
"Divergence files"); the other packages of the row contribute behaviour, not files. WP-35 owns this README, the
harness and the register tooling, never a per-ID file.

## Rationale file layout

```markdown
# D-CLASS-nn

One sentence: what snp does differently.

* **Class:** CRASH | DATA | SEC | STORE | BRAND | PLAT | PERF | EXT | DIAG | TIERB (COMPATIBILITY §5.x)
* **Owner:** WP-xx (the code that implements it); other contributing packages.

## Register entry (COMPATIBILITY §5.x)

| pre-commit 4.7.0 | snp |
|---|---|
| … | … |

## Rationale

The pre-commit source that misbehaves (file:line), the failure mode, why snp's behaviour is admissible under the
class (COMPATIBILITY §4) and why it fails closed (K5).

## Examples

A minimal reproduction with both tools' output, matching the scenario file.
```

A rationale is a stub while it still reads `To be written by` or `**Status:** stub`; a scenario while it is
`status: stub`. `cargo xtask difftest register` lists the remaining stubs per owner; `--strict` (and
`cargo xtask divergence-book --check --strict`) fails while any remain.

## Rules the tooling enforces

* every register ID has both files, and every file names a register ID (`divergence-book --check`, `register`);
* a divergence scenario's `expect_divergence` is its own ID; any curated scenario naming an ID names a real one;
* the harness fails on an **unlisted** difference (a scenario without `expect_divergence`, or one that differs in a
  way its ID does not allow) and on a **stale** divergence (the sides became identical, or an `expected:` check no
  longer holds) — the register cannot rot in either direction;
* class **DIAG** (stderr-only diagnostics) may differ on stderr only; elsewhere snp's DIAG lines are stripped from
  stderr by a documented Tier-C rule (`snp_difftest::DIAG_LINES`) so they never mask another difference;
* class **EXT** scenarios with `strict: both` also run with `SNP_COMPAT_STRICT=1`, which must match the oracle.

## The book

`cargo xtask divergence-book` renders `target/docs/DIVERGENCES.md` (`--out PATH` to choose): an index table (ID,
snp's behaviour, scenario status) and one chapter per register class with every rationale, headings demoted under
the ID. WP-37's user documentation includes it.
