//! Python `str` views of command-line arguments and small CPython text helpers shared by the parser and the
//! formatter.
//!
//! CPython decodes `sys.argv` and `os.environ` with the filesystem encoding and `surrogateescape` (POSIX) or holds
//! the UTF-16 text with lone surrogates (Windows). snp keeps the `OsString`s and works on their encoded bytes
//! (`as_encoded_bytes`: raw bytes on Unix, WTF-8 on Windows); every operation used here (`'='` splitting, prefix
//! tests against ASCII option strings, the first character of a token) gives the same answer on those bytes as
//! on the Python `str`, as long as "one Python character" is measured with [`py_char_len`].
//!
//! Owner: WP-12.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;

use snp_pycompat::chars::{decimal_value, is_space};

/// The encoded bytes of an OS string.
pub(crate) fn bytes(s: &OsStr) -> &[u8] {
    s.as_encoded_bytes()
}

/// Byte length of the Python character that starts at byte `i` of `b` (0 at the end).
///
/// A valid UTF-8 sequence is one character; on Unix every byte of an invalid sequence is one character
/// (`surrogateescape` maps each to U+DC80..U+DCFF); on Windows a WTF-8 encoded lone surrogate (`ED A0..BF xx`) is one
/// character, as it is one UTF-16 code unit in Python's `str`.
pub(crate) fn py_char_len(b: &[u8], i: usize) -> usize {
    let Some(rest) = b.get(i..) else {
        return 0;
    };
    if rest.is_empty() {
        return 0;
    }
    if cfg!(windows)
        && let [0xED, b1, b2, ..] = rest
        && (0xA0..=0xBF).contains(b1)
        && (0x80..=0xBF).contains(b2)
    {
        return 3;
    }
    // Only the first character matters: a valid one is at most 4 bytes, and a truncated sequence still classifies
    // its first byte the same way. Looking at the whole tail would make a short-option cluster O(n^2).
    let head = rest.get(..4).unwrap_or(rest);
    match head.utf8_chunks().next() {
        Some(chunk) => chunk.valid().chars().next().map_or(1, char::len_utf8),
        None => 1,
    }
}

/// `s[start:]` (by encoded byte offset `start`, which must be a [`py_char_len`] boundary) as a new `OsString`.
pub(crate) fn os_tail(s: &OsStr, start: usize) -> OsString {
    let b = bytes(s);
    let tail = b.get(start..).unwrap_or_default();
    if tail.len() == b.len() {
        return s.to_os_string();
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        OsStr::from_bytes(tail).to_os_string()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
        // Walk the UTF-16 units, counting the WTF-8 bytes each one occupies, until `start` bytes are skipped.
        let units: Vec<u16> = s.encode_wide().collect();
        let mut skipped = 0usize;
        let mut idx = 0usize;
        while skipped < start {
            let Some(&unit) = units.get(idx) else { break };
            let next_is_low = units
                .get(idx.saturating_add(1))
                .is_some_and(|&n| (0xDC00..=0xDFFF).contains(&n));
            let (width, step) = if (0xD800..=0xDBFF).contains(&unit) && next_is_low {
                (4, 2)
            } else if (0xD800..=0xDFFF).contains(&unit) || unit >= 0x800 {
                (3, 1)
            } else if unit >= 0x80 {
                (2, 1)
            } else {
                (1, 1)
            };
            skipped = skipped.saturating_add(width);
            idx = idx.saturating_add(step);
        }
        OsString::from_wide(units.get(idx..).unwrap_or_default())
    }
    #[cfg(not(any(unix, windows)))]
    {
        OsString::from(String::from_utf8_lossy(tail).into_owned())
    }
}

/// `prefix + s[start:]` for an ASCII `prefix`.
pub(crate) fn os_prefixed_tail(prefix: &str, s: &OsStr, start: usize) -> OsString {
    let mut out = OsString::from(prefix);
    out.push(os_tail(s, start));
    out
}

/// The text Python writes for a `str` holding `s` on a `backslashreplace` stream (stderr): valid text unchanged,
/// every surrogate-escaped byte as `\udcXX` (Unix) and every lone surrogate as `\udXXX` (Windows).
pub(crate) fn display(s: &OsStr) -> Cow<'_, str> {
    if let Some(t) = s.to_str() {
        return Cow::Borrowed(t);
    }
    let mut out = String::new();
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        for r in char::decode_utf16(s.encode_wide()) {
            match r {
                Ok(c) => out.push(c),
                Err(e) => {
                    write!(out, "\\u{:04x}", e.unpaired_surrogate()).unwrap_or_default();
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        for chunk in bytes(s).utf8_chunks() {
            out.push_str(chunk.valid());
            for b in chunk.invalid() {
                write!(out, "\\udc{b:02x}").unwrap_or_default();
            }
        }
    }
    Cow::Owned(out)
}

/// `len(s)` of a Python `str` (code points).
pub(crate) fn pylen(s: &str) -> usize {
    s.chars().count()
}

/// [`pylen`] as a signed width for the formatter's arithmetic.
pub(crate) fn ilen(s: &str) -> i64 {
    i64::try_from(pylen(s)).unwrap_or(i64::MAX)
}

/// `str.strip()` with Python's Unicode whitespace.
pub(crate) fn py_strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `re.sub(r'\s+', ' ', s, flags=re.ASCII)`: runs of ASCII whitespace (`[ \t\n\r\f\v]`) become one space.
pub(crate) fn collapse_ascii_ws(s: &str) -> String {
    let mut out = String::new();
    let mut in_ws = false;
    for c in s.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c') {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

/// Whitespace `int()` strips: `_PyUnicode_TransformDecimalAndSpaceToASCII` maps only code points >= 128 that are
/// `Py_UNICODE_ISSPACE` to `' '`, then `PyLong_FromString` skips `Py_ISSPACE` (`" \t\n\v\f\r"`). So, unlike
/// `str.strip()`, the ASCII separators U+001C..U+001F are not stripped.
fn is_int_space(c: char) -> bool {
    if c.is_ascii() {
        matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
    } else {
        is_space(c)
    }
}

/// Python `int(s)` (base 10) saturated to `i64`: `int()` whitespace ([`is_int_space`]) stripped at both ends, one
/// optional sign, Unicode decimal digits with single underscores only between digits, at most 4300 digits
/// (`sys.int_info.default_max_str_digits`). `None` when Python raises `ValueError`.
pub(crate) fn py_int(s: &str) -> Option<i64> {
    const MAX_STR_DIGITS: usize = 4300;
    let t = s.trim_matches(is_int_space);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let mut val: i128 = 0;
    let mut count = 0usize;
    let mut prev_digit = false;
    for c in digits.chars() {
        if c == '_' {
            if !prev_digit {
                return None;
            }
            prev_digit = false;
            continue;
        }
        let d = decimal_value(c)?;
        val = val
            .saturating_mul(10)
            .saturating_add(i128::from(d))
            .min(i128::from(i64::MAX).saturating_add(1));
        count = count.saturating_add(1);
        prev_digit = true;
    }
    if count == 0 || !prev_digit || count > MAX_STR_DIGITS {
        return None;
    }
    let signed = if neg { val.saturating_neg() } else { val };
    Some(i64::try_from(signed).unwrap_or(if neg { i64::MIN } else { i64::MAX }))
}

/// `re.match(r'^-\d+$|^-\d*\.\d+$', s)` with Unicode `\d` and `$` matching before one trailing newline
/// (`_negative_number_matcher`).
pub(crate) fn is_negative_number(b: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(b) else {
        return false;
    };
    let Some(rest) = s.strip_prefix('-') else {
        return false;
    };
    let rest = rest.strip_suffix('\n').unwrap_or(rest);
    let is_digit = |c: char| decimal_value(c).is_some();
    if !rest.is_empty() && rest.chars().all(is_digit) {
        return true;
    }
    match rest.split_once('.') {
        Some((int, frac)) => {
            int.chars().all(is_digit) && !frac.is_empty() && frac.chars().all(is_digit)
        }
        None => false,
    }
}

/// Appends `s` as a JSON string literal the way Python's `json.dumps(…, ensure_ascii=True)` writes it.
pub(crate) fn write_json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    write!(out, "\\u{unit:04x}").unwrap_or_default();
                }
            }
        }
    }
    out.push('"');
}

/// Python's `%`-interpolation of `text` with a mapping (`help % params`, `text % dict(prog=…)`), restricted to the
/// forms argparse help strings use: `%%` and `%(key)` with optional `-` flag, width and precision followed by the
/// conversions `s`, `r`, `d` or `i`. `lookup(key, repr)` returns the `str()` (or `repr()` for `r`) of the value;
/// a missing key or an unsupported specification is copied verbatim (CPython raises at help time).
pub(crate) fn percent_format(text: &str, lookup: &dyn Fn(&str, bool) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find('%') {
        out.push_str(rest.get(..i).unwrap_or_default());
        let spec = rest.get(i..).unwrap_or_default();
        if let Some((text, used)) = expand_one(spec, lookup) {
            out.push_str(&text);
            rest = spec.get(used..).unwrap_or_default();
        } else {
            out.push('%');
            rest = spec.get(1..).unwrap_or_default();
        }
    }
    out.push_str(rest);
    out
}

/// One `%…` specification at the start of `spec`: the expansion and the number of bytes it used.
fn expand_one(
    spec: &str,
    lookup: &dyn Fn(&str, bool) -> Option<String>,
) -> Option<(String, usize)> {
    let b = spec.as_bytes();
    if b.get(1) == Some(&b'%') {
        return Some(("%".to_owned(), 2));
    }
    if b.get(1) != Some(&b'(') {
        return None;
    }
    let close = spec.find(')')?;
    let key = spec.get(2..close)?;
    let mut j = close.checked_add(1)?;
    let mut left = false;
    while let Some(&f) = b.get(j) {
        match f {
            b'-' => left = true,
            b'#' | b'0' | b' ' | b'+' => {}
            _ => break,
        }
        j = j.checked_add(1)?;
    }
    let mut width = 0usize;
    while let Some(d) = b.get(j).filter(|d| d.is_ascii_digit()) {
        width = width
            .saturating_mul(10)
            .saturating_add(usize::from(d.saturating_sub(b'0')));
        j = j.checked_add(1)?;
    }
    let precision = if b.get(j) == Some(&b'.') {
        j = j.checked_add(1)?;
        let mut p = 0usize;
        while let Some(d) = b.get(j).filter(|d| d.is_ascii_digit()) {
            p = p
                .saturating_mul(10)
                .saturating_add(usize::from(d.saturating_sub(b'0')));
            j = j.checked_add(1)?;
        }
        Some(p)
    } else {
        None
    };
    let conv = *b.get(j)?;
    let mut value = match conv {
        b's' | b'd' | b'i' => lookup(key, false)?,
        b'r' => lookup(key, true)?,
        _ => return None,
    };
    if let Some(p) = precision.filter(|_| matches!(conv, b's' | b'r')) {
        value = value.chars().take(p).collect();
    }
    let pad = width.saturating_sub(pylen(&value));
    let mut text = String::new();
    if !left {
        text.extend(std::iter::repeat_n(' ', pad));
    }
    text.push_str(&value);
    if left {
        text.extend(std::iter::repeat_n(' ', pad));
    }
    Some((text, j.checked_add(1)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_int() {
        assert_eq!(py_int(" 100 "), Some(100));
        assert_eq!(py_int("1_0"), Some(10));
        assert_eq!(py_int("-1"), Some(-1));
        assert_eq!(py_int("+7"), Some(7));
        assert_eq!(py_int("\u{663}"), Some(3));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("_1"), None);
        assert_eq!(py_int("1_"), None);
        assert_eq!(py_int("1.5"), None);
        assert_eq!(py_int("0x10"), None);
        assert_eq!(py_int(""), None);
        assert_eq!(py_int("- 1"), None);
        assert_eq!(py_int("99999999999999999999999"), Some(i64::MAX));
        assert_eq!(py_int("-99999999999999999999999"), Some(i64::MIN));
        let ones = |n: usize| std::iter::repeat_n('1', n).collect::<String>();
        assert_eq!(py_int(&ones(4300)), Some(i64::MAX));
        assert_eq!(py_int(&ones(4301)), None);
        let separated: String = std::iter::repeat_n("1_", 4300).collect();
        assert_eq!(py_int(separated.trim_end_matches('_')), Some(i64::MAX));
        let zeros: String = std::iter::repeat_n('0', 4301).collect();
        assert_eq!(py_int(&zeros), None);
    }

    /// `int()` strips only `Py_ISSPACE` among ASCII characters, so the separators U+001C..U+001F that
    /// `str.strip()` removes make it raise (CPython 3.13: `int('\x1c5')` → `ValueError`).
    #[test]
    fn python_int_ascii_separators_are_not_whitespace() {
        for c in ['\x1c', '\x1d', '\x1e', '\x1f'] {
            assert_eq!(py_int(&format!("{c}5")), None, "{c:?} + '5'");
            assert_eq!(py_int(&format!("5{c}")), None, "'5' + {c:?}");
            assert_eq!(py_int(&c.to_string()), None);
            assert_eq!(
                py_strip(&format!("{c}5{c}")),
                "5",
                "str.strip() does strip {c:?}"
            );
        }
        for c in [
            ' ', '\t', '\n', '\x0b', '\x0c', '\r', '\u{85}', '\u{a0}', '\u{3000}',
        ] {
            assert_eq!(py_int(&format!("{c}5{c}")), Some(5), "{c:?}");
        }
    }

    #[test]
    fn char_len_looks_at_one_character() {
        assert_eq!(py_char_len("a\u{e9}\u{20ac}\u{1f600}".as_bytes(), 0), 1);
        assert_eq!(py_char_len("a\u{e9}\u{20ac}\u{1f600}".as_bytes(), 1), 2);
        assert_eq!(py_char_len("a\u{e9}\u{20ac}\u{1f600}".as_bytes(), 3), 3);
        assert_eq!(py_char_len("a\u{e9}\u{20ac}\u{1f600}".as_bytes(), 6), 4);
        assert_eq!(py_char_len("a\u{e9}\u{20ac}\u{1f600}".as_bytes(), 10), 0);
        // A truncated sequence and a lone continuation byte are one character per byte.
        assert_eq!(py_char_len(&[0xF0, 0x9F, 0x98], 0), 1);
        assert_eq!(py_char_len(&[0x80, b'a'], 0), 1);
        assert_eq!(py_char_len(&[0xF0, 0x9F, 0x98, 0x80, 0xFF], 0), 4);
    }

    #[test]
    fn negative_numbers() {
        for s in ["-1", "-12", "-.5", "-1.5", "-1\n", "-\u{663}"] {
            assert!(is_negative_number(s.as_bytes()), "{s:?}");
        }
        for s in [
            "-", "--1", "-1x", "-1.", "-1e5", "1", "-1\n\n", "-.", "-1.2.3",
        ] {
            assert!(!is_negative_number(s.as_bytes()), "{s:?}");
        }
    }

    #[test]
    fn char_lengths() {
        let b = "-é-".as_bytes();
        assert_eq!(py_char_len(b, 0), 1);
        assert_eq!(py_char_len(b, 1), 2);
        assert_eq!(py_char_len(b, 3), 1);
        assert_eq!(py_char_len(b, 4), 0);
        assert_eq!(py_char_len(b"-\xff", 1), 1);
        assert_eq!(py_char_len(b"-\xe2\x82", 1), 1);
    }

    #[cfg(unix)]
    #[test]
    fn os_views() {
        use std::os::unix::ffi::OsStrExt as _;
        let s = OsStr::from_bytes(b"-x\xffy");
        assert_eq!(display(s), "-x\\udcffy");
        assert_eq!(bytes(&os_tail(s, 2)), b"\xffy");
        assert_eq!(bytes(&os_prefixed_tail("-", s, 3)), b"-y");
    }

    #[test]
    fn percent() {
        let look = |k: &str, r: bool| match k {
            "default" if r => Some("'x'".to_owned()),
            "default" => Some("x".to_owned()),
            _ => None,
        };
        assert_eq!(percent_format("a %(default)s b", &look), "a x b");
        assert_eq!(percent_format("%(default)r", &look), "'x'");
        assert_eq!(percent_format("100%% %(default)-3s|", &look), "100% x  |");
        assert_eq!(percent_format("%(nope)s %d %", &look), "%(nope)s %d %");
        assert_eq!(percent_format("%(default)5s", &look), "    x");
    }

    #[test]
    fn json_strings() {
        let mut out = String::new();
        write_json_str(&mut out, "a\"\\\n\u{1}é😀");
        assert_eq!(out, "\"a\\\"\\\\\\n\\u0001\\u00e9\\ud83d\\ude00\"");
    }
}
