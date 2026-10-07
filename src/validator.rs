use std::sync::Arc;

use indexmap::IndexMap;

use crate::check::CheckFn;
use crate::condition::ConditionValue;
use crate::error::{ValidationError, validate_context};
use crate::schema::Schema;
use crate::value::Value;

/// Callback type for `WarnAdditionalKeys`.
pub type WarnCallback = Arc<dyn Fn(&[Value], &[Value], &IndexMap<Value, Value>) + Send + Sync>;

/// A validator for a key-value pair within a `Map` schema.
///
/// Each variant corresponds to a Python cfgv validator type
/// (Required, Optional, Conditional, etc.).
#[derive(Clone)]
pub enum Validator {
    Required {
        key: Value,
        check_fn: CheckFn,
    },
    RequiredRecurse {
        key: Value,
        schema: Schema,
    },
    Optional {
        key: Value,
        check_fn: CheckFn,
        default: Value,
    },
    OptionalRecurse {
        key: Value,
        schema: Schema,
        default: Value,
    },
    OptionalNoDefault {
        key: Value,
        check_fn: CheckFn,
    },
    Conditional {
        key: Value,
        check_fn: CheckFn,
        condition_key: Value,
        condition_value: ConditionValue,
        ensure_absent: bool,
    },
    ConditionalOptional {
        key: Value,
        check_fn: CheckFn,
        default: Value,
        condition_key: Value,
        condition_value: ConditionValue,
        ensure_absent: bool,
    },
    ConditionalRecurse {
        key: Value,
        schema: Schema,
        condition_key: Value,
        condition_value: ConditionValue,
        ensure_absent: bool,
    },
    NoAdditionalKeys {
        keys: Vec<Value>,
    },
    WarnAdditionalKeys {
        keys: Vec<Value>,
        callback: WarnCallback,
    },
}

impl std::fmt::Debug for Validator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Validator::Required { key, check_fn } => f
                .debug_struct("Required")
                .field("key", key)
                .field("check_fn", check_fn)
                .finish(),
            Validator::RequiredRecurse { key, schema } => f
                .debug_struct("RequiredRecurse")
                .field("key", key)
                .field("schema", schema)
                .finish(),
            Validator::Optional {
                key,
                check_fn,
                default,
            } => f
                .debug_struct("Optional")
                .field("key", key)
                .field("check_fn", check_fn)
                .field("default", default)
                .finish(),
            Validator::OptionalRecurse {
                key,
                schema,
                default,
            } => f
                .debug_struct("OptionalRecurse")
                .field("key", key)
                .field("schema", schema)
                .field("default", default)
                .finish(),
            Validator::OptionalNoDefault { key, check_fn } => f
                .debug_struct("OptionalNoDefault")
                .field("key", key)
                .field("check_fn", check_fn)
                .finish(),
            Validator::Conditional {
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
            Validator::ConditionalOptional {
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
            Validator::ConditionalRecurse {
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
            Validator::NoAdditionalKeys { keys } => f
                .debug_struct("NoAdditionalKeys")
                .field("keys", keys)
                .finish(),
            Validator::WarnAdditionalKeys { keys, .. } => f
                .debug_struct("WarnAdditionalKeys")
                .field("keys", keys)
                .finish(),
        }
    }
}

// --- Check logic ---

fn require_key(key: &Value, dct: &IndexMap<Value, Value>) -> Result<(), ValidationError> {
    if !dct.contains_key(key) {
        return Err(ValidationError::new(format!("Missing required key: {key}")));
    }
    Ok(())
}

fn check_optional(
    key: &Value,
    check_fn: &CheckFn,
    dct: &IndexMap<Value, Value>,
) -> Result<(), ValidationError> {
    if let Some(val) = dct.get(key) {
        validate_context(|| format!("At key: {key}"), || check_fn.check(val))?;
    }
    Ok(())
}

fn check_required(
    key: &Value,
    check_fn: &CheckFn,
    dct: &IndexMap<Value, Value>,
) -> Result<(), ValidationError> {
    require_key(key, dct)?;
    check_optional(key, check_fn, dct)
}

fn check_ensure_absent(
    key: &Value,
    condition_key: &Value,
    condition_value: &ConditionValue,
    ensure_absent: bool,
    dct: &IndexMap<Value, Value>,
) -> Result<(), ValidationError> {
    if ensure_absent && dct.contains_key(condition_key) && dct.contains_key(key) {
        let explanation = condition_value.describe_opposite();
        let found_val = dct[key].repr();
        return Err(ValidationError::new(format!(
            "Expected {key} to be absent when {condition_key} {explanation}, \
             found {key}: {found_val}"
        )));
    }
    Ok(())
}

impl Validator {
    /// Validate the given map against this validator.
    pub fn check(&self, dct: &IndexMap<Value, Value>) -> Result<(), ValidationError> {
        match self {
            Validator::Required { key, check_fn } => check_required(key, check_fn, dct),

            Validator::RequiredRecurse { key, schema } => {
                require_key(key, dct)?;
                if let Some(val) = dct.get(key) {
                    validate_context(|| format!("At key: {key}"), || schema.check(val))?;
                }
                Ok(())
            }

            Validator::Optional { key, check_fn, .. } => check_optional(key, check_fn, dct),

            Validator::OptionalRecurse { key, schema, .. } => {
                if let Some(val) = dct.get(key) {
                    validate_context(|| format!("At key: {key}"), || schema.check(val))?;
                }
                Ok(())
            }

            Validator::OptionalNoDefault { key, check_fn } => check_optional(key, check_fn, dct),

            Validator::Conditional {
                key,
                check_fn,
                condition_key,
                condition_value,
                ensure_absent,
            } => {
                let cond_val = dct.get(condition_key);
                if condition_value.matches(cond_val) {
                    check_required(key, check_fn, dct)?;
                } else {
                    check_ensure_absent(key, condition_key, condition_value, *ensure_absent, dct)?;
                }
                Ok(())
            }

            Validator::ConditionalOptional {
                key,
                check_fn,
                condition_key,
                condition_value,
                ensure_absent,
                ..
            } => {
                let cond_val = dct.get(condition_key);
                if condition_value.matches(cond_val) {
                    check_optional(key, check_fn, dct)?;
                } else {
                    check_ensure_absent(key, condition_key, condition_value, *ensure_absent, dct)?;
                }
                Ok(())
            }

            Validator::ConditionalRecurse {
                key,
                schema,
                condition_key,
                condition_value,
                ensure_absent,
            } => {
                let cond_val = dct.get(condition_key);
                if condition_value.matches(cond_val) {
                    require_key(key, dct)?;
                    if let Some(val) = dct.get(key) {
                        validate_context(|| format!("At key: {key}"), || schema.check(val))?;
                    }
                } else {
                    check_ensure_absent(key, condition_key, condition_value, *ensure_absent, dct)?;
                }
                Ok(())
            }

            Validator::NoAdditionalKeys { keys } => {
                let key_set: std::collections::HashSet<&Value> = keys.iter().collect();
                let mut extra: Vec<&Value> = dct.keys().filter(|k| !key_set.contains(k)).collect();
                crate::value::py_sort(&mut extra);
                if !extra.is_empty() {
                    let extra_s: Vec<String> = extra.iter().map(|v| format!("{v}")).collect();
                    let keys_s: Vec<String> = keys.iter().map(|v| format!("{v}")).collect();
                    return Err(ValidationError::new(format!(
                        "Additional keys found: {}.  Only these keys are allowed: {}",
                        extra_s.join(", "),
                        keys_s.join(", ")
                    )));
                }
                Ok(())
            }

            Validator::WarnAdditionalKeys { keys, callback } => {
                let key_set: std::collections::HashSet<&Value> = keys.iter().collect();
                let mut extra: Vec<Value> = dct
                    .keys()
                    .filter(|k| !key_set.contains(k))
                    .cloned()
                    .collect();
                crate::value::py_sort(&mut extra);
                if !extra.is_empty() {
                    callback(&extra, keys, dct);
                }
                Ok(())
            }
        }
    }

    /// Apply defaults for this validator to the given map (mutates in place).
    pub fn apply_default(&self, dct: &mut IndexMap<Value, Value>) {
        match self {
            Validator::Required { .. }
            | Validator::OptionalNoDefault { .. }
            | Validator::NoAdditionalKeys { .. }
            | Validator::WarnAdditionalKeys { .. } => {}

            Validator::Conditional { .. } => {}

            Validator::RequiredRecurse { key, schema } => {
                if let Some(val) = dct.get(key).cloned() {
                    dct.insert(key.clone(), schema.apply_defaults(&val));
                }
            }

            Validator::Optional { key, default, .. } => {
                if !dct.contains_key(key) {
                    dct.insert(key.clone(), default.clone());
                }
            }

            Validator::OptionalRecurse {
                key,
                schema,
                default,
            } => {
                if !dct.contains_key(key) {
                    dct.insert(key.clone(), default.clone());
                }
                if let Some(val) = dct.get(key).cloned() {
                    dct.insert(key.clone(), schema.apply_defaults(&val));
                }
            }

            Validator::ConditionalOptional {
                key,
                default,
                condition_key,
                condition_value,
                ..
            } => {
                let cond_val = dct.get(condition_key).cloned();
                if condition_value.matches(cond_val.as_ref()) && !dct.contains_key(key) {
                    dct.insert(key.clone(), default.clone());
                }
            }

            Validator::ConditionalRecurse {
                key,
                schema,
                condition_key,
                condition_value,
                ..
            } => {
                let cond_val = dct.get(condition_key).cloned();
                if condition_value.matches(cond_val.as_ref())
                    && let Some(val) = dct.get(key).cloned()
                {
                    dct.insert(key.clone(), schema.apply_defaults(&val));
                }
            }
        }
    }

    /// Remove defaults for this validator from the given map (mutates in place).
    pub fn remove_default(&self, dct: &mut IndexMap<Value, Value>) {
        match self {
            Validator::Required { .. }
            | Validator::OptionalNoDefault { .. }
            | Validator::NoAdditionalKeys { .. }
            | Validator::WarnAdditionalKeys { .. } => {}

            Validator::Conditional { .. } => {}

            Validator::RequiredRecurse { key, schema } => {
                if let Some(val) = dct.get(key).cloned() {
                    dct.insert(key.clone(), schema.remove_defaults(&val));
                }
            }

            Validator::Optional { key, default, .. } => {
                if dct.get(key).is_some_and(|v| v == default) {
                    dct.swap_remove(key);
                }
            }

            Validator::OptionalRecurse {
                key,
                schema,
                default,
            } => {
                if dct.contains_key(key) {
                    if let Some(val) = dct.get(key).cloned() {
                        dct.insert(key.clone(), schema.remove_defaults(&val));
                    }
                    if dct.get(key).is_some_and(|v| v == default) {
                        dct.swap_remove(key);
                    }
                }
            }

            Validator::ConditionalOptional {
                key,
                default,
                condition_key,
                condition_value,
                ..
            } => {
                let cond_val = dct.get(condition_key).cloned();
                if condition_value.matches(cond_val.as_ref())
                    && dct.get(key).is_some_and(|v| v == default)
                {
                    dct.swap_remove(key);
                }
            }

            Validator::ConditionalRecurse {
                key,
                schema,
                condition_key,
                condition_value,
                ..
            } => {
                let cond_val = dct.get(condition_key).cloned();
                if condition_value.matches(cond_val.as_ref())
                    && let Some(val) = dct.get(key).cloned()
                {
                    dct.insert(key.clone(), schema.remove_defaults(&val));
                }
            }
        }
    }

    // --- Convenience constructors ---

    pub fn required(key: impl Into<Value>, check_fn: CheckFn) -> Self {
        Validator::Required {
            key: key.into(),
            check_fn,
        }
    }

    pub fn required_recurse(key: impl Into<Value>, schema: Schema) -> Self {
        Validator::RequiredRecurse {
            key: key.into(),
            schema,
        }
    }

    pub fn optional(key: impl Into<Value>, check_fn: CheckFn, default: impl Into<Value>) -> Self {
        Validator::Optional {
            key: key.into(),
            check_fn,
            default: default.into(),
        }
    }

    pub fn optional_recurse(
        key: impl Into<Value>,
        schema: Schema,
        default: impl Into<Value>,
    ) -> Self {
        Validator::OptionalRecurse {
            key: key.into(),
            schema,
            default: default.into(),
        }
    }

    pub fn optional_no_default(key: impl Into<Value>, check_fn: CheckFn) -> Self {
        Validator::OptionalNoDefault {
            key: key.into(),
            check_fn,
        }
    }

    pub fn conditional(
        key: impl Into<Value>,
        check_fn: CheckFn,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
    ) -> Self {
        Validator::Conditional {
            key: key.into(),
            check_fn,
            condition_key: condition_key.into(),
            condition_value,
            ensure_absent: false,
        }
    }

    pub fn conditional_with_absent(
        key: impl Into<Value>,
        check_fn: CheckFn,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
        ensure_absent: bool,
    ) -> Self {
        Validator::Conditional {
            key: key.into(),
            check_fn,
            condition_key: condition_key.into(),
            condition_value,
            ensure_absent,
        }
    }

    pub fn conditional_optional(
        key: impl Into<Value>,
        check_fn: CheckFn,
        default: impl Into<Value>,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
    ) -> Self {
        Validator::ConditionalOptional {
            key: key.into(),
            check_fn,
            default: default.into(),
            condition_key: condition_key.into(),
            condition_value,
            ensure_absent: false,
        }
    }

    pub fn conditional_optional_with_absent(
        key: impl Into<Value>,
        check_fn: CheckFn,
        default: impl Into<Value>,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
        ensure_absent: bool,
    ) -> Self {
        Validator::ConditionalOptional {
            key: key.into(),
            check_fn,
            default: default.into(),
            condition_key: condition_key.into(),
            condition_value,
            ensure_absent,
        }
    }

    pub fn conditional_recurse(
        key: impl Into<Value>,
        schema: Schema,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
    ) -> Self {
        Validator::ConditionalRecurse {
            key: key.into(),
            schema,
            condition_key: condition_key.into(),
            condition_value,
            ensure_absent: false,
        }
    }

    pub fn conditional_recurse_with_absent(
        key: impl Into<Value>,
        schema: Schema,
        condition_key: impl Into<Value>,
        condition_value: ConditionValue,
        ensure_absent: bool,
    ) -> Self {
        Validator::ConditionalRecurse {
            key: key.into(),
            schema,
            condition_key: condition_key.into(),
            condition_value,
            ensure_absent,
        }
    }

    pub fn no_additional_keys(keys: Vec<Value>) -> Self {
        Validator::NoAdditionalKeys { keys }
    }

    pub fn warn_additional_keys<F>(keys: Vec<Value>, callback: F) -> Self
    where
        F: Fn(&[Value], &[Value], &IndexMap<Value, Value>) + Send + Sync + 'static,
    {
        Validator::WarnAdditionalKeys {
            keys,
            callback: Arc::new(callback),
        }
    }
}
