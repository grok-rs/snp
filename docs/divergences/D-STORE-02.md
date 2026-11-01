# D-STORE-02

snp never touches pre-commit's directories; clean always prints Cleaned {snp store}.

* **Class:** STORE (COMPATIBILITY §5.4)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-STORE-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-33 (`snp_cmd::store_cmds::{clean, gc}`; test `clean::t10_gc_08_clean_prints_and_is_idempotent`).

## Register entry (COMPATIBILITY §5.4)

| pre-commit 4.6.2 | snp |
|---|---|
| `clean` also removes `~/.pre-commit`; `gc`/`clean` operate on pre-commit's store | snp never touches pre-commit's directories; `clean` always prints `Cleaned {snp store}.` |

## Rationale

pre-commit's `clean` (`commands/clean.py:10-16`) removes its store and the pre-1.0 store `~/.pre-commit`, and prints
`Cleaned {dir}.` for each one that exists; because `main()` constructs `Store()` (which creates the directory) before
dispatching, the store line always prints. `gc` sweeps pre-commit's store.

snp has its own store (D-STORE-01: `SNP_HOME` > `$XDG_CACHE_HOME/snp` > `~/.cache/snp`) and is meant to be installed
next to pre-commit during a migration, so it never deletes or even opens another tool's directories: `clean` retires
snp's own store under the exclusive store lock and prints `Cleaned {snp store}.` — always, also when the store did
not exist (snp does not create it just to delete it, ADR-030) — and `gc` only marks and sweeps snp's store. Neither
`~/.pre-commit` nor `~/.cache/pre-commit` is touched.

Why it is safe: pre-commit users who switch to snp can still switch back without re-installing every environment, and
two tools never race on one store. The printed line keeps pre-commit's shape (`Cleaned {path}.`), so scripts that
check for it keep working; only the path differs, which the differential harness treats as a store root (Tier C).

## Examples

```text
$ ls -a ~ ~/.cache
.pre-commit  .cache            # ~/.pre-commit exists, ~/.cache/pre-commit exists
$ pre-commit clean             # pre-commit 4.6.2
Cleaned /home/u/.cache/pre-commit.
Cleaned /home/u/.pre-commit.
$ snp clean                    # both pre-commit directories stay
Cleaned /home/u/.cache/snp.
$ snp clean                    # again: the store is gone, the line still prints
Cleaned /home/u/.cache/snp.
```

The scenario `tests/differential/divergences/D-STORE-02.yaml` runs `clean` with a pre-1.0 `~/.pre-commit` present.
