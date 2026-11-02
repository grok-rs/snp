# D-CRASH-01

validation error leaf invalid literal for int() with base 10: 'abc', exit 1

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`snp_config::items::check_min_version`, `crates/snp-config/src/items.rs`; catalog test `x13_cfg_01_min_version_int_error_is_a_validation_error`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| `minimum_pre_commit_version: "abc"` (also `""`, `"4."`, `"4.6.3rc1"`) → `ValueError` exit 3 (X13-CFG-01) | validation error leaf `invalid literal for int() with base 10: 'abc'`, exit 1 |

## Rationale

`clientlib.check_min_version` calls `parse_version(s) = tuple(int(p) for p in s.split('.'))`. A component that
`int()` rejects raises `ValueError`, which is not a `cfgv.ValidationError`: it escapes cfgv's `reraise_as` and
pre-commit prints `An unexpected error has occurred: ValueError: …` with exit 3 and writes a crash log (13 X.4.1
CFG-01). The input is user-fixable config data that pre-commit meant to validate, so 13 X.3 rule 1 converts it to
the validation channel: the same text becomes the leaf of the usual cfgv context chain and the command exits 1
(`validate-config` prints the block, run-type commands print the `InvalidConfigError` envelope).

Nothing is weakened: the config is still rejected, under the same key, with Python's own `int()` message (the
first failing component, `%.200R`, and the 4300-digit limit text, from `snp_pycompat::int::parse_version`).
The rule applies to the top level, local hooks, remote config hooks and manifests (X13-CFG-13).

## Examples

`.pre-commit-config.yaml`:

```yaml
minimum_pre_commit_version: "4.6.3rc1"
repos: []
```

pre-commit 4.7.0, `pre-commit validate-config .pre-commit-config.yaml`, exit **3**:

```
An unexpected error has occurred: ValueError: invalid literal for int() with base 10: '3rc1'
Check the log at <store>/pre-commit.log
```

snp, exit **1** (stdout):

```

==> File .pre-commit-config.yaml
==> At Config()
==> At key: minimum_pre_commit_version
=====> invalid literal for int() with base 10: '3rc1'
```

The oracle vectors `crates/snp-config/tests/vectors/load.json` (cases `X13-CFG-01 …`) record pre-commit's crash
text and snp's expected block for `abc`, `''`, `4.`, `4.6.3rc1`, `4..6`, `0x10`, `1.x` and the hook / manifest
variants.
