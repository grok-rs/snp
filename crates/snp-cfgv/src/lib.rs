//! Validate configuration and produce human readable error messages.
//!
//! A Rust port of the Python [cfgv](https://github.com/asottile/cfgv)
//! library (3.5.0) — the validation engine behind
//! [pre-commit](https://pre-commit.com) — that reproduces its error messages
//! byte for byte, including Python's `repr()` of every value kind (see
//! [Differences from Python](#differences-from-python) for the exceptions).
//!
//! # Example
//!
//! ```
//! use cfgv::{CheckFn, ConditionValue, Schema, Validator, Value, apply_defaults, validate, value_map};
//!
//! let schema = Schema::map("Repository", Some("repo".into()), vec![
//!     Validator::required("repo", CheckFn::check_string()),
//!     Validator::conditional_with_absent(
//!         "rev",
//!         CheckFn::check_string(),
//!         "repo",
//!         ConditionValue::NotIn(vec!["local".into(), "meta".into()]),
//!         true,
//!     ),
//!     Validator::optional("fail_fast", CheckFn::check_bool(), false),
//! ]);
//!
//! let repo = value_map! {"repo" => "https://github.com/example/repo", "rev" => "v1.0"};
//! validate(&repo, &schema).unwrap();
//! assert_eq!(apply_defaults(&repo, &schema).as_map().unwrap().len(), 3);
//!
//! let err = validate(&value_map! {"repo" => "local", "rev" => "v1.0"}, &schema).unwrap_err();
//! assert_eq!(
//!     err.to_string(),
//!     "\n==> At Repository(repo='local')\n\
//!      =====> Expected rev to be absent when repo is any of ('local', 'meta'), found rev: 'v1.0'",
//! );
//! ```
//!
//! # Building blocks
//!
//! * [`Value`]: the data being validated — every type PyYAML's `SafeLoader`
//!   produces (strings without lone surrogates), with Python equality,
//!   hashing and `repr()`.
//! * [`Schema`]: `Map`, `Array` and `KeyValueMap` containers.
//! * [`Validator`]: the items of a `Map` (`Required`, `Optional`,
//!   `Conditional`, ...), plus [`Validator::Custom`] for user-defined
//!   [`MapItem`]s.
//! * [`CheckFn`]: checks for single values (`check_string`, `check_one_of`,
//!   `check_regex`, ...).
//! * [`Cx`]: the validation context that collects warnings in order.
//! * [`ValidationError`]: the error, rendered like Python's
//!   `str(ValidationError)`; [`ValidationError::uncaught`] stands for the
//!   other Python exceptions that escape cfgv (`TypeError` from
//!   `re.compile`, `ValueError` from `repr()` of a huge integer, ...).
//! * [`load_from_filename`]: read, decode, parse, validate and default a file.
//! * [`py`]: CPython-compatible `repr()`, `str.isprintable()` and UTF-8
//!   decoding helpers.
//!
//! # Features
//!
//! * `serde`: `Serialize`/`Deserialize` for [`Value`] and a `Deserializer`
//!   for `&Value`, to convert validated values into typed structs.
//!
//! # Differences from Python
//!
//! * `NaN == NaN` (keeps `Eq` lawful for map keys), sets keep insertion
//!   order, and sorting values of mixed types (`check_one_of` choices,
//!   unknown keys) groups them by type where Python raises `TypeError`.
//! * [`Value::String`] cannot hold lone surrogates (`"\ud800"` in a
//!   double-quoted YAML scalar); loaders must reject or replace them. See
//!   [`Value`].
//! * Integers with more than 4300 decimal digits: everything CPython raises
//!   `ValueError` for is reproduced ([`BigInt`], [`Value::try_repr`], and
//!   uncaught errors from validation), but the infallible [`Value::repr`]
//!   and `Display` write such integers in hexadecimal.
//! * `check_regex` needs a validator of Python's `re` syntax: plug one in
//!   with [`CheckFn::check_regex_with`] or [`CheckFn::check_regex_try_with`]
//!   (snp uses `snp_pyre::cfgv_is_valid`).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod check;
mod condition;
mod error;
mod load;
pub mod py;
mod schema;
#[cfg(feature = "serde")]
mod serde_impl;
mod validator;
mod value;

pub use check::{CheckFn, CustomCheckFn, RegexCheck, RegexPattern, ValueType};
pub use condition::ConditionValue;
pub use error::{ValidationError, validate_context};
pub use load::{LoadError, load_from_filename, load_from_filename_with};
pub use schema::Schema;
#[cfg(feature = "serde")]
pub use serde_impl::{DeserializeError, from_value};
pub use validator::{Cx, MapItem, Validator, WarnCallback};
pub use value::{
    BigInt, Date, DateError, DateTime, HashedKey, Map, MaxStrDigitsError, ParseBigIntError, Set,
    StrKey, TzOffset, Value, ValueState,
};

/// Validates `value` against `schema`, discarding warnings.
///
/// # Errors
///
/// Returns the first validation error.
pub fn validate(value: &Value, schema: &Schema) -> Result<(), ValidationError> {
    schema.check(value)
}

/// Validates `value` against `schema`, emitting warnings through `cx` in
/// validation order.
///
/// # Errors
///
/// Returns the first validation error; warnings emitted before it have
/// already been passed to `cx`.
pub fn validate_with(
    value: &Value,
    schema: &Schema,
    cx: &mut Cx<'_>,
) -> Result<(), ValidationError> {
    schema.check_with(value, cx)
}

/// Returns a copy of a validated `value` with defaults applied.
#[must_use]
pub fn apply_defaults(value: &Value, schema: &Schema) -> Value {
    schema.apply_defaults(value)
}

/// Applies defaults to a validated `value`, consuming it.
#[must_use]
pub fn apply_defaults_owned(value: Value, schema: &Schema) -> Value {
    schema.apply_defaults_owned(value)
}

/// Returns a copy of a validated `value` without the values that equal their
/// defaults.
#[must_use]
pub fn remove_defaults(value: &Value, schema: &Schema) -> Value {
    schema.remove_defaults(value)
}

/// Removes defaults from a validated `value`, consuming it.
#[must_use]
pub fn remove_defaults_owned(value: Value, schema: &Schema) -> Value {
    schema.remove_defaults_owned(value)
}

/// The examples of `README.md`, compiled and run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
