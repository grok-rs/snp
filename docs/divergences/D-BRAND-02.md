# D-BRAND-02

prek and snp 1.x scripts are manager scripts (hook-script identity): replaced in place, no .legacy, no migration-mode lines; uninstall still restores a PreCommi…

* **Class:** BRAND (COMPATIBILITY §5.4)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-BRAND-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.4)

| pre-commit 4.6.2 | snp |
|---|---|
| `install` treats prek and snp 1.x hook scripts as foreign: moves them to `.legacy` and prints `Running in migration mode with existing hooks at …` / `Use -f to use only pre-commit.` (both tools then run on every commit; a pre-commit script in `.legacy` blocks every commit, 11 §A.5.2 chains C1–C4) | prek and snp 1.x scripts are manager scripts (hook-script identity): replaced in place, no `.legacy`, no migration-mode lines; `uninstall` still restores a `PreCommit`/`Prek` `.legacy` (rollback) |

## Rationale

To be written by WP-33: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-33: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-BRAND-02.yaml` must keep reproducing the difference.
