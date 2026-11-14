//! Items of a `Map` schema: built-in validators, custom items and the
//! warning context.

use std::fmt;
use std::sync::Arc;

use crate::check::{CheckFn, join_str};
use crate::condition::ConditionValue;
use crate::error::{ValidationError, validate_context};
use crate::schema::Schema;
use crate::value::{Map, Value, py_sort};

/// The validation context threaded through every check: an ordered sink for
/// the warnings that validators emit.
///
/// Python validators log warnings while validating (pre-commit's
/// `WarnMutableRev`, `DeprecatedStagesWarning`, unknown-key callbacks, ...).
/// cfgv collects them in validation order, so a caller can print them exactly
/// where Python would have logged them. Validation stops at the first error;
/// warnings emitted before it are kept.
///
/// ```
/// use cfgv::{Cx, Schema, Validator, validate_with, value_map};
///
/// let schema = Schema::map("Config", None, vec![Validator::warn_additional_keys(
///     vec!["repos".into()],
///     |extra, _keys, _dct, cx| cx.warn(format!("Unexpected key(s) present at root: {}", extra.len())),
/// )]);
/// let mut warnings = Vec::new();
/// validate_with(&value_map! {"x" => 1}, &schema, &mut Cx::collect(&mut warnings)).unwrap();
/// assert_eq!(warnings, ["Unexpected key(s) present at root: 1"]);
/// ```
pub struct Cx<'a> {
    sink: Sink<'a>,
}

enum Sink<'a> {
    Discard,
    Collect(&'a mut Vec<String>),
    Callback(Box<dyn FnMut(String) + 'a>),
}

impl<'a> Cx<'a> {
    /// A context that passes every warning to `sink`, in order.
    #[must_use]
    pub fn new<F: FnMut(String) + 'a>(sink: F) -> Self {
        Self {
            sink: Sink::Callback(Box::new(sink)),
        }
    }

    /// A context that appends every warning to `warnings`.
    #[must_use]
    pub const fn collect(warnings: &'a mut Vec<String>) -> Self {
        Self {
            sink: Sink::Collect(warnings),
        }
    }

    /// A context that drops all warnings (what [`validate`](crate::validate)
    /// uses).
    #[must_use]
    pub const fn discard() -> Self {
        Self {
            sink: Sink::Discard,
        }
    }

    /// Emits a warning.
    pub fn warn(&mut self, message: impl Into<String>) {
        match &mut self.sink {
            Sink::Discard => {}
            Sink::Collect(warnings) => warnings.push(message.into()),
            Sink::Callback(f) => f(message.into()),
        }
    }
}

impl Default for Cx<'_> {
    fn default() -> Self {
        Self::discard()
    }
}

impl fmt::Debug for Cx<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.sink {
            Sink::Discard => "discard",
            Sink::Collect(_) => "collect",
            Sink::Callback(_) => "callback",
        };
        f.debug_struct("Cx").field("sink", &kind).finish()
    }
}

/// An item of a `Map` schema: the Rust counterpart of the objects with
/// `check`, `apply_default` and `remove_default` methods that Python cfgv
/// accepts in `Map(...)`.
///
/// Implement it for validators cfgv does not provide (pre-commit's
/// `StagesMigration`, `WarnMutableRev`, ...) and add them to a schema with
/// [`Validator::custom`]. A custom item can wrap a built-in [`Validator`]
/// (which implements `MapItem` itself) to extend it, like a Python subclass.
///
/// ```
/// use cfgv::{Cx, Map, MapItem, Schema, ValidationError, Validator, Value, validate, value_map};
///
/// /// pre-commit's `NotAllowed('entry')`.
/// #[derive(Debug)]
/// struct NotAllowed(Value);
///
/// impl MapItem for NotAllowed {
///     fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
///         if dct.contains_key(&self.0) {
///             return Err(ValidationError::new(format!("{} cannot be overridden", self.0.repr())));
///         }
///         Ok(())
///     }
///
///     fn key(&self) -> Option<&Value> {
///         Some(&self.0)
///     }
/// }
///
/// let schema = Schema::map("Hook", Some("id".into()), vec![
///     Validator::custom(NotAllowed("entry".into())),
/// ]);
/// let err = validate(&value_map! {"id" => "x", "entry" => "y"}, &schema).unwrap_err();
/// assert_eq!(err.to_string(), "\n==> At Hook(id='x')\n=====> 'entry' cannot be overridden");
/// ```
pub trait MapItem: Send + Sync + fmt::Debug {
    /// Validates the map, emitting any warnings through `cx`.
    ///
    /// # Errors
    ///
    /// Returns the validation error; the enclosing schema adds the
    /// `At {object_name}(...)` context.
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError>;

    /// Applies this item's defaults to an already validated map.
    fn apply_default(&self, dct: &mut Map) {
        let _: &mut Map = dct;
    }

    /// Removes this item's defaults from an already validated map.
    fn remove_default(&self, dct: &mut Map) {
        let _: &mut Map = dct;
    }

    /// The key this item validates, for introspection.
    fn key(&self) -> Option<&Value> {
        None
    }

    /// The check function this item applies to its key, for introspection.
    fn check_fn(&self) -> Option<&CheckFn> {
        None
    }
}

/// Callback of [`Validator::WarnAdditionalKeys`]: receives the extra keys
/// (sorted like Python's `sorted()`), the allowed keys, the map and the
/// warning context.
pub type WarnCallback = Arc<dyn Fn(&[Value], &[Value], &Map, &mut Cx<'_>) + Send + Sync>;

/// An item of a `Map` schema, mirroring Python cfgv's `Required`,
/// `Optional`, `Conditional`, ... plus [`Validator::Custom`] for
/// user-defined items.
///
/// Items are checked in declaration order and the first error aborts the
/// map's validation. Schemas nested in the `*Recurse` variants are shared
/// through [`Arc`], so the same sub-schema can appear in several places
/// without being copied.
#[derive(Clone)]
#[non_exhaustive]
pub enum Validator {
    /// `Required(key, check_fn)`.
    Required {
        /// The key.
        key: Value,
        /// Check applied to the value.
        check_fn: CheckFn,
    },
    /// `RequiredRecurse(key, schema)`.
    RequiredRecurse {
        /// The key.
        key: Value,
        /// Schema the value is validated against.
        schema: Arc<Schema>,
    },
    /// `Optional(key, check_fn, default)`.
    Optional {
        /// The key.
        key: Value,
        /// Check applied to the value when present.
        check_fn: CheckFn,
        /// Value set by `apply_defaults` when the key is missing.
        default: Value,
    },
    /// `OptionalRecurse(key, schema, default)`.
    OptionalRecurse {
        /// The key.
        key: Value,
        /// Schema the value is validated against when present.
        schema: Arc<Schema>,
        /// Value set by `apply_defaults` when the key is missing.
        default: Value,
    },
    /// `OptionalNoDefault(key, check_fn)`.
    OptionalNoDefault {
        /// The key.
        key: Value,
        /// Check applied to the value when present.
        check_fn: CheckFn,
    },
    /// `Conditional(key, check_fn, condition_key, condition_value,
    /// ensure_absent)`: the key is required when the condition matches.
    Conditional {
        /// The key.
        key: Value,
        /// Check applied to the value.
        check_fn: CheckFn,
        /// Key whose value is tested.
        condition_key: Value,
        /// The condition.
        condition_value: ConditionValue,
        /// Reject the key when the condition key is present but does not
        /// match.
        ensure_absent: bool,
    },
    /// `ConditionalOptional(key, check_fn, default, condition_key,
    /// condition_value, ensure_absent)`.
    ConditionalOptional {
        /// The key.
        key: Value,
        /// Check applied to the value when present.
        check_fn: CheckFn,
        /// Value set by `apply_defaults` when the condition matches.
        default: Value,
        /// Key whose value is tested.
        condition_key: Value,
        /// The condition.
        condition_value: ConditionValue,
        /// Reject the key when the condition key is present but does not
        /// match.
        ensure_absent: bool,
    },
    /// `ConditionalRecurse(key, schema, condition_key, condition_value,
    /// ensure_absent)`.
    ConditionalRecurse {
        /// The key.
        key: Value,
        /// Schema the value is validated against.
        schema: Arc<Schema>,
        /// Key whose value is tested.
        condition_key: Value,
        /// The condition.
        condition_value: ConditionValue,
        /// Reject the key when the condition key is present but does not
        /// match.
        ensure_absent: bool,
    },
    /// `NoAdditionalKeys(keys)`.
    NoAdditionalKeys {
        /// The allowed keys, in the order shown in the error message.
        keys: Vec<Value>,
    },
    /// `WarnAdditionalKeys(keys, callback)`.
    WarnAdditionalKeys {
        /// The allowed keys.
        keys: Vec<Value>,
        /// Called with the extra keys when there are any.
        callback: WarnCallback,
    },
    /// A user-defined item.
    Custom(Arc<dyn MapItem>),
}

impl fmt::Debug for Validator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Required { key, check_fn } => f
                .debug_struct("Required")
                .field("key", key)
                .field("check_fn", check_fn)
                .finish(),
            Self::RequiredRecurse { key, schema } => f
                .debug_struct("RequiredRecurse")
                .field("key", key)
                .field("schema", schema)
                .finish(),
            Self::Optional {
                key,
                check_fn,
                default,
            } => f
                .debug_struct("Optional")
                .field("key", key)
                .field("check_fn", check_fn)
                .field("default", default)
                .finish(),
            Self::OptionalRecurse {
                key,
                schema,
                default,
            } => f
                .debug_struct("OptionalRecurse")
                .field("key", key)
                .field("schema", schema)
                .field("default", default)
                .finish(),
            Self::OptionalNoDefault { key, check_fn } => f
                .debug_struct("OptionalNoDefault")
                .field("key", key)
                .field("check_fn", check_fn)
                .finish(),
            Self::Conditional {
                key,
                check_fn,
                condition_key,
                condition_value,
                ensure_absent,
            } => f
                .debug_struct("Conditional")
                .field("key", key)
                .field("check_fn", check_fn)
                .field("condition_key", condition_key)
                .field("condition_value", condition_value)
                .field("ensure_absent", ensure_absent)
                .finish(),
            Self::ConditionalOptional {
                key,
                check_fn,
                default,
                condition_key,
                condition_value,
                ensure_absent,
            } => f
                .debug_struct("ConditionalOptional")
                .field("key", key)
                .field("check_fn", check_fn)
                .field("default", default)
                .field("condition_key", condition_key)
                .field("condition_value", condition_value)
                .field("ensure_absent", ensure_absent)
                .finish(),
            Self::ConditionalRecurse {
                key,
                schema,
                condition_key,
                condition_value,
                ensure_absent,
            } => f
                .debug_struct("ConditionalRecurse")
                .field("key", key)
                .field("schema", schema)
                .field("condition_key", condition_key)
                .field("condition_value", condition_value)
                .field("ensure_absent", ensure_absent)
                .finish(),
            Self::NoAdditionalKeys { keys } => f
                .debug_struct("NoAdditionalKeys")
                .field("keys", keys)
                .finish(),
            Self::WarnAdditionalKeys { keys, .. } => f
                .debug_struct("WarnAdditionalKeys")
                .field("keys", keys)
                .finish_non_exhaustive(),
            Self::Custom(item) => f.debug_tuple("Custom").field(item).finish(),
        }
    }
}

// --- Check logic (cfgv.py `_check_*`) --------------------------------------

/// `_require_key`.
fn require_key(key: &Value, dct: &Map) -> Result<(), ValidationError> {
    if dct.contains_key(key) {
        Ok(())
    } else {
        Err(ValidationError::new(format!(
            "Missing required key: {}",
            key.try_to_string()?
        )))
    }
}

/// `_check_optional` with an arbitrary check of the value.
fn check_optional_with<F>(key: &Value, dct: &Map, check: F) -> Result<(), ValidationError>
where
    F: FnOnce(&Value) -> Result<(), ValidationError>,
{
    match dct.get(key) {
        Some(val) => {
            // Python formats the context before running the check.
            key.check_str_digits()?;
            validate_context(|| format!("At key: {key}"), || check(val))
        }
        None => Ok(()),
    }
}

/// `_check_required` with an arbitrary check of the value.
fn check_required_with<F>(key: &Value, dct: &Map, check: F) -> Result<(), ValidationError>
where
    F: FnOnce(&Value) -> Result<(), ValidationError>,
{
    require_key(key, dct)?;
    check_optional_with(key, dct, check)
}

/// The `ensure_absent` branch of `_check_conditional`.
fn check_ensure_absent(
    key: &Value,
    condition_key: &Value,
    condition_value: &ConditionValue,
    ensure_absent: bool,
    dct: &Map,
) -> Result<(), ValidationError> {
    if !ensure_absent || !dct.contains_key(condition_key) {
        return Ok(());
    }
    match dct.get(key) {
        Some(found) => {
            condition_value.check_str_digits()?;
            let key = key.try_to_string()?;
            Err(ValidationError::new(format!(
                "Expected {key} to be absent when {} {}, found {key}: {}",
                condition_key.try_to_string()?,
                condition_value.describe_opposite(),
                found.try_repr()?,
            )))
        }
        None => Ok(()),
    }
}

/// `sorted(set(dct) - set(keys))`.
fn extra_keys<'a>(dct: &'a Map, keys: &[Value]) -> Vec<&'a Value> {
    let mut extra: Vec<&Value> = dct.keys().filter(|k| !keys.contains(k)).collect();
    py_sort(&mut extra);
    extra
}

/// Applies or removes defaults of a nested schema in place.
fn recurse_in_place(dct: &mut Map, key: &Value, schema: &Schema, apply: bool) {
    if let Some(slot) = dct.get_mut(key) {
        if apply {
            schema.apply_defaults_in_place(slot);
        } else {
            schema.remove_defaults_in_place(slot);
        }
    }
}

/// `dct.setdefault(key, default)`.
fn set_default(dct: &mut Map, key: &Value, default: &Value) {
    if !dct.contains_key(key) {
        dct.insert(key.clone(), default.clone());
    }
}

/// `if dct.get(key, MISSING) == default: del dct[key]` (order-preserving).
fn remove_if_default(dct: &mut Map, key: &Value, default: &Value) {
    if dct.get(key) == Some(default) {
        dct.shift_remove(key);
    }
}

#[expect(
    clippy::same_name_method,
    reason = "the inherent methods are the MapItem implementation's bodies, callable without the trait in scope"
)]
impl Validator {
    /// Validates the map against this item, emitting warnings through `cx`.
    ///
    /// # Errors
    ///
    /// Returns the item's validation error.
    pub fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        match self {
            Self::Required { key, check_fn } => {
                check_required_with(key, dct, |v| check_fn.check(v))
            }
            Self::RequiredRecurse { key, schema } => {
                check_required_with(key, dct, |v| schema.check_with(v, cx))
            }
            Self::Optional { key, check_fn, .. } | Self::OptionalNoDefault { key, check_fn } => {
                check_optional_with(key, dct, |v| check_fn.check(v))
            }
            Self::OptionalRecurse { key, schema, .. } => {
                check_optional_with(key, dct, |v| schema.check_with(v, cx))
            }
            Self::Conditional {
                key,
                check_fn,
                condition_key,
                condition_value,
                ensure_absent,
            } => {
                if condition_value.matches(dct.get(condition_key)) {
                    check_required_with(key, dct, |v| check_fn.check(v))
                } else {
                    check_ensure_absent(key, condition_key, condition_value, *ensure_absent, dct)
                }
            }
            Self::ConditionalOptional {
                key,
                check_fn,
                condition_key,
                condition_value,
                ensure_absent,
                ..
            } => {
                if condition_value.matches(dct.get(condition_key)) {
                    check_optional_with(key, dct, |v| check_fn.check(v))
                } else {
                    check_ensure_absent(key, condition_key, condition_value, *ensure_absent, dct)
                }
            }
            Self::ConditionalRecurse {
                key,
                schema,
                condition_key,
                condition_value,
                ensure_absent,
            } => {
                if condition_value.matches(dct.get(condition_key)) {
                    check_required_with(key, dct, |v| schema.check_with(v, cx))
                } else {
                    check_ensure_absent(key, condition_key, condition_value, *ensure_absent, dct)
                }
            }
            Self::NoAdditionalKeys { keys } => {
                let extra = extra_keys(dct, keys);
                if extra.is_empty() {
                    return Ok(());
                }
                Err(ValidationError::new(format!(
                    "Additional keys found: {}.  Only these keys are allowed: {}",
                    join_str(extra)?,
                    join_str(keys)?,
                )))
            }
            Self::WarnAdditionalKeys { keys, callback } => {
                let extra: Vec<Value> = extra_keys(dct, keys).into_iter().cloned().collect();
                if !extra.is_empty() {
                    callback(&extra, keys, dct, cx);
                }
                Ok(())
            }
            Self::Custom(item) => item.check(dct, cx),
        }
    }

    /// Applies this item's defaults to an already validated map, in place.
    pub fn apply_default(&self, dct: &mut Map) {
        match self {
            Self::RequiredRecurse { key, schema } => recurse_in_place(dct, key, schema, true),
            Self::Optional { key, default, .. } => set_default(dct, key, default),
            Self::OptionalRecurse {
                key,
                schema,
                default,
            } => {
                set_default(dct, key, default);
                recurse_in_place(dct, key, schema, true);
            }
            Self::ConditionalOptional {
                key,
                default,
                condition_key,
                condition_value,
                ..
            } => {
                if condition_value.matches(dct.get(condition_key)) {
                    set_default(dct, key, default);
                }
            }
            Self::ConditionalRecurse {
                key,
                schema,
                condition_key,
                condition_value,
                ..
            } => {
                if condition_value.matches(dct.get(condition_key)) {
                    recurse_in_place(dct, key, schema, true);
                }
            }
            Self::Custom(item) => item.apply_default(dct),
            Self::Required { .. }
            | Self::OptionalNoDefault { .. }
            | Self::Conditional { .. }
            | Self::NoAdditionalKeys { .. }
            | Self::WarnAdditionalKeys { .. } => {}
        }
    }

    /// Removes this item's defaults from an already validated map, in place
    /// (the order of the remaining keys is preserved).
    pub fn remove_default(&self, dct: &mut Map) {
        match self {
            Self::RequiredRecurse { key, schema } => recurse_in_place(dct, key, schema, false),
            Self::Optional { key, default, .. } => remove_if_default(dct, key, default),
            Self::OptionalRecurse {
                key,
                schema,
                default,
            } => {
                if dct.contains_key(key) {
                    recurse_in_place(dct, key, schema, false);
                    remove_if_default(dct, key, default);
                }
            }
            Self::ConditionalOptional {
                key,
                default,
                condition_key,
                condition_value,
                ..
            } => {
                if condition_value.matches(dct.get(condition_key)) {
                    remove_if_default(dct, key, default);
                }
            }
            Self::ConditionalRecurse {
                key,
                schema,
                condition_key,
                condition_value,
                ..
            } => {
                if condition_value.matches(dct.get(condition_key)) {
                    recurse_in_place(dct, key, schema, false);
                }
            }
            Self::Custom(item) => item.remove_default(dct),
            Self::Required { .. }
            | Self::OptionalNoDefault { .. }
            | Self::Conditional { .. }
            | Self::NoAdditionalKeys { .. }
            | Self::WarnAdditionalKeys { .. } => {}
        }
    }

    /// The key this item validates (`item.key` in Python), if it has one.
    #[must_use]
    pub fn key(&self) -> Option<&Value> {
        match self {
            Self::Required { key, .. }
            | Self::RequiredRecurse { key, .. }
            | Self::Optional { key, .. }
            | Self::OptionalRecurse { key, .. }
            | Self::OptionalNoDefault { key, .. }
            | Self::Conditional { key, .. }
            | Self::ConditionalOptional { key, .. }
            | Self::ConditionalRecurse { key, .. } => Some(key),
            Self::Custom(item) => item.key(),
            Self::NoAdditionalKeys { .. } | Self::WarnAdditionalKeys { .. } => None,
        }
    }

    /// The check function applied to the key (`item.check_fn` in Python),
    /// if the item has one. `*Recurse` items expose their schema through
    /// [`Validator::schema`] instead.
    #[must_use]
    pub fn check_fn(&self) -> Option<&CheckFn> {
        match self {
            Self::Required { check_fn, .. }
            | Self::Optional { check_fn, .. }
            | Self::OptionalNoDefault { check_fn, .. }
            | Self::Conditional { check_fn, .. }
            | Self::ConditionalOptional { check_fn, .. } => Some(check_fn),
            Self::Custom(item) => item.check_fn(),
            Self::RequiredRecurse { .. }
            | Self::OptionalRecurse { .. }
            | Self::ConditionalRecurse { .. }
            | Self::NoAdditionalKeys { .. }
            | Self::WarnAdditionalKeys { .. } => None,
        }
    }

    /// The nested schema of a `*Recurse` item.
    #[must_use]
    pub fn schema(&self) -> Option<&Arc<Schema>> {
        match self {
            Self::RequiredRecurse { schema, .. }
            | Self::OptionalRecurse { schema, .. }
            | Self::ConditionalRecurse { schema, .. } => Some(schema),
            _ => None,
        }
    }

    /// The default value of an `Optional*`/`ConditionalOptional` item.
    #[must_use]
    pub const fn default_value(&self) -> Option<&Value> {
        match self {
            Self::Optional { default, .. }
            | Self::OptionalRecurse { default, .. }
            | Self::ConditionalOptional { default, .. } => Some(default),
            _ => None,
        }
    }

    // --- Constructors -------------------------------------------------------

    /// `Required(key, check_fn)`.
    #[must_use]
    pub fn required(key: impl Into<Value>, check_fn: CheckFn) -> Self {
        Self::Required {
            key: key.into(),
            check_fn,
        }
    }

    /// `RequiredRecurse(key, schema)`.
    #[must_use]
    pub fn required_recurse(key: impl Into<Value>, schema: impl Into<Arc<Schema>>) -> Self {
        Self::RequiredRecurse {
            key: key.into(),
            schema: schema.into(),
        }
    }

    /// `Optional(key, check_fn, default)`.
    #[must_use]
    pub fn optional(key: impl Into<Value>, check_fn: CheckFn, default: impl Into<Value>) -> Self {
        Self::Optional {
            key: key.into(),
            check_fn,
            default: default.into(),
        }
    }

    /// `OptionalRecurse(key, schema, default)`.
    #[must_use]
    pub fn optional_recurse(
        key: impl Into<Value>,
        schema: impl Into<Arc<Schema>>,
        default: impl Into<Value>,
    ) -> Self {
        Self::OptionalRecurse {
            key: key.into(),
            schema: schema.into(),
            default: default.into(),
        }
    }

    /// `OptionalNoDefault(key, check_fn)`.
    #[must_use]
    pub fn optional_no_default(key: impl Into<Value>, check_fn: CheckFn) -> Self {
        Self::OptionalNoDefault {
            key: key.into(),
            check_fn,
        }
    }

    /// `Conditional(key, check_fn, condition_key, condition_value)`.
    #[must_use]
    pub fn conditional(
        key: impl Into<Value>,
        check_fn: CheckFn,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
    ) -> Self {
        Self::conditional_with_absent(key, check_fn, condition_key, condition_value, false)
    }

    /// `Conditional(key, check_fn, condition_key, condition_value,
    /// ensure_absent)`.
    #[must_use]
    pub fn conditional_with_absent(
        key: impl Into<Value>,
        check_fn: CheckFn,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
        ensure_absent: bool,
    ) -> Self {
        Self::Conditional {
            key: key.into(),
            check_fn,
            condition_key: condition_key.into(),
            condition_value,
            ensure_absent,
        }
    }

    /// `ConditionalOptional(key, check_fn, default, condition_key,
    /// condition_value)`.
    #[must_use]
    pub fn conditional_optional(
        key: impl Into<Value>,
        check_fn: CheckFn,
        default: impl Into<Value>,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
    ) -> Self {
        Self::conditional_optional_with_absent(
            key,
            check_fn,
            default,
            condition_key,
            condition_value,
            false,
        )
    }

    /// `ConditionalOptional(key, check_fn, default, condition_key,
    /// condition_value, ensure_absent)`.
    #[must_use]
    pub fn conditional_optional_with_absent(
        key: impl Into<Value>,
        check_fn: CheckFn,
        default: impl Into<Value>,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
        ensure_absent: bool,
    ) -> Self {
        Self::ConditionalOptional {
            key: key.into(),
            check_fn,
            default: default.into(),
            condition_key: condition_key.into(),
            condition_value,
            ensure_absent,
        }
    }

    /// `ConditionalRecurse(key, schema, condition_key, condition_value)`.
    #[must_use]
    pub fn conditional_recurse(
        key: impl Into<Value>,
        schema: impl Into<Arc<Schema>>,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
    ) -> Self {
        Self::conditional_recurse_with_absent(key, schema, condition_key, condition_value, false)
    }

    /// `ConditionalRecurse(key, schema, condition_key, condition_value,
    /// ensure_absent)`.
    #[must_use]
    pub fn conditional_recurse_with_absent(
        key: impl Into<Value>,
        schema: impl Into<Arc<Schema>>,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
        ensure_absent: bool,
    ) -> Self {
        Self::ConditionalRecurse {
            key: key.into(),
            schema: schema.into(),
            condition_key: condition_key.into(),
            condition_value,
            ensure_absent,
        }
    }

    /// `NoAdditionalKeys(keys)`.
    #[must_use]
    pub const fn no_additional_keys(keys: Vec<Value>) -> Self {
        Self::NoAdditionalKeys { keys }
    }

    /// `WarnAdditionalKeys(keys, callback)`; the callback typically emits a
    /// warning through the [`Cx`] it receives.
    #[must_use]
    pub fn warn_additional_keys<F>(keys: Vec<Value>, callback: F) -> Self
    where
        F: Fn(&[Value], &[Value], &Map, &mut Cx<'_>) + Send + Sync + 'static,
    {
        Self::WarnAdditionalKeys {
            keys,
            callback: Arc::new(callback),
        }
    }

    /// A user-defined [`MapItem`].
    #[must_use]
    pub fn custom(item: impl MapItem + 'static) -> Self {
        Self::Custom(Arc::new(item))
    }
}

impl MapItem for Validator {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        Self::check(self, dct, cx)
    }

    fn apply_default(&self, dct: &mut Map) {
        Self::apply_default(self, dct);
    }

    fn remove_default(&self, dct: &mut Map) {
        Self::remove_default(self, dct);
    }

    fn key(&self) -> Option<&Value> {
        Self::key(self)
    }

    fn check_fn(&self) -> Option<&CheckFn> {
        Self::check_fn(self)
    }
}
