//! CPython 3.12's `int(s, base)` and `float(s)` on `str` arguments, as PyYAML's `construct_yaml_int` /
//! `construct_yaml_float` call them (after removing `_`).
//!
//! Both first apply `_PyUnicode_TransformDecimalAndSpaceToASCII` (Unicode decimal digits become ASCII digits,
//! non-ASCII whitespace becomes a space, any other non-ASCII character ends the string with `?`), then parse the
//! ASCII text with C-locale rules (`Py_ISSPACE` = `[ \t\n\v\f\r]` only). Errors carry the CPython texts with the
//! repr of the *original* argument.
//!
//! Owner: WP-14.

use cfgv::{BigInt, ParseBigIntError};
use snp_pycompat::chars::{decimal_value, is_space};

use crate::bignum::Big;
use crate::messages::{invalid_float_literal, invalid_int_literal};

/// `_PyUnicode_TransformDecimalAndSpaceToASCII`.
fn to_ascii(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for c in s.chars() {
        if u32::from(c) < 127 {
            // ASCII except DEL is copied as is (`ch < 127`).
            out.push(u8::try_from(u32::from(c)).unwrap_or(b'?'));
        } else if is_space(c) {
            out.push(b' ');
        } else if let Some(d) = decimal_value(c) {
            out.push(b'0'.saturating_add(u8::try_from(d).unwrap_or(0)));
        } else {
            out.push(b'?');
            break;
        }
    }
    out
}

/// `Py_ISSPACE`.
const fn c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// The value of an ASCII digit in `base` (`_PyLong_DigitValue`), if any.
fn digit_value(b: u8, base: u32) -> Option<u32> {
    char::from(b).to_digit(base)
}

/// CPython's `int(s, base)` for `base` in `{2, 8, 10, 16}`: the `Big` value or the `ValueError` text.
///
/// `PyLong_FromString`: leading and trailing C whitespace, one sign, the base prefix (`0b`/`0o`/`0x`, any case)
/// followed by at most one `_`, then digits with single `_` between them, then trailing whitespace. Trailing
/// characters are rejected first (`invalid literal`); only then does a base-10 digit run longer than 4300 digits
/// fail with the `sys.set_int_max_str_digits()` message (`int('1'*5000 + ' x')` is an invalid literal on CPython
/// 3.12 and 3.13, `int('1'*5000 + ' ')` exceeds the limit).
pub(crate) fn py_int(s: &str, base: u32) -> Result<Big, String> {
    let invalid = || invalid_int_literal(base, s);
    let ascii = to_ascii(s);
    let mut p: &[u8] = &ascii;
    while let [b, rest @ ..] = p
        && c_space(*b)
    {
        p = rest;
    }
    let mut neg = false;
    if let [sign @ (b'+' | b'-'), rest @ ..] = p {
        neg = *sign == b'-';
        p = rest;
    }
    if let [b'0', x, rest @ ..] = p {
        let prefixed = matches!(
            (base, x.to_ascii_lowercase()),
            (16, b'x') | (8, b'o') | (2, b'b')
        );
        if prefixed {
            p = rest;
            if let [b'_', rest @ ..] = p {
                p = rest;
            }
        }
    }
    // Digits with single underscores between them (`long_from_string_base`).
    let mut digits: Vec<u8> = Vec::new();
    if let [b'_', ..] = p {
        return Err(invalid());
    }
    let mut prev_underscore = false;
    while let [b, rest @ ..] = p {
        if *b == b'_' {
            if prev_underscore {
                return Err(invalid());
            }
            prev_underscore = true;
        } else if digit_value(*b, base).is_some() {
            digits.push(*b);
            prev_underscore = false;
        } else {
            break;
        }
        p = rest;
    }
    if prev_underscore || digits.is_empty() {
        return Err(invalid());
    }
    while let [b, rest @ ..] = p
        && c_space(*b)
    {
        p = rest;
    }
    if !p.is_empty() {
        return Err(invalid());
    }
    if base == 10 && digits.len() > BigInt::MAX_STR_DIGITS {
        return Err(ParseBigIntError::ExceedsMaxStrDigits {
            digits: digits.len(),
        }
        .to_string());
    }
    Ok(Big::from_ascii_digits(neg, &digits, base))
}

/// CPython's `float(s)`: the value or the `ValueError` text `could not convert string to float: %R`.
///
/// The grammar after stripping C whitespace is `[sign] (digits [. digits] | . digits) [(e|E) [sign] digits]` or
/// `[sign] (inf | infinity | nan)` in any case, correctly rounded — the grammar Rust's `f64::from_str` accepts.
/// Underscores never reach this function (PyYAML removes them first).
pub(crate) fn py_float(s: &str) -> Result<f64, String> {
    let ascii = to_ascii(s);
    let text = std::str::from_utf8(&ascii).unwrap_or("?");
    let trimmed = text.trim_matches(|c: char| u8::try_from(c).is_ok_and(c_space));
    if trimmed.is_empty() || trimmed.contains('_') {
        return Err(invalid_float_literal(s));
    }
    trimmed.parse::<f64>().map_err(|_| invalid_float_literal(s))
}

/// Python's `str.lower()` (full Unicode case mapping, final sigma included).
pub(crate) fn py_lower(s: &str) -> String {
    s.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(s: &str, base: u32) -> Result<String, String> {
        py_int(s, base).map(|b| b.to_value().to_string())
    }

    #[test]
    fn ints() {
        assert_eq!(int(" 12 ", 10), Ok("12".into()));
        assert_eq!(int("-12", 10), Ok("-12".into()));
        assert_eq!(int("0o17", 8), Ok("15".into()));
        assert_eq!(int("17", 8), Ok("15".into()));
        assert_eq!(int("0x_1f", 16), Ok("31".into()));
        assert_eq!(int("\u{661}\u{662}", 10), Ok("12".into()));
        assert_eq!(
            int("", 2),
            Err("invalid literal for int() with base 2: ''".into())
        );
        assert_eq!(
            int("0X1F", 8),
            Err("invalid literal for int() with base 8: '0X1F'".into())
        );
        assert_eq!(
            int("1x", 10),
            Err("invalid literal for int() with base 10: '1x'".into())
        );
        let many: String = std::iter::repeat_n('9', 4301).collect();
        // Trailing characters first (CPython 3.12 and 3.13), then the digit limit.
        for tail in ["x", " x", "-1"] {
            let text = format!("{many}{tail}");
            assert_eq!(int(&text, 10), Err(invalid_int_literal(10, &text)));
        }
        assert_eq!(
            int(&format!("{many} \t"), 10),
            Err("Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; \
                 use sys.set_int_max_str_digits() to increase the limit"
                .into())
        );
    }

    #[test]
    fn floats() {
        assert_eq!(py_float(" 1.5 "), Ok(1.5));
        assert_eq!(py_float("1e3"), Ok(1000.0));
        assert_eq!(py_float("-infinity"), Ok(f64::NEG_INFINITY));
        assert!(py_float("nan").is_ok_and(f64::is_nan));
        assert_eq!(
            py_float("abc"),
            Err("could not convert string to float: 'abc'".into())
        );
        assert!(py_float(".").is_err());
        assert!(py_float("1e").is_err());
    }
}
