# D-DIAG-06

snp brand, `-c` not given and no config file in the repository: `snp: no .snp.yaml or .pre-commit-config.yaml file was
found in the repository root` before the unchanged error of the commands that need a config (`run`, `install-hooks`,
`list`, `autoupdate`, `migrate-config`, `config dump`, `migrate builtin|upstream`, `hook-impl` without `--config`; once
per process; ADR-087)

* **Class:** DIAG (COMPATIBILITY §5.8)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-DIAG-06.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by ADR-087 (`snp_config::discover::Discovery::note`).

## Register entry (COMPATIBILITY §5.8)

| Diagnostic (stderr only) |
|---|
| snp brand, `-c` not given and no config file in the repository: `snp: no .snp.yaml or .pre-commit-config.yaml file was found in the repository root` before the unchanged error of the commands that need a config (`run`, `install-hooks`, `list`, `autoupdate`, `migrate-config`, `config dump`, `migrate builtin\|upstream`, `hook-impl` without `--config`; once per process; ADR-087) |

## Rationale

snp also reads `.snp.yaml` (D-EXT-09), so pre-commit's `=====> .pre-commit-config.yaml is not a file` names only one
of the files snp looked for. The error texts stay pre-commit's — the snp brand's bytes remain the brand-mapped oracle's,
and an installed hook script, which always passes `--config`, still says `No .pre-commit-config.yaml file was found` —
and one stderr line says what else was tried. `install`, `uninstall` and `doctor` work without a config and print
nothing; the compat brand never prints it.

Safe: stderr only; stdout and exit codes are unchanged.

## Examples

`snp run --all-files` in a repository without a config file (`tests/differential/divergences/D-DIAG-06.yaml`):
pre-commit 4.6.2 and snp both print `An error has occurred: InvalidConfigError: ` / `=====> .pre-commit-config.yaml is
not a file` on stdout and exit 1; snp's stderr also has:

```text
snp: no .snp.yaml or .pre-commit-config.yaml file was found in the repository root
```
