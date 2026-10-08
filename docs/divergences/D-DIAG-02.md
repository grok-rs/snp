# D-DIAG-02

install/uninstall/init-templatedir/doctor --fix notes when replacing other managers' scripts: M1 Replaced hook script from {owner} at {hook_path} [+ (Python:…

* **Class:** DIAG (COMPATIBILITY §5.8)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-DIAG-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.8)

| Diagnostic (stderr only) |
|---|
| `install`/`uninstall`/`init-templatedir`/`doctor --fix` notes when replacing other managers' scripts: M1 `Replaced hook script from {owner} at {hook_path}` [+ ` (Python: {INSTALL_PYTHON})`], M1b, M2 `Removed {legacy_path}: it is a hook script from {owner}, not a user hook`, M3 ``Note: {hook_path} still runs {owner}; run `{prog} install -t {T}` to switch it to snp``, M12 (11 §A.5.4) — both brands |

## Rationale

To be written by WP-33: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-33: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DIAG-02.yaml` must keep reproducing the difference.
