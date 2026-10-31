# D-EXT-01

recognised extension keys (§7) produce no Unexpected key(s) warning and take effect

* **Class:** EXT (COMPATIBILITY §5.7)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-EXT-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (extension `MapItem`s appended after pre-commit's items and added to the root known-key list, `crates/snp-config/src/{schema,ext,items}.rs`; acceptance tests `extension_keys_are_recognised_without_warnings`, `strict_mode_reproduces_pre_commit_warnings`).

## Register entry (COMPATIBILITY §5.7)

| Divergence |
|---|
| recognised extension keys (§7) produce no `Unexpected key(s)` warning and take effect |

## Rationale

snp's extension keys (COMPATIBILITY §7) are plain keys a config shares with pre-commit (ADR-039). pre-commit treats
them as unknown keys: root keys print `Unexpected key(s) present at root: …` when the config loads, hook keys print
`Unexpected key(s) present on <src> => <id>: …` when the hooks are built, and nothing else changes. snp recognises
them: they are validated (with pre-commit's contexts and snp leaves, appended after pre-commit's items so every
pre-commit error and warning keeps its order) and take effect, and they produce no warning.

The difference is observable only in a config that uses an extension key. Unknown keys — including `x-*` keys and
extension keys inside a manifest — keep pre-commit's warnings byte for byte, and `SNP_COMPAT_STRICT=1`
(`ExtMode::Strict`) makes snp treat every extension key exactly as pre-commit does (the differential harness's
mode).

## Examples

```yaml
priorities: {early: 1}
repos:
-   repo: local
    hooks:
    -   id: fmt
        name: fmt
        entry: echo fmt
        language: system
        priority: early
        groups: [fast]
```

pre-commit 4.6.2 `run --all-files` prints `[WARNING] Unexpected key(s) present at root: priorities` and
`[WARNING] Unexpected key(s) present on local => fmt: groups, priority` before the hook line. snp prints only the
hook line (and with `SNP_COMPAT_STRICT=1` prints both warnings exactly like pre-commit).
