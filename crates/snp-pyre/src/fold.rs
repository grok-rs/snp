//! Character sets and IGNORECASE folding (01 Addendum §A.6.6, §A.6.7).
//!
//! * str, Unicode semantics: the simple case-folding closure (`regex_syntax`'s `ClassUnicode::case_fold_simple`,
//!   UCD 16), plus the Turkish-i rule: a set meeting `{I, i, İ, ı}` gets all four (CPython's `_EXTRA_CASES`
//!   and `unicode_tolower`, `re/_compiler.py:40-97`, `re/_casefix.py`).
//! * str with `(?a)`, and bytes: only ASCII letters gain their other case.
//!
//! Owner: WP-13.

use regex_syntax::hir::{ClassUnicode, ClassUnicodeRange};

/// The Turkish-i family (U+0049, U+0069, U+0130, U+0131).
const TURKISH_I: [u32; 4] = [0x49, 0x69, 0x130, 0x131];

/// A sorted set of disjoint, non-adjacent inclusive ranges of code units.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RangeSet {
    ranges: Vec<(u32, u32)>,
}

impl RangeSet {
    /// The set of `ranges` (any order, overlaps allowed; `lo > hi` pairs are ignored).
    pub(crate) fn new(mut ranges: Vec<(u32, u32)>) -> Self {
        ranges.retain(|&(lo, hi)| lo <= hi);
        ranges.sort_unstable();
        let mut out: Vec<(u32, u32)> = Vec::new();
        for (lo, hi) in ranges {
            match out.last_mut() {
                Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
                _ => out.push((lo, hi)),
            }
        }
        Self { ranges: out }
    }

    /// The ranges, ascending.
    pub(crate) fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    /// No member.
    pub(crate) fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// A single member, if the set has exactly one.
    pub(crate) fn single(&self) -> Option<u32> {
        match self.ranges.as_slice() {
            [(lo, hi)] if lo == hi => Some(*lo),
            _ => None,
        }
    }

    /// Removes the surrogates U+D800–U+DFFF (str patterns; they never occur in a UTF-8 path, §A.5.4).
    pub(crate) fn clip_surrogates(&mut self) {
        let mut out = Vec::new();
        for &(lo, hi) in &self.ranges {
            if hi < 0xD800 || lo > 0xDFFF {
                out.push((lo, hi));
                continue;
            }
            if lo < 0xD800 {
                out.push((lo, 0xD7FF));
            }
            if hi > 0xDFFF {
                out.push((0xE000, hi));
            }
        }
        self.ranges = out;
    }

    /// Unicode IGNORECASE (`fold(S)` of §A.6.6). Surrogates must have been clipped.
    pub(crate) fn fold_unicode(&mut self) {
        let mut class = ClassUnicode::new(self.ranges.iter().filter_map(|&(lo, hi)| {
            Some(ClassUnicodeRange::new(
                char::from_u32(lo)?,
                char::from_u32(hi)?,
            ))
        }));
        if class.try_case_fold_simple().is_err() {
            // Unreachable with regex-syntax's `unicode-case` feature (a default feature); keep the set.
            return;
        }
        let mut ranges: Vec<(u32, u32)> = class
            .ranges()
            .iter()
            .map(|r| (u32::from(r.start()), u32::from(r.end())))
            .collect();
        let folded = Self::new(ranges.clone());
        if TURKISH_I.iter().any(|&c| folded.contains(c)) {
            ranges.extend(TURKISH_I.iter().map(|&c| (c, c)));
        }
        *self = Self::new(ranges);
    }

    /// ASCII IGNORECASE: every ASCII letter gains its other case (str `(?a)`, bytes, `(?L)`).
    pub(crate) fn fold_ascii(&mut self) {
        let mut extra = Vec::new();
        for &(lo, hi) in &self.ranges {
            for (a, b, delta_up) in [(0x41_u32, 0x5A_u32, true), (0x61, 0x7A, false)] {
                let (s, e) = (lo.max(a), hi.min(b));
                if s <= e {
                    if delta_up {
                        extra.push((s.saturating_add(0x20), e.saturating_add(0x20)));
                    } else {
                        extra.push((s.saturating_sub(0x20), e.saturating_sub(0x20)));
                    }
                }
            }
        }
        if !extra.is_empty() {
            extra.extend_from_slice(&self.ranges);
            *self = Self::new(extra);
        }
    }

    /// Membership.
    pub(crate) fn contains(&self, c: u32) -> bool {
        self.ranges
            .binary_search_by(|&(lo, hi)| {
                if hi < c {
                    std::cmp::Ordering::Less
                } else if lo > c {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folded(c: char) -> Vec<(u32, u32)> {
        let mut s = RangeSet::new(vec![(u32::from(c), u32::from(c))]);
        s.fold_unicode();
        s.ranges().to_vec()
    }

    #[test]
    fn unicode_folding() {
        assert_eq!(
            folded('k'),
            vec![(0x4B, 0x4B), (0x6B, 0x6B), (0x212A, 0x212A)]
        );
        assert_eq!(
            folded('i'),
            vec![(0x49, 0x49), (0x69, 0x69), (0x130, 0x131)]
        );
        assert_eq!(
            folded('\u{130}'),
            vec![(0x49, 0x49), (0x69, 0x69), (0x130, 0x131)]
        );
        assert_eq!(folded('1'), vec![(0x31, 0x31)]);
        assert_eq!(
            folded('s'),
            vec![(0x53, 0x53), (0x73, 0x73), (0x17F, 0x17F)]
        );
    }

    #[test]
    fn ascii_folding_and_clipping() {
        let mut s = RangeSet::new(vec![(0x5A, 0x62), (0xE9, 0xE9)]);
        s.fold_ascii();
        assert_eq!(
            s.ranges(),
            &[(0x41, 0x42), (0x5A, 0x62), (0x7A, 0x7A), (0xE9, 0xE9)]
        );
        let mut s = RangeSet::new(vec![(0xD000, 0xE100)]);
        s.clip_surrogates();
        assert_eq!(s.ranges(), &[(0xD000, 0xD7FF), (0xE000, 0xE100)]);
        let mut s = RangeSet::new(vec![(0xD800, 0xDFFF)]);
        s.clip_surrogates();
        assert!(s.is_empty());
    }
}
