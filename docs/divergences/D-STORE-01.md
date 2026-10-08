# D-STORE-01

own store SNP_HOME > $XDG_CACHE_HOME/snp > ~/.cache/snp (Windows %USERPROFILE%\.cache\snp), versioned layout, no SQLite; refuses pre-commit's store; crash log n…

* **Class:** STORE (COMPATIBILITY §5.4)
* **Owner:** WP-22 (contributing: WP-33); the first package of the register row owns this file and
  `tests/differential/divergences/D-STORE-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.4)

| pre-commit 4.6.2 | snp |
|---|---|
| store `$PRE_COMMIT_HOME` or `~/.cache/pre-commit` (SQLite `db.db`); crash log `pre-commit.log` there | own store `SNP_HOME` > `$XDG_CACHE_HOME/snp` > `~/.cache/snp` (Windows `%USERPROFILE%\.cache\snp`), versioned layout, no SQLite; refuses pre-commit's store; crash log named by brand in snp's store; the crash-log body (below the version block) is snp's |

## Rationale

To be written by WP-22: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-22: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-STORE-01.yaml` must keep reproducing the difference.
