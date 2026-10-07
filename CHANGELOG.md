# Changelog

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
- `ValueType::{List, Tuple, Map, Set, Null, Date, DateTime}`
  (`check_type(ValueType::Map, None)` is Python's `check_type(dict)`).
- `CheckFn::check_regex_with` to plug in a validator for Python's `re`
  dialect, `CheckFn::check_any`, and `RegexCheck`.
- `py` module with CPython-exact helpers: `repr_str`, `repr_bytes`,
  `repr_float`, `is_printable` (Unicode 15.0.0), `decode_utf8` /
  `Utf8DecodeError` and `universal_newlines`.
- By-value `apply_defaults_owned` / `remove_defaults_owned`.
- `LoadError` distinguishing validation failures from I/O errors.
- Optional `serde` feature: `Deserializer` for `&Value`, `from_value`,
  `Serialize` and `Deserialize` for `Value`.
- Parity test suite generated from the real Python cfgv
  (`scripts/gen_parity_vectors.py`, `tests/vectors/`), API tests and
  property tests; CI job checking that the vectors are up to date.

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

- `remove_defaults` preserves the order of the remaining keys.
- `Hash` of `Value::Map` is consistent with its order-independent `Eq`.
- The declared MSRV (1.85) is now accurate and tested in CI (the toolchain
  file no longer masks the MSRV job).
- `#![forbid(unsafe_code)]`.

## [0.2.0] - 2026-04-05

### Added

- `yaml_serde` feature with `From<yaml_serde::Value>` for `Value`.

## [0.1.0] - 2026-04-05

- Initial release: a Rust port of Python cfgv.

[0.3.0]: https://github.com/grok-rs/cfgv-rs/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/grok-rs/cfgv-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/grok-rs/cfgv-rs/releases/tag/v0.1.0
