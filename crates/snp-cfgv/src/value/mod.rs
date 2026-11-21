//! The dynamically typed [`Value`] tree that cfgv validates.

mod bigint;
mod cmp;
mod datetime;
mod fmt;
mod hashed;

use indexmap::{IndexMap, IndexSet};

pub use bigint::{BigInt, MaxStrDigitsError, ParseBigIntError};
pub use cmp::StrKey;
pub(crate) use cmp::py_sort;
pub use datetime::{Date, DateError, DateTime, TzOffset};
pub use hashed::{HashedKey, ValueState};

/// A Python `dict`: insertion-ordered, with [`Value`] keys, hashed with the
/// process-wide [`ValueState`] (build one with `Map::default()` or
/// `Map::with_capacity_and_hasher(n, ValueState)`).
pub type Map = IndexMap<Value, Value, ValueState>;

/// A Python `set`. cfgv keeps insertion order (CPython iterates sets in hash
/// order, which is not reproducible for strings). Hashed like [`Map`].
pub type Set = IndexSet<Value, ValueState>;

/// A dynamically typed value mirroring the Python objects that PyYAML's
/// `SafeLoader` produces and Python cfgv validates.
///
/// | Variant | Python type (`type_name`) |
/// |---|---|
/// | [`Null`](Value::Null) | `NoneType` |
/// | [`Bool`](Value::Bool) | `bool` |
/// | [`Int`](Value::Int), [`BigInt`](Value::BigInt) | `int` |
/// | [`Float`](Value::Float) | `float` |
/// | [`String`](Value::String) | `str` |
/// | [`Bytes`](Value::Bytes) | `bytes` |
/// | [`List`](Value::List) | `list` |
/// | [`Tuple`](Value::Tuple) | `tuple` |
/// | [`Map`](Value::Map) | `dict` |
/// | [`Set`](Value::Set) | `set` |
/// | [`Date`](Value::Date) | `date` |
/// | [`DateTime`](Value::DateTime) | `datetime` |
///
/// # Equality and hashing
///
/// [`PartialEq`] and [`Hash`] follow Python's `==` and `hash()`: numbers
/// compare by value across `bool`, `int` and `float` (`True == 1 == 1.0`,
/// `0.0 == -0.0`), dicts and sets compare without regard to order, lists and
/// tuples never compare equal to each other, and aware date-times compare by
/// instant. The one deliberate difference is that NaN equals NaN, which keeps
/// `Eq` lawful so that values can be used as map keys.
///
/// # Formatting
///
/// [`Value::repr`] is Python's `repr()`; [`Display`](std::fmt::Display) is
/// Python's `str()` (strings unquoted, dates in ISO format, everything else
/// as `repr()`). Where CPython raises because an integer has more than 4300
/// decimal digits, [`Value::try_repr`] and [`Value::try_to_string`] fail
/// and the infallible forms write the integer in hexadecimal.
///
/// # Strings and lone surrogates
///
/// [`Value::String`] holds a Rust [`String`], which cannot contain the
/// surrogate code points `U+D800..=U+DFFF`. A Python `str` can: PyYAML turns
/// the escape `"\ud800"` of a double-quoted scalar into a lone surrogate, and
/// turns `"\ud83d\ude00"` into *two* code points, not one emoji. Such values
/// cannot be represented, and their Python `repr()` (`'\ud800'`) cannot be
/// reproduced. A loader should reject them with an error naming the
/// escape, or, when losing them is acceptable, replace each surrogate
/// with `U+FFFD`; it should not combine escaped surrogate pairs into one
/// character, which PyYAML never does. Input files themselves are strict
/// UTF-8 and never contain surrogates.
///
/// # Nesting depth
///
/// Cloning, dropping, comparing, hashing and formatting recurse into nested
/// values, so a pathologically deep value (many thousands of levels) can
/// exhaust the stack. Validation itself only recurses as deep as the
/// schema. Loaders that build values from untrusted input should cap the
/// nesting depth (PyYAML-compatible loaders typically allow a few hundred
/// levels).
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum Value {
    /// `None`.
    #[default]
    Null,
    /// `True` / `False`.
    Bool(bool),
    /// An `int` that fits in an `i64`.
    Int(i64),
    /// An `int` outside the `i64` range.
    BigInt(BigInt),
    /// A `float`.
    Float(f64),
    /// A `str` (without lone surrogates, see
    /// [Strings and lone surrogates](#strings-and-lone-surrogates)).
    String(String),
    /// A `bytes` object (PyYAML `!!binary`).
    Bytes(Vec<u8>),
    /// A `list`.
    List(Vec<Self>),
    /// A `tuple` (PyYAML produces 2-tuples for `!!omap` and `!!pairs`).
    Tuple(Vec<Self>),
    /// A `dict`.
    Map(Map),
    /// A `set` (PyYAML `!!set`).
    Set(Set),
    /// A `datetime.date`.
    Date(Date),
    /// A `datetime.datetime`.
    DateTime(DateTime),
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the comparison helpers live in value/cmp.rs and the formatting methods in value/fmt.rs"
)]
impl Value {
    /// The Python type name used in error messages (`type(v).__name__`).
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "NoneType",
            Self::Bool(_) => "bool",
            Self::Int(_) | Self::BigInt(_) => "int",
            Self::Float(_) => "float",
            Self::String(_) => "str",
            Self::Bytes(_) => "bytes",
            Self::List(_) => "list",
            Self::Tuple(_) => "tuple",
            Self::Map(_) => "dict",
            Self::Set(_) => "set",
            Self::Date(_) => "date",
            Self::DateTime(_) => "datetime",
        }
    }

    /// Whether the value is `None`.
    #[must_use]
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// The boolean, if the value is a [`Bool`](Value::Bool).
    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The integer, if the value is an [`Int`](Value::Int) (or a
    /// [`BigInt`](Value::BigInt) that fits in an `i64`). Booleans are not
    /// converted.
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            Self::BigInt(b) => b.to_i64(),
            _ => None,
        }
    }

    /// The float, if the value is a [`Float`](Value::Float).
    #[must_use]
    pub const fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// The string, if the value is a [`String`](Value::String).
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// The bytes, if the value is [`Bytes`](Value::Bytes).
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// The items, if the value is a [`List`](Value::List).
    #[must_use]
    pub fn as_list(&self) -> Option<&[Self]> {
        match self {
            Self::List(l) => Some(l),
            _ => None,
        }
    }

    /// The items, if the value is a [`List`](Value::List) or a
    /// [`Tuple`](Value::Tuple) — what cfgv's `check_array` accepts.
    #[must_use]
    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::List(l) | Self::Tuple(l) => Some(l),
            _ => None,
        }
    }

    /// The map, if the value is a [`Map`](Value::Map).
    #[must_use]
    pub const fn as_map(&self) -> Option<&Map> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }

    /// The map, mutably, if the value is a [`Map`](Value::Map).
    #[must_use]
    pub const fn as_map_mut(&mut self) -> Option<&mut Map> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }

    /// The set, if the value is a [`Set`](Value::Set).
    #[must_use]
    pub const fn as_set(&self) -> Option<&Set> {
        match self {
            Self::Set(s) => Some(s),
            _ => None,
        }
    }

    /// The date, if the value is a [`Date`](Value::Date).
    #[must_use]
    pub const fn as_date(&self) -> Option<Date> {
        match self {
            Self::Date(d) => Some(*d),
            _ => None,
        }
    }

    /// The date-time, if the value is a [`DateTime`](Value::DateTime).
    #[must_use]
    pub const fn as_datetime(&self) -> Option<DateTime> {
        match self {
            Self::DateTime(d) => Some(*d),
            _ => None,
        }
    }

    /// Converts into the map, if the value is a [`Map`](Value::Map).
    #[must_use]
    pub fn into_map(self) -> Option<Map> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }

    /// Converts into the items, if the value is a [`List`](Value::List).
    #[must_use]
    pub fn into_list(self) -> Option<Vec<Self>> {
        match self {
            Self::List(l) => Some(l),
            _ => None,
        }
    }

    /// Converts into the string, if the value is a [`String`](Value::String).
    #[must_use]
    pub fn into_string(self) -> Option<String> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }
}

// --- Conversions ---------------------------------------------------------

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}

macro_rules! value_from_small_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(i: $t) -> Self {
                Self::Int(i64::from(i))
            }
        }
    )*};
}

value_from_small_int!(i8, i16, i32, i64, u8, u16, u32);

macro_rules! value_from_wide_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            /// Produces [`Value::Int`] when the integer fits in an `i64`,
            /// otherwise [`Value::BigInt`].
            fn from(i: $t) -> Self {
                i64::try_from(i).map_or_else(|_| Self::BigInt(BigInt::from(i)), Self::Int)
            }
        }
    )*};
}

value_from_wide_int!(u64, i128, u128, isize, usize);

impl From<BigInt> for Value {
    /// Produces [`Value::Int`] when the integer fits in an `i64`, otherwise
    /// [`Value::BigInt`].
    fn from(b: BigInt) -> Self {
        b.to_i64().map_or(Self::BigInt(b), Self::Int)
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Self {
        Self::Float(f)
    }
}

impl From<f32> for Value {
    fn from(f: f32) -> Self {
        Self::Float(f64::from(f))
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Self::String(s.to_owned())
    }
}

impl From<std::borrow::Cow<'_, str>> for Value {
    fn from(s: std::borrow::Cow<'_, str>) -> Self {
        Self::String(s.into_owned())
    }
}

impl From<Vec<u8>> for Value {
    fn from(b: Vec<u8>) -> Self {
        Self::Bytes(b)
    }
}

impl From<&[u8]> for Value {
    fn from(b: &[u8]) -> Self {
        Self::Bytes(b.to_vec())
    }
}

impl From<Vec<Self>> for Value {
    fn from(l: Vec<Self>) -> Self {
        Self::List(l)
    }
}

impl From<Map> for Value {
    fn from(m: Map) -> Self {
        Self::Map(m)
    }
}

impl From<Set> for Value {
    fn from(s: Set) -> Self {
        Self::Set(s)
    }
}

impl From<Date> for Value {
    fn from(d: Date) -> Self {
        Self::Date(d)
    }
}

impl From<DateTime> for Value {
    fn from(d: DateTime) -> Self {
        Self::DateTime(d)
    }
}

impl<T: Into<Self>> From<Option<T>> for Value {
    /// `None` becomes [`Value::Null`].
    fn from(v: Option<T>) -> Self {
        v.map_or(Self::Null, Into::into)
    }
}

impl FromIterator<Self> for Value {
    /// Collects into a [`Value::List`].
    fn from_iter<I: IntoIterator<Item = Self>>(iter: I) -> Self {
        Self::List(iter.into_iter().collect())
    }
}

impl FromIterator<(Self, Self)> for Value {
    /// Collects into a [`Value::Map`] (later duplicates win, like a Python
    /// `dict`).
    fn from_iter<I: IntoIterator<Item = (Self, Self)>>(iter: I) -> Self {
        Self::Map(iter.into_iter().collect())
    }
}

/// Builds a [`Value::Map`] from `key => value` pairs; keys and values are
/// converted with [`Value::from`].
///
/// ```
/// use cfgv::{Value, value_map};
///
/// let val = value_map! {
///     "name" => "example",
///     "enabled" => true,
/// };
/// assert_eq!(val.repr(), "{'name': 'example', 'enabled': True}");
/// ```
#[macro_export]
macro_rules! value_map {
    ($($key:expr => $val:expr),* $(,)?) => {{
        // Inserted in order: a repeated key keeps its first position and takes the last value.
        let map: $crate::Map = ::core::iter::Iterator::collect(::core::iter::IntoIterator::into_iter([
            $(($crate::Value::from($key), $crate::Value::from($val))),*
        ]));
        $crate::Value::Map(map)
    }};
}
