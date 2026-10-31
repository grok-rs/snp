# D-CRASH-07

validation-style errors, exit 1

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-07.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`snp_config::migrate::{read_config, migrate_config}`, `crates/snp-config/src/migrate.rs`; catalog tests `x13_cfg_10_…` to `x13_cfg_12_…`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| `migrate-config`/`autoupdate` with a missing config, a non-UTF-8 config, or an invalid re-indented fallback → exit 3 (X13-CFG-10…12) | validation-style errors, exit 1 |

## Rationale

`migrate_config` opens the config with `open(config_file)` outside any validation context and composes the
re-indented fallback of `_migrate_map` without a guard. A missing config (`FileNotFoundError`), a config that the
locale cannot decode (`UnicodeDecodeError`) and a list-root config whose four-space fallback is not YAML
(`ScannerError`, e.g. a leading `%YAML` directive) therefore crash `migrate-config` and `autoupdate` (which migrates
first) with exit 3 (13 X.4.1 CFG-10…12).

snp reports each as the `InvalidConfigError` that `run` prints for the same file, exit 1: a missing file is
`\n=====> .pre-commit-config.yaml is not a file` (`run`'s text), an undecodable file is the UTF-8 decode error under
`==> File …`, and an invalid fallback is the YAML error text under `==> File …`. The file is never written in any
of these cases (pre-commit does not write either).

## Examples

`.pre-commit-config.yaml`:

```yaml
%YAML 1.1
---
-   repo: local
    hooks: []
```

pre-commit 4.6.2, `pre-commit migrate-config`, exit **3**:

```
An unexpected error has occurred: ScannerError: while scanning for the next token
found character that cannot start any token
  in "<unicode string>", line 2, column 5
Check the log at <store>/pre-commit.log
```

snp, exit **1**:

```
An error has occurred: InvalidConfigError:
==> File .pre-commit-config.yaml
=====> while scanning for the next token
found character that cannot start any token
  in "<unicode string>", line 2, column 5
Check the log at <store>/snp.log
```
