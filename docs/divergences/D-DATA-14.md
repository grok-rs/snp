# D-DATA-14

rewrites deduplicated by YAML node

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-14.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`migrate_composed` dedupes rewrites by node id, `crates/snp-config/src/migrate.rs`; vector `MC4 alias stages`, unit test `aliases_are_rewritten_once`).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| migrate-config doubles the rewrite of an aliased `stages` list (`[pre-commitpre-commit]`) | rewrites deduplicated by YAML node |

## Rationale

`_migrate_composed` collects the scalar nodes its matchers reach and splices each replacement at the node's marks.
A YAML alias is the same node object as its anchor, so a stage list reached twice (`default_stages: &s [commit]`
and `stages: *s`) is rewritten twice at the same offsets: the second splice lands on the first one's output and
the file becomes `default_stages: &s [pre-commitpre-commit]`, an invalid stage that breaks the config (01 §10).

snp's composer gives aliases the anchor's `NodeId` (ADR-002), and the rewrites are deduplicated by node before
splicing, so the anchored value is rewritten exactly once — which is also what every alias then sees.

## Examples

```yaml
default_stages: &s [commit]
repos:
-   repo: local
    hooks:
    -   id: x
        name: x
        entry: x
        language: system
        stages: *s
```

pre-commit 4.6.2 `migrate-config` writes `default_stages: &s [pre-commitpre-commit]` (exit 0; every later command
fails validation). snp writes `default_stages: &s [pre-commit]` and leaves `stages: *s` as is.
