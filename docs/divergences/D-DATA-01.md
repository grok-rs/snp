# D-DATA-01

one stderr line after the envelope: [ERROR] Your unstaged changes are saved in <patch>; restore them with: git apply --3way <patch> (X13-STASH-02)

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| final stash-restore failure prints only a traceback; the user does not learn where the unstaged work is | one stderr line after the envelope: `[ERROR] Your unstaged changes are saved in <patch>; restore them with: git apply --3way <patch>` (X13-STASH-02) |

## Rationale

To be written by WP-23: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-23: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DATA-01.yaml` must keep reproducing the difference.
