# D-DIAG-05

store notices: the foreign-owner warning (ADR-044), `Removed snp 1.x cache data from {store}.` (gc)

* **Class:** DIAG (COMPATIBILITY §5.8)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-DIAG-05.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.8)

| Diagnostic (stderr only) |
|---|
| store notices: the foreign-owner warning (ADR-044), `Removed snp 1.x cache data from {store}.` (gc) |

## Rationale

Two stderr-only notices of snp's own store, which pre-commit cannot print because it has no such situations:

* **Foreign owner** (ADR-044): `[WARNING] the snp store <path> is owned by uid <n>, not by the current user` — once
  per process, from `Store::ensure_created`, when the store belongs to another uid and is used anyway (volume-mounted
  CI caches; as root only with `SNP_ALLOW_FOREIGN_STORE=1`, D-SEC-13).
* **snp 1.x leftovers** (11 §A.10.3): snp 1.x used `~/.cache/snp` with `db.db`, `repos/`, `environments/` and
  `resource-pools/`. `Store::sweep` (gc) removes them only when the store's README carries the 1.x signature (`This
  directory is maintained by the SNP (Shell Not Pass) project.`), rewrites the README and reports
  `SweepReport::legacy_v1_removed`; WP-33's `gc` then prints `Removed snp 1.x cache data from {store}.` on stderr
  before `{n} repo(s) removed.`.

Safe: stderr only; stdout and exit codes are unchanged.

## Examples

`gc` with `SNP_HOME` pointing at a directory that holds an snp 1.x README and `db.db`
(`tests/differential/divergences/D-DIAG-05.yaml`). pre-commit 4.7.0 (its own store), exit 0, stdout
`0 repo(s) removed.`, empty stderr. snp, exit 0, the same stdout and on stderr:

```text
Removed snp 1.x cache data from <W>/snpstore.
```

Unit coverage: `crates/snp-store/tests/support/behaviour.rs::sweep_removes_other_versions_and_snp1_leftovers`;
the foreign-owner text is golden-tested in `crates/snp-store/src/messages.rs`.
