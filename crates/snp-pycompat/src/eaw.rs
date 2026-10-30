//! `pre_commit.commands.run._len_cjk` (`run.py:36-38`, 02 §13.1): east-asian width W and F count
//! two columns, everything else one (UCD 15.1 table generated from CPython 3.13's `unicodedata`;
//! not wcwidth: combining marks and zero-width characters count one).

use crate::chars::in_ranges;
use crate::generated::tables::EAST_ASIAN_WIDE;

/// The first code point whose width is W or F (U+1100 HANGUL CHOSEONG KIYEOK): everything below
/// counts one column.
const FIRST_WIDE: u32 = 0x1100;

/// Columns of one character: 2 for `east_asian_width(c) in 'WF'`, else 1.
fn width(c: char) -> usize {
    let cp = u32::from(c);
    if cp >= FIRST_WIDE && in_ranges(cp, EAST_ASIAN_WIDE) {
        2
    } else {
        1
    }
}

/// `sum({'A': 1, 'F': 2, 'H': 1, 'N': 1, 'Na': 1, 'W': 2}[unicodedata.east_asian_width(c)] for c in
/// s)`: the hook-name width that sizes the status lines (`_compute_cols`).
#[must_use]
pub fn len_cjk(s: &str) -> usize {
    if s.is_ascii() {
        return s.len();
    }
    s.chars().fold(0_usize, |n, c| n.saturating_add(width(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths() {
        assert_eq!(len_cjk(""), 0);
        assert_eq!(len_cjk("passing hook"), 12);
        assert_eq!(len_cjk("检查中文名称の長いフック"), 24);
        assert_eq!(len_cjk("🎉"), 2);
        assert_eq!(len_cjk("e\u{301}"), 2, "combining marks count 1");
        assert_eq!(len_cjk("\u{200b}"), 1, "zero-width characters count 1");
        assert_eq!(len_cjk("\u{10ff}\u{1100}"), 3);
    }

    #[test]
    fn first_wide_is_the_table_start() {
        assert_eq!(EAST_ASIAN_WIDE.first().map(|r| r.0), Some(FIRST_WIDE));
    }
}
