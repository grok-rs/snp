# D-CRASH-03

the one-of / type-tag validation errors, exit 1

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-03.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`StagesMigration`, `LanguageMigration`, `check_type_tag` in `crates/snp-config/src/items.rs`; catalog tests `x13_cfg_04_…`, `x13_cfg_13_…`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| unhashable `stages`/`language`/`types` values → `TypeError` exit 3 (X13-CFG-04, X13-CFG-13 in manifests) | the one-of / type-tag validation errors, exit 1 |

## Rationale

`transform_stage`, `_translate_language` and `check_type_tag` look values up in a dict / frozenset. An unhashable
value (`stages: [[a]]`, `language: [x]`, `types: [[a]]`) raises `TypeError: unhashable type: 'list'` before the
`check_one_of` / tag check can report it, so pre-commit crashes with exit 3 (13 X.4.1 CFG-04, and CFG-13 for the
same values in a cloned repo's `.pre-commit-hooks.yaml`).

The value is invalid either way; snp lets it reach the check pre-commit meant to run: the lookup passes a
non-string value through unchanged, and the one-of / type-tag check rejects it with its usual message and the
Python `repr` of the value, under the usual contexts. Exit 1 (`InvalidConfigError` or `InvalidManifestError`).

## Examples

```yaml
repos:
-   repo: local
    hooks:
    -   id: a
        name: a
        entry: a
        language: system
        stages: [[a]]
```

pre-commit 4.7.0, `validate-config`, exit **3**: `An unexpected error has occurred: TypeError: unhashable type: 'list'`.

snp, exit **1**:

```

==> File .pre-commit-config.yaml
==> At Config()
==> At key: repos
==> At Repository(repo='local')
==> At key: hooks
==> At Hook(id='a')
==> At key: stages
==> At index 0
=====> Expected one of commit-msg, manual, post-checkout, post-commit, post-merge, post-rewrite, pre-commit, pre-merge-commit, pre-push, pre-rebase, prepare-commit-msg but got: ['a']
```

`language: [x]` gives `=====> Expected one of conda, …, unsupported_script but got: ['x']` under `At key:
language`; `types: [[a]]` gives `=====> Type tag ['a'] is not recognized.  Try upgrading identify and
pre-commit?` under `At key: types` / `At index 0`.
