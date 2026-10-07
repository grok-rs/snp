//! Python `repr()` and `str()` for [`Value`].

use std::fmt::{self, Write};

use super::Value;
use crate::py;

impl Value {
    /// Python's `repr()` of the value, byte-identical to CPython 3.12.
    ///
    /// ```
    /// use cfgv::Value;
    ///
    /// assert_eq!(Value::from("it's").repr(), r#""it's""#);
    /// assert_eq!(Value::from(1.0).repr(), "1.0");
    /// assert_eq!(Value::Null.repr(), "None");
    /// assert_eq!(Value::Tuple(vec![Value::from("a")]).repr(), "('a',)");
    /// ```
    #[must_use]
    pub fn repr(&self) -> String {
        let mut out = String::new();
        // Writing to a `String` cannot fail.
        let _ = self.write_repr(&mut out);
        out
    }

    /// Writes Python's `repr()` of the value to `out`.
    ///
    /// Dicts, lists, tuples and sets keep their iteration order; a
    /// one-element tuple is written as `(x,)` and an empty set as `set()`.
    ///
    /// # Errors
    ///
    /// Returns an error only if `out` itself fails.
    pub fn write_repr<W: Write + ?Sized>(&self, out: &mut W) -> fmt::Result {
        match self {
            Self::String(s) => py::write_repr_str(out, s),
            Self::Date(d) => d.write_repr(out),
            Self::DateTime(d) => d.write_repr(out),
            _ => self.write_common(out),
        }
    }

    /// The parts of `repr()` and `str()` that coincide.
    fn write_common<W: Write + ?Sized>(&self, out: &mut W) -> fmt::Result {
        match self {
            Self::Null => out.write_str("None"),
            Self::Bool(true) => out.write_str("True"),
            Self::Bool(false) => out.write_str("False"),
            Self::Int(i) => write!(out, "{i}"),
            Self::BigInt(b) => out.write_str(b.as_str()),
            Self::Float(f) => py::write_repr_float(out, *f),
            Self::Bytes(b) => py::write_repr_bytes(out, b),
            Self::List(items) => {
                out.write_char('[')?;
                write_items(out, items)?;
                out.write_char(']')
            }
            Self::Tuple(items) => {
                out.write_char('(')?;
                write_items(out, items)?;
                if items.len() == 1 {
                    out.write_char(',')?;
                }
                out.write_char(')')
            }
            Self::Map(map) => {
                out.write_char('{')?;
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        out.write_str(", ")?;
                    }
                    k.write_repr(out)?;
                    out.write_str(": ")?;
                    v.write_repr(out)?;
                }
                out.write_char('}')
            }
            Self::Set(set) if set.is_empty() => out.write_str("set()"),
            Self::Set(set) => {
                out.write_char('{')?;
                write_items(out, set)?;
                out.write_char('}')
            }
            Self::String(s) => out.write_str(s),
            Self::Date(d) => write!(out, "{d}"),
            Self::DateTime(d) => write!(out, "{d}"),
        }
    }
}

fn write_items<'a, W, I>(out: &mut W, items: I) -> fmt::Result
where
    W: Write + ?Sized,
    I: IntoIterator<Item = &'a Value>,
{
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            out.write_str(", ")?;
        }
        item.write_repr(out)?;
    }
    Ok(())
}

impl fmt::Display for Value {
    /// Python's `str()`: strings are written unquoted, dates and date-times
    /// in ISO format (`2020-01-01`, `2020-01-01 10:00:00+00:00`), and every
    /// other value exactly like [`Value::repr`] (containers show their items
    /// with `repr()`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_common(f)
    }
}
