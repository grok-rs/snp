# cfgv

[![CI](https://github.com/grok-rs/cfgv-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/grok-rs/cfgv-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/cfgv.svg)](https://crates.io/crates/cfgv)
[![docs.rs](https://docs.rs/cfgv/badge.svg)](https://docs.rs/cfgv)
[![License: MIT](https://img.shields.io/crates/l/cfgv.svg)](LICENSE)

Validate configuration and produce human readable error messages.

A Rust port of the Python [cfgv](https://github.com/asottile/cfgv) library
(3.5.0), the validation engine behind [pre-commit](https://pre-commit.com).
Error messages are **byte-identical** to Python cfgv's, including Python's
`repr()` of every value kind, which makes the crate suitable for drop-in
reimplementations of Python tools.

## Installation

```toml
[dependencies]
cfgv = "0.3"
```

Minimum supported Rust version: **1.85**.

### Features

| Feature | Default | Description |
|---|---|---|
| `regex` | yes | `CheckFn::check_regex()` backed by the [`regex`](https://crates.io/crates/regex) crate |
| `serde` | no | `Serialize`/`Deserialize` for `Value`, a `Deserializer` for `&Value` and `cfgv::from_value` |

## Usage

```rust
use cfgv::{CheckFn, ConditionValue, Schema, Validator, apply_defaults, validate, value_map};

let schema = Schema::map("Repository", Some("repo".into()), vec![
    Validator::required("repo", CheckFn::check_string()),
    Validator::conditional_with_absent(
        "rev",
        CheckFn::check_string(),
        "repo",
        ConditionValue::NotIn(vec!["local".into(), "meta".into()]),
        true,
    ),
    Validator::optional("fail_fast", CheckFn::check_bool(), false),
]);

let repo = value_map! {"repo" => "https://github.com/example/repo", "rev" => "v1.0"};
validate(&repo, &schema).unwrap();
let with_defaults = apply_defaults(&repo, &schema); // adds fail_fast: False

let err = validate(&value_map! {"repo" => "local", "rev" => "v1.0"}, &schema).unwrap_err();
assert_eq!(
    err.to_string(),
    "\n==> At Repository(repo='local')\n\
     =====> Expected rev to be absent when repo is any of ('local', 'meta'), found rev: 'v1.0'",
);
```

## Error messages

Errors render exactly like Python's `str(ValidationError)`: a leading
newline, one `==> context` line per level, and the `=====> message` leaf.

```text

==> File .pre-commit-config.yaml
==> At Config()
==> At key: repos
==> At Repository(repo='https://github.com/pre-commit/pre-commit-hooks')
==> At key: hooks
==> At Hook(id="it's")
==> At key: always_run
=====> Expected bool got str
```

Every message format of `cfgv.py` is reproduced, with `repr()` wherever
Python uses it: string quoting and escapes (`"it's"`, `'a\\b'`,
`'​'`), floats (`1.0`, `1e+16`, `1e-05`, `nan`), big integers, bytes,
tuples (`('a',)`), sets, `datetime.date(2020, 1, 1)` and
`datetime.datetime(...)` with time zones.

## Values

`Value` mirrors the objects PyYAML's `SafeLoader` produces:

| Variant | Python type | Variant | Python type |
|---|---|---|---|
| `Null` | `NoneType` | `Bytes` | `bytes` |
| `Bool` | `bool` | `List` / `Tuple` | `list` / `tuple` |
| `Int` / `BigInt` | `int` | `Map` | `dict` |
| `Float` | `float` | `Set` | `set` |
| `String` | `str` | `Date` / `DateTime` | `date` / `datetime` |

Equality and hashing follow Python (`True == 1 == 1.0`, `0.0 == -0.0`,
order-independent dicts, aware date-times compared by instant), so
conditions, `check_one_of` and key sets behave like Python cfgv. `repr()`
is `Value::repr`, `str()` is `Display`. The only deliberate difference is
that NaN equals NaN, which keeps `Eq` lawful for map keys. Sets keep
insertion order (CPython iterates them in hash order).

cfgv does not parse YAML. Build `Value`s with your own loader (to match
pre-commit, a YAML 1.1 / PyYAML `SafeLoader`-compatible one), or with the
`serde` feature from any serde format. `Date::new`, `DateTime::new` and
`TzOffset::from_hours_minutes` report out-of-range components with
CPython's `datetime` error texts, and `BigInt::from_str_radix` handles
arbitrarily large hex, octal and binary integers.

## Building blocks

| Python cfgv | cfgv-rs |
|---|---|
| `Map(name, id_key, *items)` | `Schema::map(name, id_key, items)` |
| `Array(of, allow_empty)` | `Schema::array(of)`, `Schema::array_nonempty(of)` |
| `KeyValueMap(name, check_key, schema)` | `Schema::key_value_map(name, check_key, schema)` |
| `Required`, `RequiredRecurse`, `Optional`, `OptionalRecurse`, `OptionalNoDefault` | `Validator::required`, `required_recurse`, `optional`, `optional_recurse`, `optional_no_default` |
| `Conditional`, `ConditionalOptional`, `ConditionalRecurse` | `Validator::conditional[_with_absent]`, `conditional_optional[_with_absent]`, `conditional_recurse[_with_absent]` |
| `NoAdditionalKeys`, `WarnAdditionalKeys` | `Validator::no_additional_keys`, `Validator::warn_additional_keys` |
| any object with `check`/`apply_default`/`remove_default` | `impl MapItem` + `Validator::custom` |
| `Not`, `In`, `NotIn`, plain value | `ConditionValue::Not`, `In`, `NotIn`, `Exact` |
| `check_any`, `check_bool`, `check_int`, `check_bytes`, `check_string`, `check_text`, `check_type(tp)`, `check_one_of`, `check_regex`, `check_array`, `check_and` | `CheckFn::check_*` (`check_type(ValueType::Map, None)` is `check_type(dict)`) |
| `validate`, `apply_defaults`, `remove_defaults` | `validate[_with]`, `apply_defaults[_owned]`, `remove_defaults[_owned]` |
| `load_from_filename` | `load_from_filename[_with]` |
| `validate_context` | `validate_context` |
| `reraise_as(tp)` | `.map_err(...)` |

## Custom items and warnings

Python validators may log warnings while validating. cfgv threads a `Cx`
context through validation that collects warnings in order; validation
stops at the first error but keeps the warnings emitted before it.

```rust
use cfgv::{CheckFn, Cx, Map, MapItem, Schema, ValidationError, Validator, Value, validate_with, value_map};

/// Like pre-commit's `WarnMutableRev`: extends a built-in validator.
#[derive(Debug)]
struct WarnMutableRev(Validator);

impl MapItem for WarnMutableRev {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        self.0.check(dct, cx)?;
        if let Some(rev) = dct.get(&Value::from("rev")).and_then(Value::as_str) {
            if !rev.contains('.') {
                cx.warn(format!("The 'rev' field appears to be a mutable reference: {rev}"));
            }
        }
        Ok(())
    }

    fn key(&self) -> Option<&Value> {
        self.0.key()
    }
}

let schema = Schema::map("Repository", Some("repo".into()), vec![
    Validator::custom(WarnMutableRev(Validator::optional_no_default("rev", CheckFn::check_string()))),
]);
let mut warnings = Vec::new();
let repo = value_map! {"repo" => "r", "rev" => "main"};
validate_with(&repo, &schema, &mut Cx::collect(&mut warnings)).unwrap();
assert_eq!(warnings, ["The 'rev' field appears to be a mutable reference: main"]);
```

`MapItem` also has `apply_default`/`remove_default` hooks (pre-commit's
stage and language migrations rewrite values there) and `key`/`check_fn`
for introspection, so one schema can be derived from another's items.

## Regular expressions

`check_regex` must accept exactly what Python's `re` accepts, which the
`regex` crate does not (look-around, back-references, `\Z`, `a{`, ...).
Plug in your own validator:

```rust,ignore
let check = cfgv::CheckFn::check_regex_with(|pattern| my_python_re::compile(pattern).is_ok());
```

The failure message is always `{pattern!r} is not a valid python regex`.

## Loading files

`load_from_filename` reproduces Python's file handling: `{name} is not a
file`, strict UTF-8 with CPython's `UnicodeDecodeError` text, universal
newlines, the loader's error as the leaf message, and defaults applied to
the result. Read failures are reported separately as `LoadError::Io`
(Python lets that `OSError` escape).

## Python compatibility helpers

The `cfgv::py` module exposes the CPython 3.12 primitives used for the
messages: `repr_str`, `repr_bytes`, `repr_float`, `is_printable` (Unicode
15.0.0), `decode_utf8` (with `str(UnicodeDecodeError)` as `Display`) and
`universal_newlines`.

## Migrating from 0.2

* `Value` is `#[non_exhaustive]` and gained `BigInt`, `Tuple`, `Set`,
  `Date` and `DateTime`; `PartialEq`/`Hash` follow Python semantics
  (`Value::Int(1) == Value::Bool(true)`).
* `Value::repr` is exact Python `repr()`; e.g. floats are `1.0` and
  `1e+16`, strings choose quotes and escape like Python.
* The `serde_json` and `yaml_serde` features are removed. Enable `serde`
  and use `serde_json::from_str::<cfgv::Value>(...)` (or any serde
  format); `yaml_serde` resolved YAML 1.2 types, not PyYAML's.
* `Validator::check` takes a `&mut Cx`; use `validate_with` /
  `Schema::check_with` to receive warnings (`validate` discards them).
  `WarnAdditionalKeys` callbacks take a fourth `&mut Cx` argument.
* `*Recurse` validators hold `Arc<Schema>` (constructors accept `Schema`
  or `Arc<Schema>`); `Schema::Map` items are `Arc<[Validator]>`.
* `CheckFn::Regex` carries its checker; `check_regex()` requires the
  default `regex` feature, `check_regex_with()` always works.
* `ValueType` gained `List`, `Tuple`, `Map`, `Set`, `Null`, `Date` and
  `DateTime`; `ValueType::Int` (`check_int`) accepts booleans like Python.
* `ValidationError::message()` returns the leaf message;
  `contexts()` iterates the contexts; `Error::source()` is `None`.
* `load_from_filename` returns `Result<Value, LoadError>` and its loader
  may return any error type implementing `Display`.
* `remove_defaults` keeps the order of the remaining keys, and
  `apply_defaults` never replaces data that does not match the schema.

## Testing

Besides unit, integration and property tests, `tests/parity.rs` checks
thousands of cases byte for byte against vectors produced by the real
Python cfgv 3.5.0 on CPython 3.12 (`scripts/gen_parity_vectors.py`). To
regenerate them, run from a directory without `.py` files:

```sh
python3.12 -I scripts/gen_parity_vectors.py <dir containing cfgv.py> tests/vectors
python3.12 -I scripts/gen_unicode_tables.py src/py/tables.rs
```

## License

MIT
