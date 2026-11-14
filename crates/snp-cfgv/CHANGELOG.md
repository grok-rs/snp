# Changelog

Frozen history of the standalone cfgv-rs crate up to its import into snp at `6a04a30` (ADR-116). Later changes to
`crates/snp-cfgv` are recorded in snp's `CHANGELOG.md`.

All notable changes to this project are documented in this file. The format
is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/).

## [0.3.0] - Unreleased

Byte-for-byte parity with Python cfgv 3.5.0 (as used by pre-commit) on
CPython 3.12. See the README for migration notes.

### Added

- `MapItem` trait and `Validator::Custom` for user-defined map items with
  `check`/`apply_default`/`remove_default` hooks (pre-commit's
  `StagesMigration`, `WarnMutableRev`, `NotAllowed`, ... can now be
  expressed); `Validator` implements `MapItem` so custom items can wrap
  built-ins.
- `Cx` validation context collecting warnings in validation order;
  `validate_with`, `Schema::check_with` and `load_from_filename_with`
  accept it, and `WarnAdditionalKeys` callbacks receive it.
- Introspection: `Validator::{key, check_fn, schema, default_value}`,
  `Schema::{items, object_name}`.
- `Value` variants for every PyYAML `SafeLoader` type: `BigInt`
  (arbitrary-precision `int`), `Tuple`, `Set`, `Date` and `DateTime` (with
  fixed UTC offsets via `TzOffset`); `BigInt`, `Date`, `DateTime`,
  `TzOffset` and `DateError` types (`DateError` displays CPython's
  `datetime` messages); `Map`/`Set` type aliases; more `From` impls and
  accessors (`as_float`, `as_bytes`, `as_array`, `as_map_mut`, ...).
- `BigInt` stores a binary magnitude: `from_str_radix` parses hexadecimal,
  octal and binary text of any length in linear time, and `bit_length`,
  `to_u64`, `to_u128`, `to_decimal_string` and `LowerHex`/`UpperHex`
  (`{:#x}` is Python's `hex()`) are available.
- CPython's 4300-digit limit on `int`/`str` conversions:
  `ParseBigIntError::ExceedsMaxStrDigits` (CPython's message) for longer
  decimal input, and `MaxStrDigitsError` from `BigInt::to_decimal_string`,
  `Value::try_repr` and `Value::try_to_string` where `str()`/`repr()`
  raise `ValueError`. Validation raises that `ValueError` as an uncaught
  error wherever Python formats such an integer, including the eagerly
  built `At Hook(id=...)` and `For key: ...` contexts.
- `ValidationError::uncaught`, `uncaught_type` and `is_uncaught` represent
  Python exceptions other than `ValidationError` that escape cfgv (no
  contexts, not wrapped by `load_from_filename`, displayed as
  `{type}: {message}`), e.g. from custom checks such as pre-commit's
  `check_min_version`.
- `ValueType::{List, Tuple, Map, Set, Null, Date, DateTime}`
  (`check_type(ValueType::Map, None)` is Python's `check_type(dict)`).
- `CheckFn::check_regex_with` to plug in a validator for Python's `re`
  dialect, `CheckFn::check_regex_try_with` (with `RegexCheck` and
  `RegexPattern`) for a validator that sees `str` and `bytes` patterns and
  can raise like `re.compile` (`OverflowError`, `RecursionError`), and
  `CheckFn::check_any`.
- `py` module with CPython-exact helpers: `repr_str`, `repr_bytes`,
  `repr_float`, `is_printable` (Unicode 15.0.0), `decode_utf8` /
  `Utf8DecodeError` and `universal_newlines`.
- By-value `apply_defaults_owned` / `remove_defaults_owned`.
- `LoadError` distinguishing validation failures from I/O errors. Its
  `Error::source()` is `None` (the `Display` text already includes the OS
  error); the `io::Error` is in `LoadError::Io::error`.
- Optional `serde` feature: `Deserializer` for `&Value`, `from_value`,
  `Serialize` and `Deserialize` for `Value`. Integer targets accept
  booleans like `check_int`, and struct fields and enum variants match
  keys by their Python `str()`, so non-string keys (YAML `1:`, `true:`)
  are ignored or reported like unknown string keys.
- Parity test suite generated from the real Python cfgv
  (`scripts/gen_parity_vectors.py`, `tests/vectors/`), including the
  exceptions that escape cfgv and the integer digit limit; API tests,
  property tests and `cargo-fuzz` targets (`fuzz/`); CI job checking that
  the vectors are up to date.

### Changed

- **Breaking:** `Value::repr` is exact Python `repr()` (quote selection and
  escapes for strings, `1.0`/`1e+16`/`1e-05`/`nan` for floats, `b'...'`
  for bytes, `('a',)` for 1-tuples, ...), and `Display` is Python's
  `str()`.
- **Breaking:** `Value` equality and hashing follow Python
  (`True == 1 == 1.0`, `0.0 == -0.0`, exact int/float comparison,
  order-independent dicts and sets with a matching hash).
- **Breaking:** `ensure_absent` messages use `repr()` and tuple syntax:
  `repo is any of ('local', 'meta'), found rev: 'v1.0'`.
- **Breaking:** the invalid-regex message uses `repr()` of the pattern.
  `check_regex` also checks `bytes` patterns (`b'(' is not a valid python
  regex`) and, like `re.compile`, raises an uncaught `TypeError` for any
  other type instead of reporting `Expected string got {type}`.
- **Breaking:** `check_one_of` and the additional-keys validators sort with
  Python's `sorted()` order (numbers numerically).
- **Breaking:** `check_int` (`ValueType::Int`) accepts booleans and big
  integers, and `check_array` accepts tuples, like Python.
- **Breaking:** `Validator::check` takes a `&mut Cx`; `WarnAdditionalKeys`
  callbacks take a fourth `&mut Cx` argument.
- **Breaking:** `*Recurse` validators hold `Arc<Schema>`, `Schema::Map`
  items are `Arc<[Validator]>`, so sub-schemas are shared, not cloned.
- **Breaking:** `CheckFn::Regex` carries its checker; the `regex`
  dependency is optional (default feature `regex`).
- **Breaking:** `ValidationError::message()` returns the leaf message, new
  `contexts()`; `Error::source()` is `None` (no duplicated lines in error
  reporters).
- **Breaking:** `load_from_filename` returns `LoadError`, accepts any
  `FnOnce` loader whose error implements `Display`, decodes UTF-8 with
  CPython's error text and applies universal newlines.
- **Breaking:** `Value`, `ValueType`, `CheckFn`, `ConditionValue`,
  `Validator`, `Schema` and `LoadError` are `#[non_exhaustive]`.
- `apply_defaults` no longer deep-clones nested values for every
  recursive item, and never replaces data that does not match the schema
  with an empty map or list.

### Removed

- **Breaking:** the `serde_json` and `yaml_serde` features and their lossy
  `From` conversions (`yaml_serde` resolves YAML 1.2 types, not PyYAML's,
  and pulls in a large amount of `unsafe` code). Use the `serde` feature.

### Fixed

- `check_type(tp, Some(""))` falls back to the type name, like Python's
  `typename or tp.__name__`.
- `remove_defaults` preserves the order of the remaining keys.
- `Hash` of `Value::Map` is consistent with its order-independent `Eq`.
- The declared MSRV (1.85) is now accurate and tested in CI (the toolchain
  file no longer masks the MSRV job).
- `#![forbid(unsafe_code)]`.

### Known limitations

- `Value::String` cannot hold lone surrogates, which PyYAML produces for
  escapes such as `"\ud800"`; loaders must reject or replace them.
- `Value::repr` and `Display` write integers beyond CPython's 4300-digit
  limit in hexadecimal, where Python raises (use `Value::try_repr`).

## [0.2.0] - 2026-04-05

### Added

- `yaml_serde` feature with `From<yaml_serde::Value>` for `Value`.

## [0.1.0] - 2026-04-05

- Initial release: a Rust port of Python cfgv.

[0.3.0]: https://github.com/grok-rs/cfgv-rs/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/grok-rs/cfgv-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/grok-rs/cfgv-rs/releases/tag/v0.1.0
