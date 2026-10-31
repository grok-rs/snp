# D-STORE-01

own store `SNP_HOME` > `$XDG_CACHE_HOME/snp` > `~/.cache/snp`, versioned layout, no SQLite; refuses pre-commit's store; crash log named by brand

* **Class:** STORE (COMPATIBILITY §5.4)
* **Owner:** WP-22 (contributing: WP-33); the first package of the register row owns this file and
  `tests/differential/divergences/D-STORE-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.4)

| pre-commit 4.6.2 | snp |
|---|---|
| store `$PRE_COMMIT_HOME` or `~/.cache/pre-commit` (SQLite `db.db`); crash log `pre-commit.log` there | own store `SNP_HOME` > `$XDG_CACHE_HOME/snp` > `~/.cache/snp` (Windows `%USERPROFILE%\.cache\snp`), versioned layout, no SQLite; refuses pre-commit's store; crash log named by brand in snp's store; the crash-log body (below the version block) is snp's |

## Rationale

Owner decision FD-2 (ADR-011, ADR-024): snp keeps its own store. Sharing pre-commit's would mix two layouts and two
lock protocols in one directory (pre-commit locks with `msvcrt` byte ranges on Windows, snp with `LockFileEx`), make
`snp clean` delete pre-commit's `db.db`, and let either tool's environments break the other's health checks.

`snp_store::resolve_location`: `SNP_HOME` (non-empty), else `$XDG_CACHE_HOME/snp` (non-empty, every OS, as pre-commit
honours it), else `expanduser("~/.cache")/snp` — `%USERPROFILE%\.cache\snp` on Windows, `~/.cache/snp` on macOS,
pre-commit's own default shape (14 P-65); relative values resolve against the invocation cwd and the result is
`realpath`'d like pre-commit's. `PRE_COMMIT_HOME` is read only to compute pre-commit's store and refuse it: a resolved
path equal to it, or a directory whose README starts with pre-commit's signature, is a FatalError (`snp refuses to
use pre-commit's cache directory <path>; set SNP_HOME to another directory`, M7). The layout is versioned
(`repos-v1`, `keys-v1`, `configs-v1`, `tools-v1`, `cache-v1`, ARCHITECTURE §5.2) and has no database. The crash log
is `<store>/pre-commit.log` in the compat brand and `<store>/snp.log` in the snp brand (ADR-013).

Safe: everything observable stays pre-commit's except store-derived paths (`Check the log at …`, `Stashing unstaged
files to …`), which the comparison tiers treat as Tier C. CI cache recipes must cache `~/.cache/snp` (user docs).

## Examples

Recorded with pre-commit 4.6.2 on CPython 3.12.3 (a venv built from the hash lock `xtask/oracle/requirements-pc462.txt`); `<W>` is the scratch root, `<store>` the store directory.

An invalid config (`repos: 1`), `run --all-files` with `HOME=<W>/home`, `XDG_CACHE_HOME` unset
(`tests/differential/divergences/D-STORE-01.yaml`).

pre-commit 4.6.2 with `PRE_COMMIT_HOME=<W>/pch`, exit 1:

```text
An error has occurred: InvalidConfigError:
==> File .pre-commit-config.yaml
==> At Config()
==> At key: repos
=====> Expected array but got 'int'
Check the log at <W>/pch/pre-commit.log
```

(and `<W>/pch` now holds `README`, `db.db`, `.lock`, `pre-commit.log`). snp prints the same lines except the last:
`Check the log at <W>/home/.cache/snp/pre-commit.log` (compat brand) / `…/snp/snp.log` (snp brand); `<W>/pch` is
neither created nor read. With `XDG_CACHE_HOME=<W>/xdg` pre-commit logs to `<W>/xdg/pre-commit/pre-commit.log` and
snp to `<W>/xdg/snp/pre-commit.log`.

Unit coverage: `crates/snp-store/tests/catalog.rs::t10_sto_01_default_directory_precedence`,
`support/behaviour.rs::location_matrix`, `m7_refuses_pre_commit_store_by_path_and_by_readme`.
