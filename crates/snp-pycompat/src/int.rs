//! `int(s)` (base 10) and `clientlib.parse_version` (15 §A4.7; `Objects/longobject.c`
//! `PyLong_FromUnicodeObject`, `_PyUnicode_TransformDecimalAndSpaceToASCII`).

use std::fmt;

use crate::chars::{decimal_value, is_space};

/// The whitespace `int()` skips around the literal.
const fn is_int_space(c: char) -> bool {
    matches!(c, '\t'..='\r' | ' ') || (!c.is_ascii() && is_space(c))
}
use crate::messages;

/// `ValueError` from `int()`; `Display` is CPython's text, whose `%.200R` truncates the repr to
/// 200 characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntParseError {
    /// The original, unstripped literal.
    pub literal: String,
}

impl IntParseError {
    /// The Python exception class: `ValueError`.
    #[must_use]
    pub const fn py_type(&self) -> &'static str {
        messages::VALUE_ERROR
    }
}

impl fmt::Display for IntParseError {
    /// `invalid literal for int() with base 10: '…'` (see [`crate::repr::int_literal_error`]).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::repr::int_literal_error(&self.literal))
    }
}

impl std::error::Error for IntParseError {}

/// `int(s)` in base 10: whitespace stripped at both ends, one optional sign directly before the
/// digits, Unicode `Nd` digits of any script (`int('١٢')` == 12, `int('²')` fails), a single `_`
/// only between digits, no prefix, no inner whitespace. The whitespace is `_PyUnicode_Transform…`'s:
/// non-ASCII PY-WS becomes a space while ASCII stays as is, and `PyLong_FromString` skips only
/// `Py_ISSPACE` (`' \t\n\r\x0b\x0c'`), so `int('\x1c5')` fails although `'\x1c'.isspace()`. CPython ints are unbounded; values outside
/// `i128` are reported as this error (pre-commit only compares small version components).
pub fn parse_int(s: &str) -> Result<i128, IntParseError> {
    let err = || IntParseError {
        literal: s.to_owned(),
    };
    let t = s.trim_matches(is_int_space);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    // Accumulate towards the sign so that i128::MIN parses.
    let mut val: i128 = 0;
    let mut prev_digit = false;
    for c in digits.chars() {
        if c == '_' {
            if !prev_digit {
                return Err(err());
            }
            prev_digit = false;
            continue;
        }
        let d = i128::from(decimal_value(c).ok_or_else(err)?);
        val = val
            .checked_mul(10)
            .and_then(|v| {
                if neg {
                    v.checked_sub(d)
                } else {
                    v.checked_add(d)
                }
            })
            .ok_or_else(err)?;
        prev_digit = true;
    }
    // no digit at all, or a trailing `_`
    if prev_digit { Ok(val) } else { Err(err()) }
}

/// `clientlib.parse_version(s)` (`clientlib.py:50-52`): `tuple(int(p) for p in s.split('.'))`; the
/// first component that is not an int is the error (`'1.x'` → `… base 10: 'x'`).
pub fn parse_version(s: &str) -> Result<Vec<i128>, IntParseError> {
    s.split('.').map(parse_int).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("4.6.2"), Ok(vec![4, 6, 2]));
        assert_eq!(parse_version("1. 2"), Ok(vec![1, 2]));
        assert_eq!(parse_version(" 1.2 "), Ok(vec![1, 2]));
        assert_eq!(parse_version("1_0.2"), Ok(vec![10, 2]));
        assert_eq!(parse_version("\u{663}.1"), Ok(vec![3, 1]));
        assert_eq!(parse_version("1.2\n"), Ok(vec![1, 2]));
        let e = |s: &str| parse_version(s).unwrap_err().to_string();
        assert_eq!(e("1.x"), "invalid literal for int() with base 10: 'x'");
        assert_eq!(e(""), "invalid literal for int() with base 10: ''");
        assert_eq!(e("1..2"), "invalid literal for int() with base 10: ''");
        assert_eq!(
            e("1.2-dev"),
            "invalid literal for int() with base 10: '2-dev'"
        );
        assert_eq!(
            e("it's"),
            "invalid literal for int() with base 10: \"it's\""
        );
    }

    #[test]
    fn ints() {
        assert_eq!(parse_int("-12"), Ok(-12));
        assert_eq!(parse_int("+7"), Ok(7));
        for bad in ["_1", "1_", "1__0", "+_1", "- 1", "0x10", "1.0", "--1"] {
            assert!(parse_int(bad).is_err(), "{bad:?}");
        }
        assert_eq!(parse_int("x").unwrap_err().py_type(), "ValueError");
        assert_eq!(
            parse_int("-170141183460469231731687303715884105728"),
            Ok(i128::MIN)
        );
        assert_eq!(
            parse_int("170141183460469231731687303715884105727"),
            Ok(i128::MAX)
        );
        assert!(parse_int("170141183460469231731687303715884105728").is_err());
        assert_eq!(parse_int("\u{1d7cf}\u{1d7d0}"), Ok(12));
        assert_eq!(parse_int("12\u{660}"), Ok(120));
        assert!(parse_int("\x1c5").is_err());
        assert_eq!(parse_int("\u{a0}5\u{85}"), Ok(5));
    }
}
