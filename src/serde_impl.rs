//! `serde` integration (feature `serde`).

use std::fmt;

use ::serde::de::value::{MapDeserializer, SeqDeserializer};
use ::serde::de::{
    self, DeserializeSeed, Deserializer, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use ::serde::ser::{self, Serialize, SerializeMap, SerializeSeq};
use ::serde::{Deserialize, forward_to_deserialize_any};

use crate::value::{Map, Value};

/// Error returned when deserializing a type from a [`Value`] fails.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeserializeError {
    message: String,
}

impl fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DeserializeError {}

impl de::Error for DeserializeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self {
            message: msg.to_string(),
        }
    }
}

/// Deserializes a `T` from a (typically validated and defaulted) [`Value`].
///
/// Strings, byte strings and map keys are borrowed from `value` where `T`
/// allows it. Big integers are visited as `u64`, `i128` or `u128` when they
/// fit and as their Python `str()` text otherwise; dates and date-times as
/// their ISO text (Python `str()`). Enums are accepted as a string (unit
/// variant) or a single-entry map (`{variant: content}`).
///
/// Two rules keep validated values convertible:
///
/// * Integer targets (`i64`, `u8`, ...) accept booleans as `0` and `1`,
///   since [`CheckFn::check_int`](crate::CheckFn::check_int) accepts them
///   (`bool` is a subclass of `int` in Python). Other targets, including
///   floats, still reject booleans.
/// * Struct fields and enum variants are matched against keys through
///   their Python `str()`: a non-string key (YAML `1:`, `true:`, `null:`)
///   is ignored like any other unknown key, or reported by
///   `#[serde(deny_unknown_fields)]` as e.g. ``unknown field `True` ``. As
///   a consequence, a field renamed to `"1"` also matches the integer key
///   `1`, where a Python dict lookup would not.
///
/// ```
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Hook<'a> {
///     id: &'a str,
///     args: Vec<String>,
///     always_run: bool,
/// }
///
/// let value = cfgv::value_map! {
///     "id" => "black",
///     "args" => vec![cfgv::Value::from("--check")],
///     "always_run" => false,
/// };
/// let hook: Hook<'_> = cfgv::from_value(&value).unwrap();
/// assert_eq!((hook.id, hook.args.as_slice(), hook.always_run), ("black", &["--check".to_owned()][..], false));
/// ```
///
/// # Errors
///
/// Returns a [`DeserializeError`] when the value does not have the shape `T`
/// expects.
pub fn from_value<'de, T: Deserialize<'de>>(value: &'de Value) -> Result<T, DeserializeError> {
    T::deserialize(value)
}

impl<'de> IntoDeserializer<'de, DeserializeError> for &'de Value {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

fn visit_seq<'de, I, V>(items: I, visitor: V) -> Result<V::Value, DeserializeError>
where
    I: Iterator<Item = &'de Value>,
    V: Visitor<'de>,
{
    let mut seq = SeqDeserializer::new(items);
    let value = visitor.visit_seq(&mut seq)?;
    seq.end()?;
    Ok(value)
}

fn visit_map<'de, V: Visitor<'de>>(
    map: &'de Map,
    visitor: V,
) -> Result<V::Value, DeserializeError> {
    let mut access = MapDeserializer::new(map.iter());
    let value = visitor.visit_map(&mut access)?;
    access.end()?;
    Ok(value)
}

/// Integer targets: like `deserialize_any`, but booleans are visited as
/// `0`/`1` because Python's `bool` is an `int` (what `check_int` accepts).
macro_rules! deserialize_integer {
    ($($method:ident)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            match self {
                Value::Bool(b) => visitor.visit_u64(u64::from(*b)),
                _ => self.deserialize_any(visitor),
            }
        }
    )*};
}

impl<'de> Deserializer<'de> for &'de Value {
    type Error = DeserializeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Null => visitor.visit_unit(),
            Value::Bool(b) => visitor.visit_bool(*b),
            Value::Int(i) => visitor.visit_i64(*i),
            Value::BigInt(b) => {
                if let Some(u) = b.to_u64() {
                    visitor.visit_u64(u)
                } else if let Some(i) = b.to_i128() {
                    visitor.visit_i128(i)
                } else if let Some(u) = b.to_u128() {
                    visitor.visit_u128(u)
                } else {
                    visitor.visit_string(b.to_string())
                }
            }
            Value::Float(f) => visitor.visit_f64(*f),
            Value::String(s) => visitor.visit_borrowed_str(s),
            Value::Bytes(b) => visitor.visit_borrowed_bytes(b),
            Value::List(items) | Value::Tuple(items) => visit_seq(items.iter(), visitor),
            Value::Set(items) => visit_seq(items.iter(), visitor),
            Value::Map(map) => visit_map(map, visitor),
            Value::Date(d) => visitor.visit_string(d.to_string()),
            Value::DateTime(d) => visitor.visit_string(d.to_string()),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Value::String(_) => visitor.visit_enum(EnumRef {
                variant: self,
                content: None,
            }),
            Value::Map(map) if map.len() == 1 => match map.iter().next() {
                Some((variant, content)) => visitor.visit_enum(EnumRef {
                    variant,
                    content: Some(content),
                }),
                None => Err(de::Error::custom("expected an enum")),
            },
            _ => Err(de::Error::invalid_type(
                unexpected(self),
                &"a string or a single-key map",
            )),
        }
    }

    /// Struct fields and enum variants: strings as themselves, any other
    /// key as its Python `str()`.
    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Value::String(s) => visitor.visit_borrowed_str(s),
            other => visitor.visit_string(other.to_string()),
        }
    }

    deserialize_integer! {
        deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128
        deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128
    }

    forward_to_deserialize_any! {
        bool f32 f64 char str string bytes byte_buf unit unit_struct seq tuple
        tuple_struct map struct ignored_any
    }
}

fn unexpected(value: &Value) -> de::Unexpected<'_> {
    match value {
        Value::Null => de::Unexpected::Unit,
        Value::Bool(b) => de::Unexpected::Bool(*b),
        Value::Int(i) => de::Unexpected::Signed(*i),
        Value::Float(f) => de::Unexpected::Float(*f),
        Value::String(s) => de::Unexpected::Str(s),
        Value::Bytes(b) => de::Unexpected::Bytes(b),
        Value::List(_) | Value::Tuple(_) | Value::Set(_) => de::Unexpected::Seq,
        Value::Map(_) => de::Unexpected::Map,
        _ => de::Unexpected::Other(value.type_name()),
    }
}

/// `{variant: content}` or a bare `variant` string.
struct EnumRef<'de> {
    variant: &'de Value,
    content: Option<&'de Value>,
}

impl<'de> EnumAccess<'de> for EnumRef<'de> {
    type Error = DeserializeError;
    type Variant = VariantRef<'de>;

    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(self.variant)?;
        Ok((
            variant,
            VariantRef {
                content: self.content,
            },
        ))
    }
}

struct VariantRef<'de> {
    content: Option<&'de Value>,
}

impl<'de> VariantAccess<'de> for VariantRef<'de> {
    type Error = DeserializeError;

    fn unit_variant(self) -> Result<(), Self::Error> {
        match self.content {
            None | Some(Value::Null) => Ok(()),
            Some(other) => Err(de::Error::invalid_type(unexpected(other), &"unit variant")),
        }
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        match self.content {
            Some(content) => seed.deserialize(content),
            None => Err(de::Error::invalid_type(
                de::Unexpected::UnitVariant,
                &"newtype variant",
            )),
        }
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self.content {
            Some(content) => content.deserialize_seq(visitor),
            None => Err(de::Error::invalid_type(
                de::Unexpected::UnitVariant,
                &"tuple variant",
            )),
        }
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self.content {
            Some(content) => content.deserialize_map(visitor),
            None => Err(de::Error::invalid_type(
                de::Unexpected::UnitVariant,
                &"struct variant",
            )),
        }
    }
}

// --- Value: Deserialize ----------------------------------------------------

impl<'de> Deserialize<'de> for Value {
    /// Builds a [`Value`] from any self-describing format (JSON, YAML, ...).
    /// Integers outside the `i64` range become [`Value::BigInt`]; unit and
    /// `None` become [`Value::Null`]; maps keep their order, with later
    /// duplicate keys overwriting earlier ones like a Python `dict`.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Int(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_i128<E: de::Error>(self, v: i128) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_u128<E: de::Error>(self, v: u128) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Ok(Value::Float(v))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Value, E> {
        Ok(Value::Bytes(v.to_vec()))
    }

    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Value, E> {
        Ok(Value::Bytes(v))
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        Value::deserialize(deserializer)
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Value, D::Error> {
        Value::deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::List(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut map = Map::with_capacity(access.size_hint().unwrap_or(0).min(4096));
        while let Some((k, v)) = access.next_entry::<Value, Value>()? {
            map.insert(k, v);
        }
        Ok(Value::Map(map))
    }
}

// --- Value: Serialize ------------------------------------------------------

impl Serialize for Value {
    /// Serializes the value with serde's data model: lists, tuples and sets
    /// as sequences, dicts as maps, bytes as bytes, big integers as `i128`
    /// or `u128` when they fit (as their Python `str()` text otherwise),
    /// dates and date-times as their ISO text.
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(b) => serializer.serialize_bool(*b),
            Self::Int(i) => serializer.serialize_i64(*i),
            Self::BigInt(b) => match (b.to_i128(), b.to_u128()) {
                (Some(i), _) => serializer.serialize_i128(i),
                (None, Some(u)) => serializer.serialize_u128(u),
                (None, None) => serializer.collect_str(b),
            },
            Self::Float(f) => serializer.serialize_f64(*f),
            Self::String(s) => serializer.serialize_str(s),
            Self::Bytes(b) => serializer.serialize_bytes(b),
            Self::List(items) | Self::Tuple(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Self::Set(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Self::Map(map) => {
                let mut out = serializer.serialize_map(Some(map.len()))?;
                for (k, v) in map {
                    out.serialize_entry(k, v)?;
                }
                out.end()
            }
            Self::Date(d) => serializer.collect_str(d),
            Self::DateTime(d) => serializer.collect_str(d),
        }
    }
}
