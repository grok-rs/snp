//! Check functions for single values (`check_bool`, `check_one_of`, ...).

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use crate::error::{ValidationError, validate_context};
use crate::py;
use crate::value::{Value, py_sort};

/// A user-provided check function, see [`CheckFn::custom`].
pub type CustomCheckFn = Arc<dyn Fn(&Value) -> Result<(), ValidationError> + Send + Sync>;

/// Decides whether a pattern is a valid regular expression, see
/// [`CheckFn::check_regex_try_with`].
///
/// `Ok(true)` accepts the pattern, `Ok(false)` rejects it like Python's
/// `re.error` (`{pattern!r} is not a valid python regex`), and `Err(e)`
/// aborts validation with `e`, typically an
/// [uncaught](ValidationError::uncaught) exception such as the
/// `OverflowError` or `RecursionError` that `re.compile` raises for some
/// patterns.
pub type RegexCheck = Arc<dyn Fn(RegexPattern<'_>) -> Result<bool, ValidationError> + Send + Sync>;

/// A pattern handed to a [`RegexCheck`]: Python's `re.compile` accepts
/// both `str` and `bytes` patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RegexPattern<'a> {
    /// A `str` pattern.
    Str(&'a str),
    /// A `bytes` pattern.
    Bytes(&'a [u8]),
}

impl<'a> RegexPattern<'a> {
    /// The pattern as text: a `str` pattern unchanged, a `bytes` pattern
    /// decoded as Latin-1 (every byte becomes the code point of the same
    /// value), which is how Python's `re` parser reads bytes patterns.
    ///
    /// ```
    /// use cfgv::RegexPattern;
    ///
    /// assert_eq!(RegexPattern::Bytes(b"a\xff(").to_text(), "a\u{ff}(");
    /// ```
    #[must_use]
    pub fn to_text(self) -> Cow<'a, str> {
        match self {
            Self::Str(s) => Cow::Borrowed(s),
            Self::Bytes(b) => Cow::Owned(b.iter().copied().map(char::from).collect()),
        }
    }
}

/// A Python type for [`CheckFn::Type`], matched with `isinstance`
/// semantics.
///
/// | Variant | Python type | Matches |
/// |---|---|---|
/// | `Bool` | `bool` | `Bool` |
/// | `Int` | `int` | `Int`, `BigInt` and `Bool` (`bool` subclasses `int`) |
/// | `Float` | `float` | `Float` |
/// | `String` | `str` | `String` |
/// | `Bytes` | `bytes` | `Bytes` |
/// | `List` | `list` | `List` |
/// | `Tuple` | `tuple` | `Tuple` |
/// | `Map` | `dict` | `Map` |
/// | `Set` | `set` | `Set` |
/// | `Null` | `NoneType` | `Null` |
/// | `Date` | `date` | `Date` and `DateTime` (`datetime` subclasses `date`) |
/// | `DateTime` | `datetime` | `DateTime` |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ValueType {
    /// `bool`.
    Bool,
    /// `int` (including `bool`).
    Int,
    /// `float`.
    Float,
    /// `str`.
    String,
    /// `bytes`.
    Bytes,
    /// `list`.
    List,
    /// `tuple`.
    Tuple,
    /// `dict`.
    Map,
    /// `set`.
    Set,
    /// `NoneType`.
    Null,
    /// `datetime.date` (including `datetime.datetime`).
    Date,
    /// `datetime.datetime`.
    DateTime,
}

impl ValueType {
    /// The Python type name (`tp.__name__`), used in `Expected {name} got
    /// {type}` messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::String => "str",
            Self::Bytes => "bytes",
            Self::List => "list",
            Self::Tuple => "tuple",
            Self::Map => "dict",
            Self::Set => "set",
            Self::Null => "NoneType",
            Self::Date => "date",
            Self::DateTime => "datetime",
        }
    }

    /// Whether `value` is an instance of this type (Python `isinstance`).
    #[must_use]
    pub const fn matches(self, value: &Value) -> bool {
        matches!(
            (self, value),
            (Self::Bool, Value::Bool(_))
                | (Self::Int, Value::Int(_) | Value::BigInt(_) | Value::Bool(_))
                | (Self::Float, Value::Float(_))
                | (Self::String, Value::String(_))
                | (Self::Bytes, Value::Bytes(_))
                | (Self::List, Value::List(_))
                | (Self::Tuple, Value::Tuple(_))
                | (Self::Map, Value::Map(_))
                | (Self::Set, Value::Set(_))
                | (Self::Null, Value::Null)
                | (Self::Date, Value::Date(_) | Value::DateTime(_))
                | (Self::DateTime, Value::DateTime(_))
        )
    }
}

/// A check function that validates a single value, mirroring Python cfgv's
/// `check_*` functions.
///
/// Build check functions with the constructors (`CheckFn::check_string()`,
/// `CheckFn::check_one_of(...)`, ...) and run them with [`CheckFn::check`].
///
/// ```
/// use cfgv::{CheckFn, Value};
///
/// let check = CheckFn::check_array(CheckFn::check_one_of(["pre-commit", "pre-push"]));
/// let err = check.check(&Value::from(vec![Value::from("commit")])).unwrap_err();
/// assert_eq!(
///     err.to_string(),
///     "\n==> At index 0\n=====> Expected one of pre-commit, pre-push but got: 'commit'",
/// );
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub enum CheckFn {
    /// Accepts any value (`check_any`).
    Any,
    /// `check_type(tp, typename)`: the value must be an instance of the type.
    /// The optional name replaces the type name in the error message
    /// (`check_string` uses `"string"`).
    Type(ValueType, Option<&'static str>),
    /// `check_one_of(possible)`: the value must equal (Python `==`) one of
    /// the possibilities.
    OneOf(Vec<Value>),
    /// `check_regex`: the value must be a `str` or `bytes` pattern that the
    /// checker accepts.
    Regex(RegexCheck),
    /// `check_array(inner)`: the value must be a list or tuple whose items
    /// all pass the inner check.
    Array(Box<CheckFn>),
    /// `check_and(*fns)`: runs every check in order; the first failure wins.
    And(Vec<CheckFn>),
    /// A user-provided check.
    Custom(CustomCheckFn),
}

impl fmt::Debug for CheckFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => f.write_str("Any"),
            Self::Type(t, name) => f.debug_tuple("Type").field(t).field(name).finish(),
            Self::OneOf(values) => f.debug_tuple("OneOf").field(values).finish(),
            Self::Regex(_) => f.write_str("Regex(..)"),
            Self::Array(inner) => f.debug_tuple("Array").field(inner).finish(),
            Self::And(fns) => f.debug_tuple("And").field(fns).finish(),
            Self::Custom(_) => f.write_str("Custom(..)"),
        }
    }
}

impl CheckFn {
    /// Validates `value` against this check function.
    ///
    /// # Errors
    ///
    /// Returns the cfgv error describing the first failure:
    ///
    /// * `Type`: `Expected {typename} got {type(v).__name__}`
    /// * `OneOf`: `Expected one of {sorted choices} but got: {v!r}`
    /// * `Regex`: `{v!r} is not a valid python regex` for a `str` or `bytes`
    ///   pattern the checker rejects; the checker's own error; or, for any
    ///   other value, the [uncaught](ValidationError::uncaught) `TypeError`
    ///   of `re.compile` (`first argument must be string or compiled
    ///   pattern`, or `unhashable type: 'list'` for unhashable values)
    /// * `Array`: `Expected array but got {type(v).__name__!r}`, or the
    ///   inner error inside an `At index {i}` context
    ///
    /// Where Python's message would contain `repr()` of an integer with more
    /// than 4300 digits, the error is CPython's uncaught `ValueError`.
    pub fn check(&self, value: &Value) -> Result<(), ValidationError> {
        match self {
            Self::Any => Ok(()),
            Self::Type(tp, typename) => {
                if tp.matches(value) {
                    Ok(())
                } else {
                    let expected = typename.unwrap_or_else(|| tp.name());
                    Err(ValidationError::new(format!(
                        "Expected {expected} got {}",
                        value.type_name()
                    )))
                }
            }
            Self::OneOf(possible) => {
                if possible.contains(value) {
                    return Ok(());
                }
                let mut sorted: Vec<&Value> = possible.iter().collect();
                py_sort(&mut sorted);
                let choices = join_str(sorted)?;
                Err(ValidationError::new(format!(
                    "Expected one of {choices} but got: {}",
                    value.try_repr()?
                )))
            }
            Self::Regex(checker) => {
                let pattern = match value {
                    Value::String(s) => RegexPattern::Str(s),
                    Value::Bytes(b) => RegexPattern::Bytes(b),
                    _ => return Err(regex_type_error(value)),
                };
                if checker(pattern)? {
                    Ok(())
                } else {
                    Err(ValidationError::new(format!(
                        "{} is not a valid python regex",
                        value.repr()
                    )))
                }
            }
            Self::Array(inner) => {
                let items = value.as_array().ok_or_else(|| {
                    ValidationError::new(format!(
                        "Expected array but got {}",
                        py::repr_str(value.type_name())
                    ))
                })?;
                for (i, item) in items.iter().enumerate() {
                    validate_context(|| format!("At index {i}"), || inner.check(item))?;
                }
                Ok(())
            }
            Self::And(fns) => fns.iter().try_for_each(|f| f.check(value)),
            Self::Custom(f) => f(value),
        }
    }

    /// `check_any`: accepts every value.
    #[must_use]
    pub const fn check_any() -> Self {
        Self::Any
    }

    /// `check_bool`: `Expected bool got {type}`.
    #[must_use]
    pub const fn check_bool() -> Self {
        Self::Type(ValueType::Bool, None)
    }

    /// `check_int`: `Expected int got {type}`. Like Python, booleans are
    /// accepted (`bool` is a subclass of `int`).
    #[must_use]
    pub const fn check_int() -> Self {
        Self::Type(ValueType::Int, None)
    }

    /// `check_string`: `Expected string got {type}`.
    #[must_use]
    pub const fn check_string() -> Self {
        Self::Type(ValueType::String, Some("string"))
    }

    /// `check_text`: `Expected text got {type}`.
    #[must_use]
    pub const fn check_text() -> Self {
        Self::Type(ValueType::String, Some("text"))
    }

    /// `check_bytes`: `Expected bytes got {type}`.
    #[must_use]
    pub const fn check_bytes() -> Self {
        Self::Type(ValueType::Bytes, None)
    }

    /// `check_type(tp, typename)`; e.g. `check_type(ValueType::Map, None)`
    /// is Python's `check_type(dict)` (`Expected dict got int`).
    #[must_use]
    pub const fn check_type(tp: ValueType, typename: Option<&'static str>) -> Self {
        Self::Type(tp, typename)
    }

    /// `check_one_of(possible)`.
    #[must_use]
    pub fn check_one_of<I, V>(possible: I) -> Self
    where
        I: IntoIterator<Item = V>,
        V: Into<Value>,
    {
        Self::OneOf(possible.into_iter().map(Into::into).collect())
    }

    /// `check_regex` backed by the [`regex`](https://docs.rs/regex) crate.
    ///
    /// The `regex` crate's syntax differs from Python's `re` (no
    /// look-around, no back-references, no `\Z`, ...), so this rejects some
    /// patterns Python accepts and vice versa. Use
    /// [`CheckFn::check_regex_with`] or [`CheckFn::check_regex_try_with`]
    /// to plug in a Python-compatible validator. `bytes` patterns are
    /// decoded as Latin-1 ([`RegexPattern::to_text`]) and checked with
    /// `regex::bytes`.
    #[cfg(feature = "regex")]
    #[cfg_attr(docsrs, doc(cfg(feature = "regex")))]
    #[must_use]
    pub fn check_regex() -> Self {
        Self::check_regex_try_with(|pattern| {
            Ok(match pattern {
                RegexPattern::Bytes(_) => regex::bytes::Regex::new(&pattern.to_text()).is_ok(),
                _ => regex::Regex::new(&pattern.to_text()).is_ok(),
            })
        })
    }

    /// `check_regex` with a caller-supplied validity test, e.g. a validator
    /// for Python's `re` dialect.
    ///
    /// `bytes` patterns are passed decoded as Latin-1
    /// ([`RegexPattern::to_text`]), the text Python's `re` parser sees; the
    /// few rules specific to bytes patterns (no `\u` escapes, no `(?u)`,
    /// `(?L)` allowed) need [`CheckFn::check_regex_try_with`].
    ///
    /// ```
    /// use cfgv::{CheckFn, Value};
    ///
    /// let check = CheckFn::check_regex_with(|pattern| !pattern.ends_with('\\'));
    /// assert!(check.check(&Value::from(r"a\Z")).is_ok());
    /// assert_eq!(
    ///     check.check(&Value::from("foo\\")).unwrap_err().message(),
    ///     r"'foo\\' is not a valid python regex",
    /// );
    /// assert_eq!(
    ///     check.check(&Value::from(&b"foo\\"[..])).unwrap_err().message(),
    ///     r"b'foo\\' is not a valid python regex",
    /// );
    /// ```
    #[must_use]
    pub fn check_regex_with<F>(is_valid: F) -> Self
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        Self::check_regex_try_with(move |pattern| Ok(is_valid(&pattern.to_text())))
    }

    /// `check_regex` with a caller-supplied checker that sees whether the
    /// pattern is `str` or `bytes` and can abort validation with an error
    /// other than "not a valid python regex" (see [`RegexCheck`]).
    ///
    /// This reproduces `re.compile` raising something other than
    /// `re.error`, which Python cfgv does not catch:
    ///
    /// ```
    /// use cfgv::{CheckFn, RegexPattern, ValidationError, Value};
    ///
    /// let check = CheckFn::check_regex_try_with(|pattern| match pattern {
    ///     RegexPattern::Str(p) if p.contains("{4294967296}") => Err(ValidationError::uncaught(
    ///         "OverflowError",
    ///         "the repetition number is too large",
    ///     )),
    ///     RegexPattern::Str(p) => Ok(!p.contains('(')),
    ///     _ => Ok(false),
    /// });
    /// let err = check.check(&Value::from("a{4294967296}")).unwrap_err();
    /// assert_eq!(err.to_string(), "OverflowError: the repetition number is too large");
    /// assert_eq!(
    ///     check.check(&Value::from("(")).unwrap_err().to_string(),
    ///     "\n=====> '(' is not a valid python regex",
    /// );
    /// ```
    #[must_use]
    pub fn check_regex_try_with<F>(checker: F) -> Self
    where
        F: Fn(RegexPattern<'_>) -> Result<bool, ValidationError> + Send + Sync + 'static,
    {
        Self::Regex(Arc::new(checker))
    }

    /// `check_array(inner)`.
    #[must_use]
    pub fn check_array(inner: Self) -> Self {
        Self::Array(Box::new(inner))
    }

    /// `check_and(*fns)`.
    #[must_use]
    pub fn check_and<I: IntoIterator<Item = Self>>(fns: I) -> Self {
        Self::And(fns.into_iter().collect())
    }

    /// A custom check function; return a [`ValidationError`] to reject the
    /// value.
    #[must_use]
    pub fn custom<F>(f: F) -> Self
    where
        F: Fn(&Value) -> Result<(), ValidationError> + Send + Sync + 'static,
    {
        Self::Custom(Arc::new(f))
    }
}

/// `', '.join(str(x) for x in values)`, failing like CPython for integers
/// beyond the digit limit.
pub(crate) fn join_str<'a, I: IntoIterator<Item = &'a Value>>(
    values: I,
) -> Result<String, ValidationError> {
    let mut out = String::new();
    for (i, v) in values.into_iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&v.try_to_string()?);
    }
    Ok(out)
}

/// The uncaught `TypeError` that `re.compile(v)` raises for a value that is
/// neither `str` nor `bytes`. CPython 3.12 first looks the pattern up in a
/// cache keyed by it, so unhashable values fail while hashing.
fn regex_type_error(value: &Value) -> ValidationError {
    let message = unhashable_type(value).map_or_else(
        || "first argument must be string or compiled pattern".to_owned(),
        |name| format!("unhashable type: '{name}'"),
    );
    ValidationError::uncaught("TypeError", message)
}

/// The type of the first unhashable object `hash(value)` meets.
fn unhashable_type(value: &Value) -> Option<&'static str> {
    match value {
        Value::List(_) | Value::Map(_) | Value::Set(_) => Some(value.type_name()),
        Value::Tuple(items) => items.iter().find_map(unhashable_type),
        _ => None,
    }
}
