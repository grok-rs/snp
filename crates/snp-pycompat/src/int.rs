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
    /// `invalid literal for int() with base 10: '…'` (see [`crate::repr::int_literal_error`]), or
    /// for a well-formed literal of more than 4300 digits `Exceeds the limit (4300 digits) for
    /// integer string conversion: value has N digits; use sys.set_int_max_str_digits() to increase
    /// the limit` (`Objects/longobject.c` `_MAX_STR_DIGITS_ERROR_FMT_TO_INT`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match over_digit_limit(&self.literal) {
            Some(digits) => messages::int_digit_limit(f, MAX_STR_DIGITS, digits),
            None => f.write_str(&crate::repr::int_literal_error(&self.literal)),
        }
    }
}

impl std::error::Error for IntParseError {}

/// The result of scanning an `int()` literal.
enum Scan {
    /// Not an `int()` literal at all (`invalid literal …`).
    Malformed,
    /// A well-formed literal whose value fits `i128`, with its digit count.
    Value(i128, usize),
    /// A well-formed literal outside `i128` (`neg` = negative), with its digit count.
    Overflow { neg: bool, digits: usize },
}

/// `sys.int_info.default_max_str_digits` (`_PY_LONG_DEFAULT_MAX_STR_DIGITS`): `int()` of a
/// well-formed literal with more digits raises the `Exceeds the limit` `ValueError` (3.11+).
const MAX_STR_DIGITS: usize = 4300;

fn scan(s: &str) -> Scan {
    let t = s.trim_matches(is_int_space);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.trim_prefix('+')),
    };
    // Accumulate towards the sign so that i128::MIN parses.
    let mut val: Option<i128> = Some(0);
    let mut count: usize = 0;
    let mut prev_digit = false;
    for c in digits.chars() {
        if c == '_' {
            if !prev_digit {
                return Scan::Malformed;
            }
            prev_digit = false;
            continue;
        }
        let Some(d) = decimal_value(c).map(i128::from) else {
            return Scan::Malformed;
        };
        val = val.and_then(|v| v.checked_mul(10)).and_then(|v| {
            if neg {
                v.checked_sub(d)
            } else {
                v.checked_add(d)
            }
        });
        count = count.saturating_add(1);
        prev_digit = true;
    }
    // no digit at all, or a trailing `_`
    if !prev_digit {
        return Scan::Malformed;
    }
    match val {
        Some(v) => Scan::Value(v, count),
        None => Scan::Overflow { neg, digits: count },
    }
}

/// The digit count of a well-formed literal over [`MAX_STR_DIGITS`] (CPython checks the form
/// first, so a malformed long literal keeps the `invalid literal` text).
pub(crate) fn over_digit_limit(s: &str) -> Option<usize> {
    match scan(s) {
        Scan::Value(_, digits) | Scan::Overflow { digits, .. } if digits > MAX_STR_DIGITS => {
            Some(digits)
        }
        _ => None,
    }
}

/// `int(s)` in base 10: whitespace stripped at both ends, one optional sign directly before the
/// digits, Unicode `Nd` digits of any script (`int('١٢')` == 12, `int('²')` fails), a single `_`
/// only between digits, no prefix, no inner whitespace.
///
/// The whitespace is that of `_PyUnicode_TransformDecimalAndSpaceToASCII`: non-ASCII PY-WS becomes
/// a space while ASCII stays as is, and `PyLong_FromString` skips only `Py_ISSPACE`
/// (`' \t\n\r\x0b\x0c'`), so `int('\x1c5')` fails although `'\x1c'.isspace()` is true. A
/// well-formed literal of more than 4300 digits is CPython's `Exceeds the limit (4300 digits) …`
/// error (the error's `Display` says so). CPython ints are unbounded; a well-formed value outside
/// `i128` (at most 4300 digits) is also an error here, whose `Display` is the `invalid literal`
/// text CPython would not raise — [`parse_version`], the one caller that compares such values,
/// saturates instead.
pub fn parse_int(s: &str) -> Result<i128, IntParseError> {
    match scan(s) {
        Scan::Value(v, digits) if digits <= MAX_STR_DIGITS => Ok(v),
        _ => Err(IntParseError {
            literal: s.to_owned(),
        }),
    }
}

/// `clientlib.parse_version(s)` (`clientlib.py:50-52`): `tuple(int(p) for p in s.split('.'))`; the
/// first component that is not an int is the error (`'1.x'` → `… base 10: 'x'`, a component of
/// more than 4300 digits → `Exceeds the limit …`).
///
/// A well-formed component outside `i128` saturates to `i128::MAX` / `i128::MIN`: pre-commit only
/// orders the tuple against `C.VERSION` (`check_min_version`, `clientlib.py:55-61`), and that
/// ordering is the one of CPython's unbounded ints (`'1' * 45` is newer than `4.6.2`).
pub fn parse_version(s: &str) -> Result<Vec<i128>, IntParseError> {
    s.split('.')
        .map(|p| match scan(p) {
            Scan::Value(v, digits) if digits <= MAX_STR_DIGITS => Ok(v),
            Scan::Overflow { neg, digits } if digits <= MAX_STR_DIGITS => {
                Ok(if neg { i128::MIN } else { i128::MAX })
            }
            _ => Err(IntParseError {
                literal: p.to_owned(),
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `s * n` (`str::repeat` is a disallowed method).
    fn rep(s: &str, n: usize) -> String {
        std::iter::repeat_n(s, n).collect()
    }

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
        assert!(parse_int("-170141183460469231731687303715884105729").is_err());
        assert_eq!(parse_int("\u{1d7cf}\u{1d7d0}"), Ok(12));
        assert_eq!(parse_int("12\u{660}"), Ok(120));
        assert!(parse_int("\x1c5").is_err());
        assert_eq!(parse_int("\u{a0}5\u{85}"), Ok(5));
    }

    /// Expected texts and orderings verified against `/usr/bin/python3.12 -I` (3.12.3).
    #[test]
    fn unbounded_components_and_the_digit_limit() {
        let big = rep("1", 45);
        let v = parse_version(&format!("{big}.0")).unwrap();
        assert_eq!(v, vec![i128::MAX, 0]);
        assert!(v > parse_version("4.6.2").unwrap());
        let forty = format!("1{}", rep("0", 39));
        assert!(parse_version(&format!("{forty}.0")).unwrap() > parse_version("4.6.2").unwrap());
        assert_eq!(parse_version(&format!("-{big}")), Ok(vec![i128::MIN]));
        assert_eq!(parse_version(&format!("4.{big}_1")), Ok(vec![4, i128::MAX]));
        // parse_int keeps returning an error outside i128
        assert!(parse_int(&big).is_err());
        let limit = |n: usize| {
            format!(
                "Exceeds the limit (4300 digits) for integer string conversion: value has {n} \
                 digits; use sys.set_int_max_str_digits() to increase the limit"
            )
        };
        let e = |s: &str| parse_version(s).unwrap_err().to_string();
        assert!(parse_version(&rep("1", 4300)).is_ok());
        assert_eq!(e(&rep("1", 4301)), limit(4301));
        assert_eq!(e(&format!("4.{}", rep("0", 4301))), limit(4301));
        assert_eq!(e(&format!("{}1", rep("1_", 4300))), limit(4301));
        assert_eq!(e(&format!(" -{}\n", rep("1", 4301))), limit(4301));
        assert_eq!(e(&rep("\u{661}", 4302)), limit(4302));
        assert_eq!(
            parse_int(&rep("1", 5000)).unwrap_err().to_string(),
            limit(5000)
        );
        // malformed long literals keep the `invalid literal` text
        for bad in [
            format!("{}_", rep("1", 4301)),
            format!("_{}", rep("1", 4301)),
        ] {
            assert!(e(&bad).starts_with("invalid literal for int() with base 10: '"));
        }
    }
}
