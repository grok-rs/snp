//! # cfgv
//!
//! Validate configuration and produce human readable error messages.
//!
//! A Rust port of the Python [cfgv](https://github.com/asottile/cfgv) library.
//!
//! # Example
//!
//! ```
//! use cfgv::{Schema, Validator, CheckFn, Value, value_map, validate};
//!
//! let schema = Schema::map(
//!     "Repo", Some(Value::from("url")),
//!     vec![
//!         Validator::required("url", CheckFn::check_string()),
//!         Validator::optional("rev", CheckFn::check_string(), "HEAD"),
//!     ],
//! );
//!
//! let config = value_map! {
//!     "url" => "https://github.com/example/repo",
//!     "rev" => "main",
//! };
//!
//! validate(&config, &schema).unwrap();
//! ```

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
pub use validator::{Validator, WarnCallback};
pub use value::{BigInt, Date, DateError, DateTime, Map, ParseBigIntError, Set, TzOffset, Value};

/// Hidden re-export for use by the `value_map!` macro.
#[doc(hidden)]
pub use indexmap::IndexMap as __IndexMap;

/// Validate a value against a schema.
///
/// Returns `Ok(())` on success, or a `ValidationError` on failure.
pub fn validate(value: &Value, schema: &Schema) -> Result<(), ValidationError> {
    schema.check(value)
}

/// Apply defaults to a value according to a schema.
///
/// Returns a new value with all missing optional fields set to their defaults.
pub fn apply_defaults(value: &Value, schema: &Schema) -> Value {
    schema.apply_defaults(value)
}

/// Remove defaults from a value according to a schema.
///
/// Returns a new value with all optional fields that equal their defaults removed.
pub fn remove_defaults(value: &Value, schema: &Schema) -> Value {
    schema.remove_defaults(value)
}
