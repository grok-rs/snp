//! Byte and text helpers of the harness: base64 (`!!binary`), `\xNN` escapes of byte file names, POSIX shell
//! quoting for `repro.sh`, YAML double-quoted scalars for emitted scenarios, line splitting.
//!
//! Owner: WP-35.

use std::fmt::Write as _;

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_value(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some(u32::from(c.wrapping_sub(b'A'))),
        b'a'..=b'z' => Some(u32::from(c.wrapping_sub(b'a')).wrapping_add(26)),
        b'0'..=b'9' => Some(u32::from(c.wrapping_sub(b'0')).wrapping_add(52)),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decodes base64 (whitespace ignored, `=` padding optional), as PyYAML's `!!binary` constructor accepts it.
pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for c in s.bytes() {
        if c.is_ascii_whitespace() || c == b'=' {
            continue;
        }
        let v = b64_value(c)?;
        acc = (acc << 6) | v;
        bits = bits.saturating_add(6);
        if bits >= 8 {
            bits = bits.saturating_sub(8);
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
        acc &= (1_u32 << bits).wrapping_sub(1);
    }
    Some(out)
}

/// Encodes base64 with padding.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut it = bytes.iter();
    while let Some(&a) = it.next() {
        let b = it.next().copied();
        let c = it.next().copied();
        let n = (u32::from(a) << 16) | (u32::from(b.unwrap_or(0)) << 8) | u32::from(c.unwrap_or(0));
        let sym = |shift: u32| {
            let idx = usize::try_from((n >> shift) & 63).unwrap_or(0);
            char::from(B64.get(idx).copied().unwrap_or(b'A'))
        };
        out.push(sym(18));
        out.push(sym(12));
        out.push(if b.is_some() { sym(6) } else { '=' });
        out.push(if c.is_some() { sym(0) } else { '=' });
    }
    out
}

/// Expands `\xNN` escapes (and `\\`) of a byte file name written in YAML (`files_bytes`, `commit` entries): the
/// harness's representation of names that are not valid UTF-8.
pub(crate) fn expand_byte_escapes(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0_usize;
    while let Some(&c) = b.get(i) {
        if c == b'\\' {
            if b.get(i.saturating_add(1)) == Some(&b'x') {
                let hex = b.get(i.saturating_add(2)..i.saturating_add(4));
                if let Some(v) = hex
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    out.push(v);
                    i = i.saturating_add(4);
                    continue;
                }
            } else if b.get(i.saturating_add(1)) == Some(&b'\\') {
                out.push(b'\\');
                i = i.saturating_add(2);
                continue;
            }
        }
        out.push(c);
        i = i.saturating_add(1);
    }
    out
}

/// POSIX shell single-quoting (`shlex.quote`).
pub(crate) fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"@%+=:,./-_".contains(&c))
    {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// `printf '%b'`-style quoting of arbitrary bytes for `repro.sh` (`$'…'` is not POSIX).
pub(crate) fn sh_quote_bytes(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) if !s.contains('\\') => sh_quote(s),
        _ => {
            let mut out = String::from("\"$(printf '");
            for &c in b {
                write!(out, "\\{c:03o}").unwrap_or_default();
            }
            out.push_str("')\"");
            out
        }
    }
}

/// A YAML double-quoted scalar for `s` (valid YAML 1.1; escapes everything outside printable ASCII except
/// printable non-ASCII text).
pub(crate) fn yaml_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c if u32::from(c) < 0x20 || u32::from(c) == 0x7f => {
                write!(out, "\\x{:02x}", u32::from(c)).unwrap_or_default();
            }
            c if ('\u{80}'..='\u{9f}').contains(&c) || c == '\u{feff}' => {
                write!(out, "\\u{:04x}", u32::from(c)).unwrap_or_default();
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A YAML scalar for file contents: a double-quoted string when UTF-8, else `!!binary`.
pub(crate) fn yaml_bytes(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => yaml_quote(s),
        Err(_) => format!("!!binary {}", base64_encode(b)),
    }
}

/// Splits into lines keeping their terminators (`str.splitlines(keepends=True)` restricted to `\n`).
pub(crate) fn lines_with_ends(b: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut rest = b;
    while !rest.is_empty() {
        if let Some(p) = rest.iter().position(|&c| c == b'\n') {
            let (line, tail) = rest
                .split_at_checked(p.saturating_add(1))
                .unwrap_or((rest, &[]));
            out.push(line);
            rest = tail;
        } else {
            out.push(rest);
            rest = &[];
        }
    }
    out
}

/// Replaces every occurrence of `from` in `hay` (bytes).
pub(crate) fn replace_bytes(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return hay.to_vec();
    }
    let mut out = Vec::new();
    let mut i = 0_usize;
    while let Some(rest) = hay.get(i..) {
        if rest.is_empty() {
            break;
        }
        if rest.starts_with(from) {
            out.extend_from_slice(to);
            i = i.saturating_add(from.len());
        } else {
            out.extend_from_slice(rest.get(..1).unwrap_or_default());
            i = i.saturating_add(1);
        }
    }
    out
}

/// Whether `hay` contains `needle`.
pub(crate) fn contains_bytes(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || find_bytes(hay, needle).is_some()
}

/// The first index of `needle` in `hay`.
pub(crate) fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let last = hay.len().checked_sub(needle.len())?;
    (0..=last).find(|&i| hay.get(i..).is_some_and(|r| r.starts_with(needle)))
}

/// Lossy display of bytes for reports.
pub(crate) fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// FNV-1a 64 of `b`: stable directory names for scenario roots (not security relevant).
pub(crate) fn fnv64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in b {
        h ^= u64::from(c);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        for s in [&b""[..], b"h", b"hi", b"hi!", b"\xff\x00\x10binary"] {
            assert_eq!(base64_decode(&base64_encode(s)).unwrap(), s);
        }
        assert_eq!(base64_decode("aGk=\n").unwrap(), b"hi");
        assert!(base64_decode("a*").is_none());
    }

    #[test]
    fn byte_escapes() {
        assert_eq!(expand_byte_escapes(r"bad\xff.txt"), b"bad\xff.txt");
        assert_eq!(expand_byte_escapes(r"a\\xff"), b"a\\xff");
        assert_eq!(expand_byte_escapes(r"a\xg0"), b"a\\xg0");
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("abc/d.txt"), "abc/d.txt");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote_bytes(b"\xff"), "\"$(printf '\\377')\"");
        assert_eq!(yaml_quote("a\"b\\\n\u{1}"), "\"a\\\"b\\\\\\n\\x01\"");
        assert_eq!(yaml_bytes(b"\xff"), "!!binary /w==");
    }

    #[test]
    fn line_helpers() {
        assert_eq!(lines_with_ends(b"a\nb"), vec![&b"a\n"[..], b"b"]);
        assert_eq!(lines_with_ends(b""), Vec::<&[u8]>::new());
        assert_eq!(replace_bytes(b"aXbXc", b"X", b"YY"), b"aYYbYYc");
        assert!(contains_bytes(b"hello", b"ell"));
        assert_eq!(find_bytes(b"hello", b"lo"), Some(3));
        assert_eq!(find_bytes(b"he", b"hello"), None);
        assert_ne!(fnv64(b"a"), fnv64(b"b"));
    }
}
