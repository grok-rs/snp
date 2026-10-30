# D-EXT-06

repo: builtin (pre-commit rejects these configs)

* **Class:** EXT (COMPATIBILITY §5.7)
* **Owner:** WP-21 (contributing: WP-26, WP-31); the first package of the register row owns this file and
  `tests/differential/divergences/D-EXT-06.yaml` (WBS "Divergence files", ADR-071).
* **Status:** validation implemented by WP-21 (the `repo: builtin` items of the config schema, `crates/snp-config/src/schema.rs`, `items::{UnlessBuiltin, BuiltinNoRev}`; acceptance test `builtin_repo_is_validated_by_the_config_schema`); the hooks are WP-26's, the resolution WP-31's.

## Register entry (COMPATIBILITY §5.7)

| Divergence |
|---|
| `repo: builtin` (pre-commit rejects these configs) |

## Rationale

`- repo: builtin` runs pre-commit-hooks' hooks natively without cloning anything (ARCHITECTURE §7.3). pre-commit
treats `builtin` as a remote URL and rejects the entry (`Missing required key: rev`, or a clone failure with a
rev), so such a config is snp-only by construction; COMPATIBILITY §7.3 documents it as an extension that is not
portable, and `snp migrate builtin` / `snp migrate upstream` convert both ways.

The validation is part of the config schema as cfgv items with cfgv contexts, so `validate-config` and `run` agree:
no `rev`; `entry` and `additional_dependencies` cannot be set; `language` is absent or `system`; every other hook
key and every extension key is checked like a remote config hook. The items for other repo kinds are unchanged
(pre-commit's remote-repo items skip only `builtin`), so no pre-commit message changes, and in `ExtMode::Strict`
`builtin` is an ordinary remote repo exactly as in pre-commit.

## Examples

```yaml
repos:
-   repo: builtin
    hooks:
    -   id: trailing-whitespace
```

pre-commit 4.6.2 `validate-config`, exit **1**:

```

==> File .pre-commit-config.yaml
==> At Config()
==> At key: repos
==> At Repository(repo='builtin')
=====> Missing required key: rev
```

snp: valid (exit 0); `rev: v6.0.0` added to it gives `=====> Expected rev to be absent when repo is 'builtin',
found rev: 'v6.0.0'`, `entry: x` gives `=====> 'entry' cannot be overridden` under `At Hook(id=…)`.
