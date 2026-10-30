//! The Python `str` predicates the parser needs, on `regex-syntax`'s Unicode data (UCD 16; ADR-020):
//! `str.isidentifier()` (`_PyUnicode_ScanIdentifier`: `XID_Start` or `_`, then `XID_Continue`) and
//! `str.isalpha()` (general category `L*`).
//!
//! Owner: WP-13.

use std::sync::OnceLock;

use regex_syntax::hir::{Class, ClassUnicode, HirKind};

/// `name.isidentifier()`.
pub(crate) fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || unicode_ident::is_xid_start(first))
        && chars.all(unicode_ident::is_xid_continue)
}

/// `c.isalpha()`: general categories Lu, Ll, Lt, Lm, Lo.
pub(crate) fn is_alpha(c: char) -> bool {
    static LETTERS: OnceLock<ClassUnicode> = OnceLock::new();
    let class = LETTERS.get_or_init(|| property_class(r"\p{L}"));
    contains(class, c)
}

/// The class of a Unicode property expression (`\p{…}`), empty if `regex-syntax` rejects it.
pub(crate) fn property_class(expr: &str) -> ClassUnicode {
    match regex_syntax::parse(expr).map(regex_syntax::hir::Hir::into_kind) {
        Ok(HirKind::Class(Class::Unicode(class))) => class,
        _ => ClassUnicode::empty(),
    }
}

/// Binary search of `c` in `class`.
pub(crate) fn contains(class: &ClassUnicode, c: char) -> bool {
    class
        .ranges()
        .binary_search_by(|r| {
            if r.end() < c {
                std::cmp::Ordering::Less
            } else if r.start() > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_letters() {
        assert!(is_identifier("a"));
        assert!(is_identifier("_1"));
        assert!(is_identifier("\u{e9}t\u{e9}"));
        assert!(!is_identifier("1a"));
        assert!(!is_identifier(""));
        assert!(!is_identifier("a-b"));
        assert!(!is_identifier("+1"));
        assert!(is_alpha('a'));
        assert!(is_alpha('\u{e9}'));
        assert!(is_alpha('\u{aa}'));
        assert!(
            !is_alpha('\u{345}'),
            "U+0345 is Mn: Rust's is_alphabetic says yes, Python's isalpha says no"
        );
        assert!(!is_alpha(')'));
        assert!(!is_alpha('1'));
    }
}
