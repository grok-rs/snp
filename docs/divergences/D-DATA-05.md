# D-DATA-05

POSIX sh template, LF everywhere, atomic write that replaces a symlink instead of writing through it (template identity: BRAND)

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-05.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| hook script: bash template written through symlinks, CRLF on Windows | POSIX `sh` template, LF everywhere, atomic write that replaces a symlink instead of writing through it (template identity: BRAND) |

## Rationale

To be written by WP-33: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-33: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DATA-05.yaml` must keep reproducing the difference.
