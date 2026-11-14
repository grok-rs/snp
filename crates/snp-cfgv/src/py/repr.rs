//! `repr()` of Python `str`, `bytes` and `float`.

use std::fmt::{self, Write};

use super::unicode::is_printable;

/// Writes `repr(s)` for a Python `str` to `out`.
///
/// Mirrors CPython's `unicode_repr`: the text is quoted with `'` unless it
/// contains `'` and no `"`, in which case `"` is used. The chosen quote and
/// `\` are backslash-escaped; `\t`, `\n` and `\r` use their short escapes;
/// other C0 controls and U+007F become `\xNN`; non-ASCII characters are kept
/// when printable (see [`is_printable`]) and otherwise escaped as `\xNN`,
/// `\uNNNN` or `\UNNNNNNNN` (lowercase hex).
///
/// # Errors
///
/// Returns an error only if `out` itself fails.
pub fn write_repr_str<W: Write + ?Sized>(out: &mut W, s: &str) -> fmt::Result {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    out.write_char(quote)?;
    // Copy runs of characters that need no escaping in one call.
    let mut run_start = 0;
    for (i, c) in s.char_indices() {
        if needs_no_escape(c, quote) {
            continue;
        }
        out.write_str(s.get(run_start..i).unwrap_or_default())?;
        run_start = i.saturating_add(c.len_utf8());
        write_escaped_char(out, c)?;
    }
    out.write_str(s.get(run_start..).unwrap_or_default())?;
    out.write_char(quote)
}

fn needs_no_escape(c: char, quote: char) -> bool {
    if c == quote || c == '\\' {
        return false;
    }
    let cp = u32::from(c);
    if cp < 0x80 {
        return (0x20..0x7f).contains(&cp);
    }
    is_printable(c)
}

fn write_escaped_char<W: Write + ?Sized>(out: &mut W, c: char) -> fmt::Result {
    let cp = u32::from(c);
    match c {
        '\t' => out.write_str("\\t"),
        '\n' => out.write_str("\\n"),
        '\r' => out.write_str("\\r"),
        // The quote character and the backslash.
        '\'' | '"' | '\\' => {
            out.write_char('\\')?;
            out.write_char(c)
        }
        _ if cp <= 0xff => write!(out, "\\x{cp:02x}"),
        _ if cp <= 0xffff => write!(out, "\\u{cp:04x}"),
        _ => write!(out, "\\U{cp:08x}"),
    }
}

/// Returns `repr(s)` for a Python `str`. See [`write_repr_str`].
#[must_use]
pub fn repr_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_add(2));
    // Writing to a `String` cannot fail.
    let _ = write_repr_str(&mut out, s);
    out
}

/// Writes `repr(b)` for a Python `bytes` object to `out`.
///
/// Mirrors CPython's `bytes_repr`: `b` followed by the bytes quoted with `'`
/// unless they contain `'` and no `"`. The quote and `\` are escaped,
/// `\t`/`\n`/`\r` use short escapes, and every other byte outside
/// `0x20..0x7f` becomes `\xNN`.
///
/// # Errors
///
/// Returns an error only if `out` itself fails.
pub fn write_repr_bytes<W: Write + ?Sized>(out: &mut W, b: &[u8]) -> fmt::Result {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    out.write_char('b')?;
    out.write_char(char::from(quote))?;
    for &byte in b {
        match byte {
            b'\t' => out.write_str("\\t")?,
            b'\n' => out.write_str("\\n")?,
            b'\r' => out.write_str("\\r")?,
            _ if byte == quote || byte == b'\\' => {
                out.write_char('\\')?;
                out.write_char(char::from(byte))?;
            }
            0x20..=0x7e => out.write_char(char::from(byte))?,
            _ => write!(out, "\\x{byte:02x}")?,
        }
    }
    out.write_char(char::from(quote))
}

/// Returns `repr(b)` for a Python `bytes` object. See [`write_repr_bytes`].
#[must_use]
pub fn repr_bytes(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len().saturating_add(3));
    let _ = write_repr_bytes(&mut out, b);
    out
}

/// Writes `repr(x)` (equivalently `str(x)`) for a Python `float` to `out`.
///
/// Uses the shortest digit string that round-trips, laid out like CPython's
/// `float_repr`: positional notation when the decimal exponent `e` of the
/// leading digit satisfies `-4 <= e < 16` (always with a fractional part,
/// e.g. `1.0`), scientific notation otherwise with a sign and at least two
/// exponent digits (`1e+16`, `1.5e-07`). Special values are `inf`, `-inf`
/// and `nan`; negative zero is `-0.0`.
///
/// # Errors
///
/// Returns an error only if `out` itself fails.
pub fn write_repr_float<W: Write + ?Sized>(out: &mut W, x: f64) -> fmt::Result {
    if x.is_nan() {
        return out.write_str("nan");
    }
    if x.is_infinite() {
        return out.write_str(if x.is_sign_positive() { "inf" } else { "-inf" });
    }
    if x.is_sign_negative() {
        out.write_char('-')?;
    }
    let (digits, exponent) = shortest_digits(x.abs());
    // The value is 0.DIGITS * 10^decpt.
    let decpt = exponent.saturating_add(1);
    if !(-4 < decpt && decpt <= 16) {
        let (first, rest) = digits.split_at(digits.len().min(1));
        out.write_str(first)?;
        if !rest.is_empty() {
            out.write_char('.')?;
            out.write_str(rest)?;
        }
        let sign = if exponent < 0 { '-' } else { '+' };
        return write!(out, "e{sign}{:02}", exponent.unsigned_abs());
    }
    if decpt <= 0 {
        out.write_str("0.")?;
        for _ in 0..decpt.unsigned_abs() {
            out.write_char('0')?;
        }
        return out.write_str(&digits);
    }
    let int_len = usize::try_from(decpt).unwrap_or(usize::MAX);
    if int_len >= digits.len() {
        out.write_str(&digits)?;
        for _ in digits.len()..int_len {
            out.write_char('0')?;
        }
        out.write_str(".0")
    } else {
        let (int_part, frac_part) = digits.split_at(int_len);
        out.write_str(int_part)?;
        out.write_char('.')?;
        out.write_str(frac_part)
    }
}

/// Splits Rust's scientific notation (`1.25e-7`) into its significant
/// digits (without trailing zeros) and decimal exponent.
fn split_scientific(sci: &str) -> (String, i32) {
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((sci, "0"));
    let mut digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    (digits, exponent.parse().unwrap_or(0))
}

/// The shortest digit string that round-trips to `x` (finite, positive)
/// and its decimal exponent, choosing like CPython's `dtoa` mode 0 among
/// equally short candidates: the one closest to `x`, ties to even.
fn shortest_digits(x: f64) -> (String, i32) {
    // `{:e}` yields the shortest round-trip digits, e.g. `1.2345e-7`.
    let shortest = format!("{x:e}");
    let (digits, exponent) = split_scientific(&shortest);
    if digits.len() > 1 {
        // Rust may break an exact tie between two equally short candidates
        // differently (2**-25 is 2.98023223876953125e-08). Fixed-precision
        // formatting rounds correctly, ties to even, like CPython; use it
        // whenever it still round-trips.
        let nearest = format!("{x:.*e}", digits.len().saturating_sub(1));
        if nearest != shortest && nearest.parse::<f64>() == Ok(x) {
            return split_scientific(&nearest);
        }
    }
    (digits, exponent)
}

/// Returns `repr(x)` for a Python `float`. See [`write_repr_float`].
#[must_use]
pub fn repr_float(x: f64) -> String {
    let mut out = String::new();
    let _ = write_repr_float(&mut out, x);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_quotes() {
        assert_eq!(repr_str(""), "''");
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("say \"hi\""), "'say \"hi\"'");
        assert_eq!(repr_str("both '\""), "'both \\'\"'");
        assert_eq!(repr_str("'"), "\"'\"");
        assert_eq!(repr_str("\""), "'\"'");
    }

    #[test]
    fn str_escapes() {
        assert_eq!(repr_str("back\\slash"), "'back\\\\slash'");
        assert_eq!(repr_str("t\tn\nr\r"), "'t\\tn\\nr\\r'");
        assert_eq!(repr_str("\0\x07\x1b\x7f"), "'\\x00\\x07\\x1b\\x7f'");
        assert_eq!(repr_str("\u{80}\u{a0}\u{ad}"), "'\\x80\\xa0\\xad'");
        assert_eq!(repr_str("café ß 😀"), "'café ß 😀'");
        assert_eq!(
            repr_str("\u{200b}\u{2028}\u{feff}"),
            "'\\u200b\\u2028\\ufeff'"
        );
        assert_eq!(repr_str("\u{e0001}\u{10ffff}"), "'\\U000e0001\\U0010ffff'");
    }

    #[test]
    fn bytes() {
        assert_eq!(repr_bytes(b""), "b''");
        assert_eq!(repr_bytes(b"it's"), "b\"it's\"");
        assert_eq!(repr_bytes(b"\x00\x7f\x80 \"'"), "b'\\x00\\x7f\\x80 \"\\''");
        assert_eq!(repr_bytes(b"\t\n\r\\"), "b'\\t\\n\\r\\\\'");
    }

    #[test]
    fn floats() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (1.1, "1.1"),
            (0.1, "0.1"),
            (123.456, "123.456"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (9_999_999_999_999_998.0, "9999999999999998.0"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (1.5e-7, "1.5e-07"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (1e22, "1e+22"),
            (0.1 + 0.2, "0.30000000000000004"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
            (-1.5, "-1.5"),
            (123_456_789.0, "123456789.0"),
            // Exact ties between two shortest candidates round to even.
            (2f64.powi(-25), "2.9802322387695312e-08"),
        ];
        for &(x, expected) in cases {
            assert_eq!(repr_float(x), expected, "repr({x:?})");
        }
    }
}
