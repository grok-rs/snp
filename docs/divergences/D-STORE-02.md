# D-STORE-02

snp never touches pre-commit's directories; clean always prints Cleaned {snp store}.

* **Class:** STORE (COMPATIBILITY §5.4)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-STORE-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.4)

| pre-commit 4.6.2 | snp |
|---|---|
| `clean` also removes `~/.pre-commit`; `gc`/`clean` operate on pre-commit's store | snp never touches pre-commit's directories; `clean` always prints `Cleaned {snp store}.` |

## Rationale

To be written by WP-33: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-33: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-STORE-02.yaml` must keep reproducing the difference.
