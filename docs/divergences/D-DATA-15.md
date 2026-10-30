# D-DATA-15

block scalars rewritten safely

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-15.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`migrate::replacement`, `crates/snp-config/src/migrate.rs`; vector `MC4 folded python_venv`, unit test `block_scalars_become_plain`).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| migrate-config turns a folded `language: >-\n  python_venv` into invalid YAML | block scalars rewritten safely |

## Rationale

`_preserve_style` writes the replacement as `{style}{new}{style}`, where `style` is the node's PyYAML style: `'`
and `"` for quoted scalars, but `|` and `>` for block scalars. A folded `language: >-\n    python_venv` therefore
becomes `language: >python>` — not YAML — and the next command fails to parse the config (01 §10).

snp rewrites a block scalar as the plain scalar `python` (or `pre-commit`, …) followed by exactly the line breaks
and indentation the block's token ended with, so the surrounding lines are unchanged and the value is what
pre-commit meant. Quoted and plain scalars keep pre-commit's replacement byte for byte.

## Examples

```yaml
repos:
-   repo: local
    hooks:
    -   id: x
        name: x
        entry: x
        language: >-
            python_venv
        files: a
```

pre-commit 4.6.2 writes `        language: >python>        files: a` (invalid YAML). snp writes
`        language: python\n        files: a\n`.
