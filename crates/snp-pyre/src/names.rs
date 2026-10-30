//! `\N{name}` (01 Addendum §A.5.4): CPython 3.12's `unicodedata.lookup` (`Modules/unicodedata.c` `_getcode`)
//! on `unicode_names2` 4.0.0 (UCD 16) restricted to what CPython 3.12 (UCD 15.0) knows.
//!
//! * `HANGUL SYLLABLE …` and `CJK UNIFIED IDEOGRAPH-…` are algorithmic: the prefix is ASCII-case-insensitive,
//!   the jamo short names and the 4–5 hex digits are case-sensitive (uppercase), as in `_getcode`.
//! * Every other name and every `NameAliases.txt` alias is matched ASCII-case-insensitively and exactly (no
//!   UAX44-LM2 loose matching); names of code points unassigned in UCD 15.0 and `TANGUT IDEOGRAPH-…` (no
//!   algorithmic names in 3.12) are unknown; named sequences are rejected (`ord()` of several characters).
//!
//! Behind the `py-re-names` feature (default on); without it `\N{…}` is `Unsupported` (§A.6.13 D3).
//!
//! Owner: WP-13.

use crate::error::PyReError;

/// `unicodedata.lookup(name)` as a code point, `Ok(None)` for `undefined character name`, or `Unsupported`
/// without the `py-re-names` feature.
#[cfg(not(feature = "py-re-names"))]
pub(crate) fn lookup(_name: &str) -> Result<Option<u32>, PyReError> {
    Err(PyReError::Unsupported(
        crate::messages::NAMES_DISABLED.into(),
    ))
}

/// `unicodedata.lookup(name)` as a code point, `Ok(None)` for `undefined character name`.
#[cfg(feature = "py-re-names")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "one signature with the `py-re-names`-off variant, which errs"
)]
pub(crate) fn lookup(name: &str) -> Result<Option<u32>, PyReError> {
    Ok(imp::lookup(name))
}

#[cfg(feature = "py-re-names")]
mod imp {
    use crate::generated::names312::{ALIASES, ASSIGNED, CJK_UNIFIED};

    /// `NAME_MAXLEN` (`Modules/unicodedata.c`): longer names are `KeyError("name too long")`.
    const NAME_MAXLEN: usize = 256;

    /// The algorithmic name prefixes (compared case-sensitively, `strncmp` in `_getcode`).
    const HANGUL_PREFIX: &str = "HANGUL SYLLABLE ";
    const CJK_PREFIX: &str = "CJK UNIFIED IDEOGRAPH-";

    /// Hangul syllable composition constants (`Modules/unicodedata.c`).
    const S_BASE: u32 = 0xAC00;
    const V_COUNT: u32 = 21;
    const T_COUNT: u32 = 28;
    /// `hangul_syllables` columns (L, V, T); `find_syllable` takes the first longest match.
    const L_NAMES: [&str; 19] = [
        "G", "GG", "N", "D", "DD", "R", "M", "B", "BB", "S", "SS", "", "J", "JJ", "C", "K", "T",
        "P", "H",
    ];
    const V_NAMES: [&str; 21] = [
        "A", "AE", "YA", "YAE", "EO", "E", "YEO", "YE", "O", "WA", "WAE", "OE", "YO", "U", "WEO",
        "WE", "WI", "YU", "EU", "YI", "I",
    ];
    const T_NAMES: [&str; 28] = [
        "", "G", "GG", "GS", "N", "NJ", "NH", "D", "L", "LG", "LM", "LB", "LS", "LT", "LP", "LH",
        "M", "B", "BS", "S", "SS", "NG", "J", "C", "K", "T", "P", "H",
    ];

    pub(super) fn lookup(name: &str) -> Option<u32> {
        if name.len() > NAME_MAXLEN {
            return None;
        }
        if let Some(rest) = name.strip_prefix(HANGUL_PREFIX) {
            return hangul(rest);
        }
        if let Some(rest) = name.strip_prefix(CJK_PREFIX) {
            return cjk(rest);
        }
        let upper = name.to_ascii_uppercase();
        if let Ok(i) = ALIASES.binary_search_by(|(alias, _)| (*alias).cmp(upper.as_str())) {
            return ALIASES.get(i).map(|&(_, c)| c);
        }
        // The name database has no algorithmic names: other spellings of these prefixes, and Tangut
        // ideographs (named algorithmically only from 3.14 on), are unknown.
        if [HANGUL_PREFIX, CJK_PREFIX, "TANGUT IDEOGRAPH-"]
            .iter()
            .any(|p| upper.starts_with(p))
        {
            return None;
        }
        // Canonical names are `[A-Z0-9][A-Z0-9 -]*`. Anything else can never equal one, and must not reach
        // `unicode_names2::character`: its loose-matching normaliser underflows on a leading `-`.
        let mut bytes = upper.bytes();
        if !bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
            || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b' ' || b == b'-')
        {
            return None;
        }
        let c = unicode_names2::character(name)?;
        if !in_ranges(ASSIGNED, u32::from(c)) {
            return None;
        }
        let canonical = unicode_names2::name(c)?.to_string();
        (canonical == upper).then_some(u32::from(c))
    }

    /// `find_syllable`: the index and length of the first longest entry of `table` that prefixes `s`.
    fn find_syllable(s: &str, table: &[&str]) -> Option<(u32, usize)> {
        let mut best: Option<(u32, usize)> = None;
        for (i, entry) in table.iter().enumerate() {
            if best.is_some_and(|(_, len)| entry.len() <= len) {
                continue;
            }
            if s.starts_with(entry) {
                best = Some((u32::try_from(i).ok()?, entry.len()));
            }
        }
        best
    }

    fn hangul(rest: &str) -> Option<u32> {
        let (l, n1) = find_syllable(rest, &L_NAMES)?;
        let rest = rest.get(n1..)?;
        let (v, n2) = find_syllable(rest, &V_NAMES)?;
        let rest = rest.get(n2..)?;
        let (t, n3) = find_syllable(rest, &T_NAMES)?;
        if rest.len() != n3 {
            return None;
        }
        S_BASE.checked_add(
            l.checked_mul(V_COUNT)?
                .checked_add(v)?
                .checked_mul(T_COUNT)?
                .checked_add(t)?,
        )
    }

    fn cjk(digits: &str) -> Option<u32> {
        if !(digits.len() == 4 || digits.len() == 5)
            || !digits
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
        {
            return None;
        }
        let v = u32::from_str_radix(digits, 16).ok()?;
        in_ranges(CJK_UNIFIED, v).then_some(v)
    }

    fn in_ranges(ranges: &[(u32, u32)], c: u32) -> bool {
        ranges
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn cpython_rules() {
            assert_eq!(lookup("EM DASH"), Some(0x2014));
            assert_eq!(lookup("em dash"), Some(0x2014));
            assert_eq!(lookup("LATIN SMALL LETTER GHA"), Some(0x1A3), "alias");
            assert_eq!(lookup("lf"), Some(0x0A), "alias, case-insensitive");
            assert_eq!(lookup("CJK UNIFIED IDEOGRAPH-4E00"), Some(0x4E00));
            assert_eq!(
                lookup("cjk unified ideograph-4E00"),
                None,
                "the prefix is case-sensitive"
            );
            assert_eq!(
                lookup("CJK UNIFIED IDEOGRAPH-4e00"),
                None,
                "hex digits are uppercase only"
            );
            assert_eq!(lookup("HANGUL SYLLABLE GA"), Some(0xAC00));
            assert_eq!(lookup("hangul syllable GA"), None);
            assert_eq!(lookup("HANGUL SYLLABLE ga"), None);
            assert_eq!(lookup("LATIN_SMALL_LETTER_A"), None, "no loose matching");
            assert_eq!(lookup(" LATIN SMALL LETTER A"), None);
            assert_eq!(lookup("LATIN SMALL LETTER-A"), None);
            assert_eq!(lookup("TANGUT IDEOGRAPH-17000"), None);
            assert_eq!(lookup("NOPE"), None);
            assert_eq!(lookup(""), None);
        }
    }
}
