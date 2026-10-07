//! Validate configuration and produce human readable error messages.
//!
//! A Rust port of the Python [cfgv](https://github.com/asottile/cfgv)
//! library (3.5.0) — the validation engine behind
//! [pre-commit](https://pre-commit.com) — that reproduces its error messages
//! byte for byte, including Python's `repr()` of every value kind.
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
//!   produces, with Python equality, hashing and `repr()`.
//! * [`Schema`]: `Map`, `Array` and `KeyValueMap` containers.
//! * [`Validator`]: the items of a `Map` (`Required`, `Optional`,
//!   `Conditional`, ...), plus [`Validator::Custom`] for user-defined
//!   [`MapItem`]s.
//! * [`CheckFn`]: checks for single values (`check_string`, `check_one_of`,
//!   `check_regex`, ...).
//! * [`Cx`]: the validation context that collects warnings in order.
//! * [`load_from_filename`]: read, decode, parse, validate and default a file.
//! * [`py`]: CPython-compatible `repr()`, `str.isprintable()` and UTF-8
//!   decoding helpers.
//!
//! # Features
//!
//! * `regex` (default): [`CheckFn::check_regex`] backed by the `regex`
//!   crate. Without it, supply a validator with
//!   [`CheckFn::check_regex_with`].
//! * `serde`: `Serialize`/`Deserialize` for [`Value`] and a `Deserializer`
//!   for `&Value`, to convert validated values into typed structs.

#![forbid(unsafe_code)]

mod check;
mod condition;
mod error;
mod load;
pub mod py;
mod schema;
mod validator;
mod value;

pub use check::{CheckFn, CustomCheckFn, RegexCheck, ValueType};
pub use condition::ConditionValue;
pub use error::{ValidationError, validate_context};
pub use load::load_from_filename;
pub use schema::Schema;
pub use validator::{Cx, MapItem, Validator, WarnCallback};
pub use value::{
    BigInt, Date, DateError, DateTime, Map, ParseBigIntError, Set, TzOffset, Value,
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
