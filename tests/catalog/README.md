# Test catalog index (ADR-062, ARCHITECTURE §13.8)

`index.tsv` lists every catalog row of the research documents with its namespaced ID and its owner:

| Prefix | Source | Rows |
|---|---|---|
| `T10-<FAM>-nn` | `docs/research/10-test-strategy.md` §5 (pre-commit's own test suite) | 287 |
| `X13-<FAM>-nn` | `docs/research/13-crash-and-message-catalog.md` X.4 and X.5 (crash and message catalog) | 97 |
| `M11-MIG-nn` | `docs/research/11-dropin-migration.md` §A.20 (drop-in migration tests) | 24 |

Columns (tab-separated, one header line):

* `id`: the namespaced ID. The test that ports a row is named after it: `t10_xrg_03_<slug>`, `x13_cfg_01_<slug>`,
  `m11_mig_04_<slug>`.
* `source`: the section the row comes from (`10 §5.4`, `13 X.4.1`, `13 X.5`, `11 §A.20`).
* `title`: the row's first descriptive cell, shortened to 100 characters.
* `owner`: the work package that ports the row, from the WBS table "Catalog ownership" (family owner plus row
  exceptions).
* `hermetic`: `no` when the row needs a language toolchain, the network, the pre-commit oracle or a Windows-only
  host (the rules are in `xtask/src/tasks/catalog_index.rs`).
* `test_file`: the planned home of the test: the owner's catalog file, or for the rows WP-00 ported in wave 0 the
  file that holds them. An owner may put the test elsewhere in its crate; `catalog-status` finds tests by name.

The file is generated. Do not edit it by hand: change the research documents or the WBS table, then run
`cargo xtask catalog-index` (`cargo xtask catalog-index --check` fails when the committed file is stale; CI runs it).
`cargo xtask catalog-status` (WP-35) reports which rows are ported.
