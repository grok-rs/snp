# D-CRASH-04

validation error '<repr>' is not a valid python regex, exit 1

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-21 (contributing: WP-13); the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`check_string_regex` = `check_and(check_string, check_regex_with(snp_pyre::cfgv_is_valid))`, `crates/snp-config/src/items.rs`) on WP-13's CPython-3.12 parser; catalog tests `x13_cfg_05_…` to `x13_cfg_07_…`.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| regex repeat ≥ 4294967295, nesting ≥ 485, `(?a)(?u)` in `files`/`exclude` → `OverflowError`/`RecursionError`/`ValueError` exit 3 (X13-CFG-05…07, 13) | validation error `'<repr>' is not a valid python regex`, exit 1 |

## Rationale

`cfgv.check_regex` catches only `re.error`. Three classes of pattern make CPython's `re.compile` raise something
else, which escapes cfgv and crashes pre-commit with exit 3 (01 Addendum §A.4, 13 X.4.1 CFG-05…07):
a repeat bound ≥ 4294967295 (`OverflowError: the repetition number is too large`), group nesting ≥ 485
(`RecursionError: maximum recursion depth exceeded`) and conflicting global flags in separate groups
(`ValueError: ASCII and UNICODE flags are incompatible`).

All three are invalid patterns a user can fix, so snp reports them like every other invalid regex (ADR-003,
ARCHITECTURE §1.5): the single cfgv leaf `'<repr>' is not a valid python regex` under the key's context, exit 1.
The accept/reject boundary is CPython 3.12's (`a{4294967294}` and 484 levels of nesting are accepted; `(?au)x` was
already a clean `re.error`).

## Examples

```yaml
files: 'a{4294967296}'
repos: []
```

pre-commit 4.7.0, exit **3**: `An unexpected error has occurred: OverflowError: the repetition number is too large`.

snp, exit **1**:

```

==> File .pre-commit-config.yaml
==> At Config()
==> At key: files
=====> 'a{4294967296}' is not a valid python regex
```
