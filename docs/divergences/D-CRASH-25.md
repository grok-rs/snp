# D-CRASH-25

UTF-8 always (see D-DATA-09)

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-25.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`snp_config::migrate::migrate_config` decodes strict UTF-8 on every platform, `crates/snp-config/src/migrate.rs`; catalog test `x13_cfg_11_migrate_non_utf8_config`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| `migrate-config` on a list-root / non-UTF-8 config under a non-UTF-8 locale (Windows cp1252) → crash or mis-decode | UTF-8 always (see D-DATA-09) |

## Rationale

pre-commit reads the config with `open(config_file)` (the locale encoding) in `migrate_config`, while every other
command reads it as UTF-8 (`cfgv.load_from_filename`). Under a non-UTF-8 locale (Windows cp1252, `LANG=C` on older
Pythons) a UTF-8 config with non-ASCII bytes is mis-decoded and then re-encoded on write, or a non-UTF-8 config
crashes with `UnicodeDecodeError` (exit 3).

snp reads and writes UTF-8 everywhere, the encoding every other pre-commit command already requires of the file,
so `migrate-config` agrees with `run` about which files are valid and never transcodes user data. An undecodable
file is the validation error `run` prints (D-CRASH-07).

## Examples

A config whose comment contains the byte `0xe9` (Latin-1 `é`) at byte 15:

pre-commit 4.7.0 on a UTF-8 locale, `migrate-config`, exit **3**: `An unexpected error has occurred:
UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe9 in position 15: invalid continuation byte`.
On cp1252 Windows the same file is decoded as `é` and silently re-encoded.

snp on every platform, exit **1**:

```
An error has occurred: InvalidConfigError:
==> File .pre-commit-config.yaml
=====> 'utf-8' codec can't decode byte 0xe9 in position 15: invalid continuation byte
```
