# D-SEC-11

depth 512, 1 M constructed nodes, 16 MiB file limits → YAML error

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-14; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-11.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by `snp_yaml::Limits` (`crates/snp-yaml`), applied by `compose`, `construct`, `load` and
  `emit::redump`.

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| YAML alias bombs / deep nesting / huge files exhaust memory or the stack | depth 512, 1 M constructed nodes, 16 MiB file limits → YAML error |

## Rationale

PyYAML builds Python objects: an alias is a reference to the same object, so a "billion laughs" document of ten
levels of ten aliases costs nothing to *load* — but the value it returns holds 12,345,679,021 nodes when expanded,
and anything that walks it (validation, `repr`, a re-dump, a `cfgv::Value` tree) explodes. Nesting is limited only
by the C composer's stack and Python's recursion limit, and the file size not at all. snp materialises the loaded
document as a `cfgv::Value` tree and must never overflow a stack or exhaust memory on input it reads from a
repository it does not control (ARCHITECTURE §11.1, §11.4). It therefore enforces, as YAML errors of class
`YAMLError` that cfgv reports like any other load error:

* **16 MiB** of input (`Limits::max_bytes`), checked before parsing:
  `the input is N bytes long, more than the limit of 16777216 bytes`;
* **512 levels** of nesting (`Limits::max_depth`), counted while composing (an iterative composer: nesting costs heap,
  never stack) and again on the value after aliases and merges are expanded:
  `found a node nested deeper than the limit of 512 levels` with the mark of the first collection beyond it;
* **1,000,000 constructed nodes** (`Limits::max_constructed_nodes`), counting every node of the composed document, the
  pairs `<<` merge keys copy, and every use of an alias in the expanded value — computed on the shared node graph
  in linear time, before anything is copied: `found more than the limit of 1000000 nodes (aliases count at every
  use)` with the mark of the node whose copies cross the limit;
* a value that contains itself (`&a [*a]`, `&a {x: *a}`), which a tree cannot hold, is PyYAML's own constructor
  error for unconstructable recursion: `found unconstructable recursive node` at the recursive node.

The deviation is safe: real configurations are far inside every limit (the largest of the 10 §7.4.6 corpus has a few
hundred nodes and a nesting depth under 10), every limit is reported as an ordinary `InvalidConfigError` (exit 1)
instead of a hang, an out-of-memory kill or a stack overflow, and no document within the limits is affected — the
differential corpus of `crates/snp-yaml/tests/vectors` loads byte-identically. The 10 × 10 bomb is rejected in well
under 100 ms with a peak heap far below 64 MB (`crates/snp-yaml/tests/{perf,memory}.rs`); a 10,000-deep document is a
clean error on a 256 KiB thread stack (`tests/acceptance.rs`).

## Examples

The scenario `tests/differential/divergences/D-SEC-11.yaml` validates three configurations whose unknown key `x`
holds the hostile value (pre-commit never descends into an unknown key, so it only warns).

`.pre-commit-config.yaml`, an alias bomb:

```yaml
repos: []
x:
  a0: &a0 [lol, lol, lol, lol, lol, lol, lol, lol, lol, lol]
  a1: &a1 [*a0, *a0, *a0, *a0, *a0, *a0, *a0, *a0, *a0, *a0]
  # … a2 to a8 alike …
  a9: &a9 [*a8, *a8, *a8, *a8, *a8, *a8, *a8, *a8, *a8, *a8]
```

`pre-commit validate-config .pre-commit-config.yaml` (4.6.2): exit 0, stdout

```
[WARNING] Unexpected key(s) present at root: x
```

snp: exit 1, stdout

```

==> File .pre-commit-config.yaml
=====> found more than the limit of 1000000 nodes (aliases count at every use)
  in "<unicode string>", line 3, column 7
```

`deep.yaml` = `repos: []` and `x: ` followed by 600 `[` and 600 `]`: pre-commit exits 0 with the same warning; snp
exits 1 with `=====> found a node nested deeper than the limit of 512 levels` and
`  in "<unicode string>", line 2, column 515`.

`recursive.yaml` = `repos: []` and `x: &a [*a]`: pre-commit exits 0 with the same warning (the value is a list that
contains itself); snp exits 1 with `=====> found unconstructable recursive node` and
`  in "<unicode string>", line 2, column 4`.
