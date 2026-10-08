//! `int(s)` (base 10) and `clientlib.parse_version` (15 §A4.7).

use std::fmt;

use crate::chars::{decimal_value, is_space};

/// `ValueError` from `int()`; `Display` is CPython's text, whose `%.200R` truncates the repr to
/// 200 characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntParseError {
    /// The original, unstripped literal.
    pub literal: String,
}

impl IntParseError {
    /// The Python exception class: `ValueError`.
    pub const fn py_type(&self) -> &'static str {
        "ValueError"
    }
}

impl fmt::Display for IntParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::repr::int_literal_error(&self.literal))
    }
}

impl std::error::Error for IntParseError {}

/// `int(s)` in base 10: PY-WS stripped at both ends, one optional sign directly before the digits,
/// Unicode `Nd` digits of any script, single `_` only between digits. Values outside `i128` are
/// reported as errors (pre-commit only compares small version components).
pub fn parse_int(s: &str) -> Result<i128, IntParseError> {
    let err = || IntParseError {
        literal: s.to_owned(),
    };
    let t = s.trim_matches(is_space);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let mut val: i128 = 0;
    let mut prev_digit = false;
    let mut any = false;
    for c in digits.chars() {
        if c == '_' {
            if !prev_digit {
                return Err(err());
            }
            prev_digit = false;
            continue;
        }
        let d = decimal_value(c).ok_or_else(err)?;
        val = val
            .checked_mul(10)
            .and_then(|v| v.checked_add(i128::from(d)))
            .ok_or_else(err)?;
        prev_digit = true;
        any = true;
    }
    if !any || !prev_digit {
        return Err(err());
    }
    if neg {
        val.checked_neg().ok_or_else(err)
    } else {
        Ok(val)
    }
}

/// `clientlib.parse_version(s)`: `tuple(int(p) for p in s.split('.'))`.
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
    }
}
