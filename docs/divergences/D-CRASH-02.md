# D-CRASH-02

[WARNING] Unexpected key(s) present at root: … (Python str() of the keys, sorted by type rank)

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (the root and repo `WarnAdditionalKeys` callbacks, `crates/snp-config/src/schema.rs`; catalog tests `x13_cfg_02_…`, `x13_cfg_03_…`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| non-string / mixed root keys → `TypeError` exit 3 (X13-CFG-02/03) | `[WARNING] Unexpected key(s) present at root: …` (Python `str()` of the keys, sorted by type rank) |

## Rationale

`warn_unknown_keys_root` formats the extra keys with `", ".join(extra)` after cfgv's `sorted(set(dct) -
set(keys))`. A non-string mapping key (`1: x`, `true: x`, `~: x`) makes `join` raise `TypeError: sequence item 0:
expected str instance, int found`; a mix of strings and other types already fails in `sorted()` (`'<' not
supported between instances of 'str' and 'int'`). Both are exit-3 crashes on a config whose only problem is an
unknown key, which pre-commit otherwise reports with a warning and accepts (13 X.4.1 CFG-02/03, 13 X.3 rule 3:
an obvious bug whose fix cannot weaken a check).

snp prints the warning pre-commit intended: `str()` of every key, ordered like Python's `sorted()` within a type
and grouped by type rank across types (`None`, then numbers and booleans, then strings, …; cfgv-rs's
`py_sort`). Keys that Python dict semantics collapse (`1` and `true`) are one key, as in PyYAML.

## Examples

```yaml
repos: []
1: x
foo: y
```

pre-commit 4.6.2, `validate-config`, exit **3**:

```
An unexpected error has occurred: TypeError: '<' not supported between instances of 'str' and 'int'
Check the log at <store>/pre-commit.log
```

snp, exit **0**:

```
[WARNING] Unexpected key(s) present at root: 1, foo
```
