# D-SEC-01

sanitized-or-hashed env dir names; deletion contained in the repo's envs/

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| `language_version: ../../x` (or an absolute path) makes the install step delete arbitrary directories; Windows `C:` versions break | sanitized-or-hashed env dir names; deletion contained in the repo's `envs/` |

## Rationale

To be written by WP-22: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-22: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-SEC-01.yaml` must keep reproducing the difference.
