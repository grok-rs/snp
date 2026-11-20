//! Container schemas: `Map`, `Array` and `KeyValueMap`.

use std::sync::Arc;

use crate::check::CheckFn;
use crate::error::{ValidationError, validate_context};
use crate::py;
use crate::validator::{Cx, Validator};
use crate::value::{HashedKey, Value};

/// A schema for a container value, mirroring Python cfgv's `Map`, `Array`
/// and `KeyValueMap`.
///
/// Schemas are immutable and `Send + Sync`; build them once (e.g. in a
/// `static LazyLock<Schema>`) and share them. Nested schemas and item lists
/// are reference-counted, so cloning a schema is cheap.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Schema {
    /// `Map(object_name, id_key, *items)`: a dict validated item by item.
    Map {
        /// Name used in messages (`At {object_name}(...)`).
        object_name: String,
        /// Key whose value identifies the map in the context line, with its
        /// hash precomputed.
        id_key: Option<HashedKey>,
        /// The items, checked in order.
        items: Arc<[Validator]>,
    },
    /// `Array(of, allow_empty)`: a list or tuple whose items all match `of`.
    Array {
        /// Schema of every item.
        of: Arc<Self>,
        /// Whether an empty array is valid.
        allow_empty: bool,
    },
    /// `KeyValueMap(object_name, check_key_fn, value_schema)`: a dict with
    /// arbitrary keys that pass `check_key_fn` and values matching
    /// `value_schema`.
    KeyValueMap {
        /// Name used in messages.
        object_name: String,
        /// Check applied to every key.
        check_key_fn: CheckFn,
        /// Schema of every value.
        value_schema: Arc<Self>,
    },
}

impl Schema {
    /// Validates `value`, discarding warnings. See [`Schema::check_with`].
    ///
    /// # Errors
    ///
    /// Returns the first validation error.
    pub fn check(&self, value: &Value) -> Result<(), ValidationError> {
        self.check_with(value, &mut Cx::discard())
    }

    /// Validates `value`, emitting warnings through `cx`.
    ///
    /// # Errors
    ///
    /// Returns the first validation error, wrapped in its contexts.
    pub fn check_with(&self, value: &Value, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        match self {
            Self::Map {
                object_name,
                id_key,
                items,
            } => {
                let Value::Map(dct) = value else {
                    return Err(not_a_map(object_name, value));
                };
                // Python formats the context before checking the items, so
                // an id that `repr()` cannot format fails even a valid map.
                if let Some(id_key) = id_key {
                    id_key.check_str_digits()?;
                    if let Some(id) = id_key.get(dct) {
                        id.check_str_digits()?;
                    }
                }
                let context = || match id_key {
                    None => format!("At {object_name}()"),
                    Some(id_key) => {
                        let id = id_key
                            .get(dct)
                            .map_or_else(|| "MISSING".to_owned(), Value::repr);
                        format!("At {object_name}({id_key}={id})")
                    }
                };
                validate_context(context, || {
                    items.iter().try_for_each(|item| item.check(dct, cx))
                })
            }
            Self::Array { of, allow_empty } => {
                let items = value.as_array().ok_or_else(|| {
                    ValidationError::new(format!(
                        "Expected array but got {}",
                        py::repr_str(value.type_name())
                    ))
                })?;
                if !allow_empty && items.is_empty() {
                    let name = of.object_name().unwrap_or("item");
                    return Err(ValidationError::new(format!(
                        "Expected at least 1 '{name}'"
                    )));
                }
                items.iter().try_for_each(|item| of.check_with(item, cx))
            }
            Self::KeyValueMap {
                object_name,
                check_key_fn,
                value_schema,
            } => {
                let Value::Map(dct) = value else {
                    return Err(not_a_map(object_name, value));
                };
                validate_context(
                    || format!("At {object_name}()"),
                    || {
                        for (k, v) in dct {
                            // Python formats each context before using it.
                            k.check_str_digits()?;
                            validate_context(|| format!("For key: {k}"), || check_key_fn.check(k))?;
                            validate_context(
                                || format!("At key: {k}"),
                                || value_schema.check_with(v, cx),
                            )?;
                        }
                        Ok(())
                    },
                )
            }
        }
    }

    /// Returns a copy of `value` with defaults applied (Python
    /// `apply_defaults`). Call it on validated values only: data that does
    /// not have the shape of the schema is returned unchanged.
    #[must_use]
    pub fn apply_defaults(&self, value: &Value) -> Value {
        self.apply_defaults_owned(value.clone())
    }

    /// Applies defaults to `value`, consuming it and reusing its
    /// allocations.
    #[must_use]
    pub fn apply_defaults_owned(&self, mut value: Value) -> Value {
        self.apply_defaults_in_place(&mut value);
        value
    }

    /// Returns a copy of `value` with every optional value that equals its
    /// default removed (Python `remove_defaults`).
    #[must_use]
    pub fn remove_defaults(&self, value: &Value) -> Value {
        self.remove_defaults_owned(value.clone())
    }

    /// Removes defaults from `value`, consuming it and reusing its
    /// allocations.
    #[must_use]
    pub fn remove_defaults_owned(&self, mut value: Value) -> Value {
        self.remove_defaults_in_place(&mut value);
        value
    }

    pub(crate) fn apply_defaults_in_place(&self, value: &mut Value) {
        self.transform_in_place(value, true);
    }

    pub(crate) fn remove_defaults_in_place(&self, value: &mut Value) {
        self.transform_in_place(value, false);
    }

    fn transform_in_place(&self, value: &mut Value, apply: bool) {
        match (self, value) {
            (Self::Map { items, .. }, Value::Map(dct)) => {
                for item in items.iter() {
                    if apply {
                        item.apply_default(dct);
                    } else {
                        item.remove_default(dct);
                    }
                }
            }
            (Self::Array { of, .. }, value @ (Value::List(_) | Value::Tuple(_))) => {
                // Python returns a list for both lists and tuples.
                if let Value::Tuple(items) = value {
                    *value = Value::List(std::mem::take(items));
                }
                if let Value::List(items) = value {
                    for item in items {
                        of.transform_in_place(item, apply);
                    }
                }
            }
            (Self::KeyValueMap { value_schema, .. }, Value::Map(dct)) => {
                for v in dct.values_mut() {
                    value_schema.transform_in_place(v, apply);
                }
            }
            // Never replace data that does not match the schema.
            _ => {}
        }
    }

    /// The `object_name` of a `Map` or `KeyValueMap` schema.
    #[must_use]
    pub fn object_name(&self) -> Option<&str> {
        match self {
            Self::Map { object_name, .. } | Self::KeyValueMap { object_name, .. } => {
                Some(object_name)
            }
            Self::Array { .. } => None,
        }
    }

    /// The items of a `Map` schema, e.g. to derive another schema from them
    /// like pre-commit derives its config hook schema from the manifest
    /// schema.
    #[must_use]
    pub fn items(&self) -> Option<&Arc<[Validator]>> {
        match self {
            Self::Map { items, .. } => Some(items),
            _ => None,
        }
    }

    // --- Constructors -------------------------------------------------------

    /// `Map(object_name, id_key, *items)`.
    ///
    /// ```
    /// use cfgv::{CheckFn, Schema, Validator, validate, value_map};
    ///
    /// let schema = Schema::map("Repository", Some("repo".into()), vec![
    ///     Validator::required("repo", CheckFn::check_string()),
    ///     Validator::required("rev", CheckFn::check_string()),
    /// ]);
    /// let err = validate(&value_map! {"repo" => "it's"}, &schema).unwrap_err();
    /// assert_eq!(
    ///     err.to_string(),
    ///     "\n==> At Repository(repo=\"it's\")\n=====> Missing required key: rev",
    /// );
    /// ```
    #[must_use]
    pub fn map(
        object_name: impl Into<String>,
        id_key: Option<Value>,
        items: impl Into<Arc<[Validator]>>,
    ) -> Self {
        Self::Map {
            object_name: object_name.into(),
            id_key: id_key.map(HashedKey::new),
            items: items.into(),
        }
    }

    /// `Array(of)`: empty arrays are allowed.
    #[must_use]
    pub fn array(of: impl Into<Arc<Self>>) -> Self {
        Self::Array {
            of: of.into(),
            allow_empty: true,
        }
    }

    /// `Array(of, allow_empty=False)`.
    #[must_use]
    pub fn array_nonempty(of: impl Into<Arc<Self>>) -> Self {
        Self::Array {
            of: of.into(),
            allow_empty: false,
        }
    }

    /// `KeyValueMap(object_name, check_key_fn, value_schema)`.
    #[must_use]
    pub fn key_value_map(
        object_name: impl Into<String>,
        check_key_fn: CheckFn,
        value_schema: impl Into<Arc<Self>>,
    ) -> Self {
        Self::KeyValueMap {
            object_name: object_name.into(),
            check_key_fn,
            value_schema: value_schema.into(),
        }
    }
}

/// `Expected a {object_name} map but got a {type}`.
fn not_a_map(object_name: &str, value: &Value) -> ValidationError {
    ValidationError::new(format!(
        "Expected a {object_name} map but got a {}",
        value.type_name()
    ))
}
