//! Character classes of CPython: `str.isspace`, `str.splitlines` boundaries, `bytes.isspace`,
//! `str.isprintable` and `unicodedata.decimal` (15 §A3; tables: CPython 3.13, UCD 15.1).

use core::cmp::Ordering;

use crate::generated::tables;

/// Unicode version of the generated tables (CPython 3.13's `unicodedata.unidata_version`).
pub const UNICODE_VERSION: &str = tables::UNICODE_VERSION;

/// Python `\s` in a `str` pattern as the body of a regex character class (data for snp-pyre):
/// exactly the code points for which [`is_space`] is true.
pub const PY_STR_SPACE_CLASS: &str = r"\t\n\x0B\x0C\r\x1C-\x20\x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}";

/// One Python character: a Unicode scalar value, or a lone surrogate (a `surrogateescape`d byte
/// U+DC80..=U+DCFF from POSIX `os.fsdecode`, or an unpaired UTF-16 unit of a Windows `OsStr`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PyChar {
    /// A Unicode scalar value.
    Scalar(char),
    /// A lone surrogate code point (U+D800..=U+DFFF).
    Surrogate(u16),
}

/// `str.isspace()` for one code point (`Py_UNICODE_ISSPACE`): bidi class WS/B/S or category Zs.
/// Also the set matched by `\s` in a Python `re` *str* pattern (PY-WS).
///
/// Differs from Rust's `char::is_whitespace` (Unicode `White_Space`) by exactly U+001C..=U+001F,
/// which Python treats as whitespace and Rust does not.
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

/// Line boundary of `str.splitlines()` (`Py_UNICODE_ISLINEBREAK`, PY-LB). `"\r\n"` is merged into
/// one boundary by the caller.
#[inline]
#[must_use]
pub const fn is_linebreak(c: char) -> bool {
    matches!(c, '\n'..='\r' | '\x1c'..='\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// `bytes.isspace()` / the default `bytes.strip()` set: `b' \t\n\r\x0b\x0c'` (B-WS).
///
/// Not `u8::is_ascii_whitespace`, which omits `\x0b`.
#[inline]
#[must_use]
pub const fn is_bytes_space(b: u8) -> bool {
    matches!(b, b'\t'..=b'\r' | b' ')
}

/// Whether `cp` lies in one of the sorted, disjoint, inclusive `ranges`.
pub(crate) fn in_ranges(cp: u32, ranges: &[(u32, u32)]) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                Ordering::Less
            } else if lo > cp {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// `str.isprintable()` for one code point (`Py_UNICODE_ISPRINTABLE`, UCD 15.1): false for the
/// categories Cc, Cf, Cs, Co, Cn, Zl, Zp and Zs except U+0020.
#[must_use]
pub fn is_printable(c: char) -> bool {
    let cp = u32::from(c);
    if cp < 0x80 {
        return (0x20..0x7f).contains(&cp);
    }
    !in_ranges(cp, tables::NONPRINTABLE)
}

/// `unicodedata.decimal(c, None)`: the value of a Unicode `Nd` digit of any script.
#[must_use]
pub fn decimal_value(c: char) -> Option<u32> {
    let cp = u32::from(c);
    let idx = tables::DECIMAL_ZEROS.partition_point(|&z| z <= cp);
    let zero = *tables::DECIMAL_ZEROS.get(idx.checked_sub(1)?)?;
    let d = cp.checked_sub(zero)?;
    (d < 10).then_some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_classes_differ_from_rust_std() {
        assert!(is_space('\x1f') && !'\x1f'.is_whitespace());
        assert!(is_space('\u{a0}') && is_space('\u{3000}') && !is_space('\u{200b}'));
        assert!(is_bytes_space(b'\x0b') && !b'\x0b'.is_ascii_whitespace());
        assert!(!is_bytes_space(0xa0));
    }

    #[test]
    fn linebreaks() {
        for c in [
            '\n', '\r', '\x0b', '\x0c', '\x1c', '\x1d', '\x1e', '\u{85}', '\u{2028}', '\u{2029}',
        ] {
            assert!(is_linebreak(c), "{c:?}");
        }
        assert!(!is_linebreak('\x1f') && !is_linebreak(' ') && !is_linebreak('\t'));
    }

    #[test]
    fn printable() {
        assert!(is_printable('a') && is_printable(' ') && is_printable('é') && is_printable('😀'));
        for c in [
            '\x7f', '\u{80}', '\u{a0}', '\u{ad}', '\u{378}', '\u{200b}', '\u{2028}', '\u{feff}',
            '\u{e000}',
        ] {
            assert!(!is_printable(c), "{c:?}");
        }
        assert!(!is_printable('\u{1fae9}'), "unassigned in UCD 15.1");
    }

    #[test]
    fn decimals() {
        assert_eq!(decimal_value('7'), Some(7));
        assert_eq!(decimal_value('\u{663}'), Some(3)); // ARABIC-INDIC DIGIT THREE
        assert_eq!(decimal_value('a'), None);
        assert_eq!(decimal_value('\u{b2}'), None); // superscript two is not Nd
    }

    #[test]
    fn space_class_matches_is_space() {
        assert_eq!(UNICODE_VERSION, "15.1.0");
        assert!(PY_STR_SPACE_CLASS.contains(r"\x1C-\x20"));
    }
}
