//! `str.isprintable()` for single code points.

use std::cmp::Ordering;

use super::tables;

/// Version of the Unicode database that [`is_printable`] follows.
///
/// cfgv pins the database of CPython 3.12 (`"15.0.0"`). Later CPython
/// releases reclassify a handful of newly assigned code points, which only
/// changes how those characters appear in `repr()` output.
pub const UNICODE_VERSION: &str = tables::UNICODE_VERSION;

/// Returns CPython's `c.isprintable()` for a single character.
///
/// A character is printable unless its general category is one of `Cc`,
/// `Cf`, `Cs`, `Co`, `Cn`, `Zl`, `Zp` or `Zs` — except that the ASCII space
/// is printable. `repr()` of a `str` escapes exactly the non-printable
/// characters (plus quotes and backslashes).
///
/// ```
/// assert!(cfgv::py::is_printable('é'));
/// assert!(cfgv::py::is_printable(' '));
/// assert!(!cfgv::py::is_printable('\u{a0}'));
/// assert!(!cfgv::py::is_printable('\u{200b}'));
/// ```
#[must_use]
pub fn is_printable(c: char) -> bool {
    let cp = u32::from(c);
    if cp < 0x80 {
        return (0x20..0x7f).contains(&cp);
    }
    tables::NONPRINTABLE
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_disjoint() {
        let mut prev_hi = 0x7f;
        for &(lo, hi) in tables::NONPRINTABLE {
            assert!(lo > prev_hi, "{lo:#x} overlaps or is out of order");
            assert!(lo <= hi);
            prev_hi = hi;
        }
    }

    #[test]
    fn samples() {
        assert!(is_printable('a'));
        assert!(!is_printable('\u{7f}'));
        assert!(!is_printable('\u{85}'));
        assert!(is_printable('\u{fffd}'));
        assert!(!is_printable('\u{ffff}'));
        assert!(!is_printable('\u{e000}'));
        assert!(is_printable('😀'));
        assert!(!is_printable('\u{1fae9}'));
        assert!(!is_printable('\u{10ffff}'));
    }
}
