use std::sync::Arc;

use crate::error::ValidationError;
use crate::value::Value;

/// A user-provided custom check function.
pub type CustomCheckFn = Arc<dyn Fn(&Value) -> Result<(), ValidationError> + Send + Sync>;

/// The type of a Value, used for type-checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Bool,
    Int,
    Float,
    String,
    Bytes,
}

impl ValueType {
    fn name(self) -> &'static str {
        match self {
            ValueType::Bool => "bool",
            ValueType::Int => "int",
            ValueType::Float => "float",
            ValueType::String => "str",
            ValueType::Bytes => "bytes",
        }
    }

    fn matches(self, value: &Value) -> bool {
        matches!(
            (self, value),
            (ValueType::Bool, Value::Bool(_))
                | (ValueType::Int, Value::Int(_))
                | (ValueType::Float, Value::Float(_))
                | (ValueType::String, Value::String(_))
                | (ValueType::Bytes, Value::Bytes(_))
        )
    }
}

/// A check function that validates a single value.
///
/// Mirrors Python's check functions like `check_bool`, `check_string`,
/// `check_one_of`, `check_regex`, `check_array`, `check_and`.
#[derive(Clone)]
pub enum CheckFn {
    /// Accepts any value (noop).
    Any,
    /// Checks that the value is of a specific type.
    /// The optional string overrides the type name in error messages.
    Type(ValueType, Option<&'static str>),
    /// Checks that the value is one of the given possibilities.
    OneOf(Vec<Value>),
    /// Checks that the value is a valid regex.
    Regex,
    /// Checks that the value is a list/tuple and each element passes the inner check.
    Array(Box<CheckFn>),
    /// Composes multiple check functions — all must pass.
    And(Vec<CheckFn>),
    /// A user-provided custom check function.
    Custom(CustomCheckFn),
}

impl std::fmt::Debug for CheckFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckFn::Any => write!(f, "CheckFn::Any"),
            CheckFn::Type(t, n) => write!(f, "CheckFn::Type({t:?}, {n:?})"),
            CheckFn::OneOf(v) => write!(f, "CheckFn::OneOf({v:?})"),
            CheckFn::Regex => write!(f, "CheckFn::Regex"),
            CheckFn::Array(inner) => write!(f, "CheckFn::Array({inner:?})"),
            CheckFn::And(fns) => write!(f, "CheckFn::And({fns:?})"),
            CheckFn::Custom(_) => write!(f, "CheckFn::Custom(...)"),
        }
    }
}

impl CheckFn {
    /// Validate a value against this check function.
    pub fn check(&self, value: &Value) -> Result<(), ValidationError> {
        match self {
            CheckFn::Any => Ok(()),

            CheckFn::Type(expected_type, typename_override) => {
                if expected_type.matches(value) {
                    Ok(())
                } else {
                    let expected = typename_override.unwrap_or_else(|| expected_type.name());
                    Err(ValidationError::new(format!(
                        "Expected {expected} got {}",
                        value.type_name()
                    )))
                }
            }

            CheckFn::OneOf(possible) => {
                if possible.contains(value) {
                    Ok(())
                } else {
                    let mut sorted: Vec<&Value> = possible.iter().collect();
                    crate::value::py_sort(&mut sorted);
                    let sorted_strs: Vec<String> = sorted.iter().map(|v| v.to_string()).collect();
                    let possible_s = sorted_strs.join(", ");
                    Err(ValidationError::new(format!(
                        "Expected one of {possible_s} but got: {}",
                        value.repr()
                    )))
                }
            }

            CheckFn::Regex => {
                let s = value.as_str().ok_or_else(|| {
                    ValidationError::new(format!("Expected string got {}", value.type_name()))
                })?;
                regex::Regex::new(s).map_err(|_| {
                    ValidationError::new(format!("'{s}' is not a valid python regex"))
                })?;
                Ok(())
            }

            CheckFn::Array(inner_check) => {
                let list = match value {
                    Value::List(l) => l,
                    _ => {
                        return Err(ValidationError::new(format!(
                            "Expected array but got '{}'",
                            value.type_name()
                        )));
                    }
                };
                for (i, val) in list.iter().enumerate() {
                    crate::error::validate_context(
                        || format!("At index {i}"),
                        || inner_check.check(val),
                    )?;
                }
                Ok(())
            }

            CheckFn::And(fns) => {
                for check_fn in fns {
                    check_fn.check(value)?;
                }
                Ok(())
            }

            CheckFn::Custom(f) => f(value),
        }
    }

    // --- Convenience constructors ---

    pub fn check_bool() -> Self {
        CheckFn::Type(ValueType::Bool, None)
    }

    pub fn check_int() -> Self {
        CheckFn::Type(ValueType::Int, None)
    }

    pub fn check_string() -> Self {
        CheckFn::Type(ValueType::String, Some("string"))
    }

    pub fn check_text() -> Self {
        CheckFn::Type(ValueType::String, Some("text"))
    }

    pub fn check_bytes() -> Self {
        CheckFn::Type(ValueType::Bytes, Some("bytes"))
    }

    pub fn check_type(vt: ValueType, typename: Option<&'static str>) -> Self {
        CheckFn::Type(vt, typename)
    }

    pub fn check_one_of(possible: Vec<Value>) -> Self {
        CheckFn::OneOf(possible)
    }

    pub fn check_regex() -> Self {
        CheckFn::Regex
    }

    pub fn check_array(inner: CheckFn) -> Self {
        CheckFn::Array(Box::new(inner))
    }

    pub fn check_and(fns: Vec<CheckFn>) -> Self {
        CheckFn::And(fns)
    }

    pub fn custom<F>(f: F) -> Self
    where
        F: Fn(&Value) -> Result<(), ValidationError> + Send + Sync + 'static,
    {
        CheckFn::Custom(Arc::new(f))
    }
}
