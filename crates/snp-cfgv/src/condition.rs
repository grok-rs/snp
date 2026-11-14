//! Conditions for the `Conditional*` validators.

use crate::value::{MaxStrDigitsError, Value};

/// The `condition_value` of a `Conditional*` validator: decides, from the
/// value of the condition key, whether the conditional check applies.
///
/// A missing condition key (Python's `MISSING`) never matches.
///
/// ```
/// use cfgv::{ConditionValue, Value};
///
/// let not_local = ConditionValue::NotIn(vec![Value::from("local"), Value::from("meta")]);
/// assert!(not_local.matches(Some(&Value::from("https://example.com/repo"))));
/// assert!(!not_local.matches(Some(&Value::from("local"))));
/// assert!(!not_local.matches(None));
/// assert_eq!(not_local.describe_opposite(), "is any of ('local', 'meta')");
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ConditionValue {
    /// A plain condition value: matches when the value equals it (Python
    /// `==`).
    Exact(Value),
    /// `cfgv.Not(v)`: matches when the value is present and not equal to
    /// `v`.
    Not(Value),
    /// `cfgv.In(*values)`: matches when the value is present and equal to
    /// one of the values.
    In(Vec<Value>),
    /// `cfgv.NotIn(*values)`: matches when the value is present and equal to
    /// none of the values.
    NotIn(Vec<Value>),
}

impl ConditionValue {
    /// Whether the condition matches the condition key's value; `None`
    /// stands for a missing key.
    #[must_use]
    pub fn matches(&self, value: Option<&Value>) -> bool {
        let Some(value) = value else {
            return false;
        };
        match self {
            Self::Exact(expected) => value == expected,
            Self::Not(excluded) => value != excluded,
            Self::In(values) => values.contains(value),
            Self::NotIn(values) => !values.contains(value),
        }
    }

    /// Fails when [`ConditionValue::describe_opposite`] would raise in
    /// Python (an integer beyond CPython's digit limit).
    pub(crate) fn check_str_digits(&self) -> Result<(), MaxStrDigitsError> {
        match self {
            Self::Exact(v) | Self::Not(v) => v.check_str_digits(),
            Self::In(values) | Self::NotIn(values) => {
                values.iter().try_for_each(Value::check_str_digits)
            }
        }
    }

    /// The explanation used in `ensure_absent` errors (`describe_opposite`
    /// in Python): `is not {v!r}`, `is {v!r}`, `is not any of {values!r}` or
    /// `is any of {values!r}`, where `values` is shown as a Python tuple.
    #[must_use]
    pub fn describe_opposite(&self) -> String {
        match self {
            Self::Exact(v) => format!("is not {}", v.repr()),
            Self::Not(v) => format!("is {}", v.repr()),
            Self::In(values) => format!("is not any of {}", tuple_repr(values)),
            Self::NotIn(values) => format!("is any of {}", tuple_repr(values)),
        }
    }
}

/// `repr(tuple(values))`, including the trailing comma of a 1-tuple.
fn tuple_repr(values: &[Value]) -> String {
    Value::Tuple(values.to_vec()).repr()
}
