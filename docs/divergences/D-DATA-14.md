# D-DATA-14

rewrites deduplicated by YAML node

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-14.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`migrate_composed` dedupes rewrites by node id with a bitmap and keeps the anchor of a rewritten node, `crates/snp-config/src/migrate.rs`; vectors `MC4 alias stages`, `anchored sha key`, `tagged stages`, `tagged sha key`; unit tests `aliases_are_rewritten_once`, `anchors_survive_a_rewrite`, `tags_are_dropped_like_pre_commit`; perf smoke `migrate_300k_stage_items_under_4s`).

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
splicing, so the anchored value is rewritten exactly once — which is also what every alias then sees. The
dedupe is a bitmap over the dense node ids, linear in the number of rewrites (a `Vec::contains` scan made a
320k-item `default_stages` list take ≈ 35 s).

The same hazard has a second form: pre-commit replaces a node from its *start mark*, which lies before the
node's properties, so a rewritten scalar loses its `!tag` and its `&anchor` (`&k sha: v1` → `rev: v1`). Losing
the anchor makes every later `*k` an undefined alias, and the migrated file no longer loads. snp keeps each
`&anchor` (with the whitespace after it) in front of the replacement; the `!tag` is dropped exactly as in
pre-commit (`[!!str commit]` → `[pre-commit]`, byte-identical), since the rewritten value is a plain string
either way.

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

```yaml
repos:
-   repo: https://x/r
    &k sha: v1
    hooks: []
x: *k
```

pre-commit 4.6.2 writes `    rev: v1` and leaves `x: *k` (exit 0; the next load fails with `=====> found undefined
alias`). snp writes `    &k rev: v1`, and the file still loads.
