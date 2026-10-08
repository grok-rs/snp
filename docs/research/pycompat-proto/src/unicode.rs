//! Character classes of CPython 3.12 (Unicode 15.0.0).
use core::cmp::Ordering;

use crate::tables::{DECIMAL_ZEROS, NONPRINTABLE};

/// `str.isspace()` for one code point (`Py_UNICODE_ISSPACE`): bidi class WS/B/S or
/// category Zs. Also the set matched by `\s` in a Python `re` *str* pattern.
///
/// Differs from Rust's `char::is_whitespace` (Unicode `White_Space`) by exactly
/// U+001C..=U+001F, which Python treats as whitespace and Rust does not.
#[inline]
#[must_use]
pub const fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | '\x1c'..=' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// Line boundary for `str.splitlines()` (`Py_UNICODE_ISLINEBREAK`).
/// `\r\n` is handled by the caller as one boundary.
#[inline]
#[must_use]
pub const fn is_linebreak(c: char) -> bool {
    matches!(c, '\n'..='\r' | '\x1c'..='\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// `bytes.isspace()` / `bytes.strip()` set: `b' \t\n\r\x0b\x0c'`.
///
/// NOT `u8::is_ascii_whitespace`, which omits `\x0b`.
#[inline]
#[must_use]
pub const fn is_ascii_space(b: u8) -> bool {
    matches!(b, b'\t'..=b'\r' | b' ')
}

/// `str.isprintable()` for one code point (`Py_UNICODE_ISPRINTABLE`): false for
/// categories Cc, Cf, Cs, Co, Cn, Zl, Zp and Zs except U+0020.
#[must_use]
pub fn is_printable(c: char) -> bool {
    let cp = u32::from(c);
    if cp < 0x80 {
        return (0x20..0x7f).contains(&cp);
    }
    NONPRINTABLE
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                Ordering::Less
            } else if lo > cp {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_err()
}

/// `unicodedata.decimal(c)` (Unicode `Nd` digit value).
#[must_use]
pub fn decimal_value(c: char) -> Option<u32> {
    let cp = u32::from(c);
    let idx = DECIMAL_ZEROS.partition_point(|&z| z <= cp);
    let zero = *DECIMAL_ZEROS.get(idx.checked_sub(1)?)?;
    let d = cp - zero;
    (d < 10).then_some(d)
}
