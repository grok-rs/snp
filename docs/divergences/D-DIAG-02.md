# D-DIAG-02

install/uninstall/init-templatedir/doctor --fix notes when replacing other managers' scripts: M1 Replaced hook script from {owner} at {hook_path} [+ (Python:…

* **Class:** DIAG (COMPATIBILITY §5.8)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-DIAG-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-33 (`snp_cmd::messages`, `snp_cmd::install`, `doctor --fix`; goldens
  `install::migration_note_goldens_in_both_brands`).

## Register entry (COMPATIBILITY §5.8)

| Diagnostic (stderr only) |
|---|
| `install`/`uninstall`/`init-templatedir`/`doctor --fix` notes when replacing other managers' scripts: M1 `Replaced hook script from {owner} at {hook_path}` [+ ` (Python: {INSTALL_PYTHON})`], M1b, M2 `Removed {legacy_path}: it is a hook script from {owner}, not a user hook`, M3 ``Note: {hook_path} still runs {owner}; run `{prog} install -t {T}` to switch it to snp``, M12 (11 §A.5.4) — both brands |

## Rationale

When snp replaces another manager's hook script (D-BRAND-02), deletes a manager script parked in `.legacy`
(D-DATA-03) or leaves another hook type still pointing at another manager, the user should learn what changed:
otherwise a replaced prek or pre-commit script, or a hook type that still runs the old tool, is invisible. pre-commit
prints nothing of the kind (it has no such cases: it moves the scripts to `.legacy`).

The notes of 11 §A.5.4 are written to **stderr** in both brands, never stdout, so the stdout of `install`,
`uninstall`, `init-templatedir` and `doctor --fix` stays pre-commit's line for line (`pre-commit installed at …`,
`Running in migration mode …`) and tools that parse it are unaffected:

* M1 `Replaced hook script from {owner} at {hook_path}`, with ` (Python: {INSTALL_PYTHON})` for a pre-commit script
  whose `INSTALL_PYTHON` was parsed (so the user can find the interpreter that used to run hooks), or ` (could not
  read its options; defaults used)` from `doctor --fix`;
* M1b `Note: the replaced prek hook used prek-only options ({opts}); they are not supported and were dropped`;
* M2 `Removed {legacy_path}: it is a hook script from {owner}, not a user hook`;
* M3 ``Note: {hook_path} still runs {owner}; run `{prog} install -t {T}` to switch it to snp`` (`{prog}` follows the
  brand);
* M12 `Removed {legacy_path}: it is a hook script from snp, not a user hook` (`uninstall`).

Why it is safe: stderr-only diagnostics change no exit code, no stdout byte and no file; the differential harness
strips class DIAG lines from stderr only through this scenario (ARCHITECTURE §13.2).

## Examples

`pre-commit` holds a pre-commit script (`INSTALL_PYTHON=/old/python`), `pre-push` a prek script:

```text
$ pre-commit install           # pre-commit 4.7.0: stdout, stderr empty
pre-commit installed at .git/hooks/pre-commit
$ pre-commit install           # snp, compat brand: the same stdout; stderr:
Replaced hook script from pre-commit at .git/hooks/pre-commit (Python: /old/python)
Note: .git/hooks/pre-push still runs prek; run `pre-commit install -t pre-push` to switch it to snp
$ snp install                  # snp brand: stdout "snp installed at .git/hooks/pre-commit"; stderr:
Replaced hook script from pre-commit at .git/hooks/pre-commit (Python: /old/python)
Note: .git/hooks/pre-push still runs prek; run `snp install -t pre-push` to switch it to snp
```

With a pre-commit script also parked in `pre-commit.legacy`, snp adds (M2, stderr)
`Removed .git/hooks/pre-commit.legacy: it is a hook script from pre-commit, not a user hook`.

The scenario `tests/differential/divergences/D-DIAG-02.yaml` runs the first case in both brands.
