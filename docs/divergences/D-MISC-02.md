# D-MISC-02

argv[1] == "__hazmat" is reserved (in-hook hazmat entry, both brands) — class BRAND

* **Class:** BRAND (COMPATIBILITY §5.4)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-MISC-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-33 (`snp::dispatch`, `snp_hooks::hazmat::cli`; test `hazmat_cli::reserved_in_hook_entry`).

## Register entry (COMPATIBILITY §5.4)

| pre-commit 4.7.0 | snp |
|---|---|
| `pre-commit __hazmat …` → `invalid choice: '__hazmat'`, exit 2 | `argv[1] == "__hazmat"` is reserved (in-hook hazmat entry, both brands) — class BRAND |

## Rationale

pre-commit 4.7.0 has a public `hazmat` command whose rewritten entries run `pre-commit hazmat …` inside hooks; its
top-level parser rejects anything else, so `pre-commit __hazmat …` is a usage error (`invalid choice: '__hazmat'`,
exit 2). snp reserves `argv[1] == "__hazmat"` in both brands as the in-hook hazmat entry: dispatch hands the
remaining arguments to `snp_hooks::hazmat::cli` before the argparse model is consulted (12 §A.17), so a hook entry
that snp rewrites (`cd`, `ignore-exit-code`, `n1`) runs without parsing the whole CLI and without depending on how
the public command is spelled in a given brand. Its parser texts use prog `hazmat.py`, like pre-commit's module.

Why it is safe: `__hazmat` is not a name a user types — it starts with two underscores, appears in no help text, and
in pre-commit it is only ever an error. The public `hazmat` command keeps pre-commit's behaviour byte for byte
(X13-HZ-01…05).

## Examples

```text
$ pre-commit __hazmat n1 echo -- a b          # pre-commit 4.7.0, stderr, exit 2
usage: pre-commit [-h] [-V]
                  {autoupdate,clean,gc,hazmat,init-templatedir,install,install-hooks,migrate-config,run,sample-config,try-repo,uninstall,validate-config,validate-manifest,help,hook-impl} ...
pre-commit: error: argument command: invalid choice: '__hazmat' (choose from 'autoupdate', 'clean', 'gc', 'hazmat', 'init-templatedir', 'install', 'install-hooks', 'migrate-config', 'run', 'sample-config', 'try-repo', 'uninstall', 'validate-config', 'validate-manifest', 'help', 'hook-impl')
$ pre-commit __hazmat n1 echo -- a b          # snp (either brand), exit 0
a
b
$ snp __hazmat n1 echo                         # stderr, exit 1
hazmat entry must end with `--`
```

The scenario `tests/differential/divergences/D-MISC-02.yaml` runs the first command in both brands.
