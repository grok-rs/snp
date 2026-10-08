# D-CRASH-12

bytes end to end (argv, identify); lossy UTF-8 view for regex matching

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-12.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| non-UTF-8 file names from git or `--files` → `UnicodeDecodeError`/`UnicodeEncodeError` exit 3 (X13-GIT-05…08) | bytes end to end (argv, identify); lossy UTF-8 view for regex matching |

## Rationale

To be written by WP-22: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-22: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-CRASH-12.yaml` must keep reproducing the difference.
