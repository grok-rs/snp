//! `pre_commit.commands.run._len_cjk` (02 §13.1): east-asian width W and F count two columns,
//! everything else one (UCD 15.1 table generated from CPython 3.13; not wcwidth).

use crate::chars::in_ranges;
use crate::generated::tables::EAST_ASIAN_WIDE;

/// `sum(2 if unicodedata.east_asian_width(c) in 'WF' else 1 for c in s)`.
#[must_use]
pub fn len_cjk(s: &str) -> usize {
    s.chars().fold(0_usize, |n, c| {
        n.saturating_add(if in_ranges(u32::from(c), EAST_ASIAN_WIDE) {
            2
        } else {
            1
        })
    })
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
    }
}
