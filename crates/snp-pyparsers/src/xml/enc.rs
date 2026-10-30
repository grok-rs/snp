//! expat's encodings (`xmltok.c`): the byte-type tables, UTF-8 validation, the UTF-16 byte types, the
//! "unknown encoding" tables pyexpat builds from Python's single-byte codecs, and the conversions to the internal
//! UTF-8 that entity replacement texts are stored in.
//!
//! Owner: WP-17.

use std::rc::Rc;

use super::generated::names;

/// expat's byte types (`xmltok_impl.h`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Bt {
    Nonxml,
    Malform,
    Lt,
    Amp,
    Rsqb,
    Lead2,
    Lead3,
    Lead4,
    Trail,
    Cr,
    Lf,
    Gt,
    Quot,
    Apos,
    Equals,
    Quest,
    Excl,
    Sol,
    Semi,
    Num,
    Lsqb,
    S,
    Nmstrt,
    Hex,
    Digit,
    Name,
    Minus,
    Other,
    Nonascii,
    Percnt,
    Lpar,
    Rpar,
    Ast,
    Plus,
    Comma,
    Verbar,
}

/// `asciitab.h` (non-namespace: `':'` is a name-start character); `internal` is `iasciitab.h` (CR is `BT_S`).
pub(super) const fn ascii_type(b: u8, internal: bool) -> Bt {
    match b {
        0x09 | 0x20 => Bt::S,
        0x0A => Bt::Lf,
        0x0D => {
            if internal {
                Bt::S
            } else {
                Bt::Cr
            }
        }
        0x00..=0x1F => Bt::Nonxml,
        b'!' => Bt::Excl,
        b'"' => Bt::Quot,
        b'#' => Bt::Num,
        b'%' => Bt::Percnt,
        b'&' => Bt::Amp,
        b'\'' => Bt::Apos,
        b'(' => Bt::Lpar,
        b')' => Bt::Rpar,
        b'*' => Bt::Ast,
        b'+' => Bt::Plus,
        b',' => Bt::Comma,
        b'-' => Bt::Minus,
        b'.' => Bt::Name,
        b'/' => Bt::Sol,
        b'0'..=b'9' => Bt::Digit,
        b':' | b'G'..=b'Z' | b'g'..=b'z' | b'_' => Bt::Nmstrt,
        b';' => Bt::Semi,
        b'<' => Bt::Lt,
        b'=' => Bt::Equals,
        b'>' => Bt::Gt,
        b'?' => Bt::Quest,
        b'A'..=b'F' | b'a'..=b'f' => Bt::Hex,
        b'[' => Bt::Lsqb,
        b']' => Bt::Rsqb,
        b'|' => Bt::Verbar,
        _ => Bt::Other,
    }
}

fn in_ranges(table: &[(u16, u16)], cp: u32) -> bool {
    let Ok(c) = u16::try_from(cp) else {
        return false;
    };
    table
        .binary_search_by(|&(a, b)| {
            if b < c {
                std::cmp::Ordering::Less
            } else if a > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `UCS2_GET_NAMING(nmstrtPages, …)` for a BMP code point ≥ U+0080.
pub(super) fn is_nmstrt_cp(cp: u32) -> bool {
    in_ranges(names::NMSTRT, cp)
}

/// `UCS2_GET_NAMING(namePages, …)`.
pub(super) fn is_name_cp(cp: u32) -> bool {
    in_ranges(names::NAME, cp)
}

/// `latin1tab.h`: the byte types of U+0080–U+00FF (also the UTF-16 types when the high byte is 0).
fn latin1_type(b: u8) -> Bt {
    if b < 0x80 {
        return ascii_type(b, false);
    }
    let cp = u32::from(b);
    if is_nmstrt_cp(cp) {
        Bt::Nmstrt
    } else if is_name_cp(cp) {
        Bt::Name
    } else {
        Bt::Other
    }
}

/// `checkCharRefNumber`.
pub(super) fn check_char_ref_number(n: i64) -> i64 {
    match n >> 8 {
        0xD8..=0xDF => -1,
        0 => {
            if u8::try_from(n).is_ok_and(|b| latin1_type(b) == Bt::Nonxml) {
                -1
            } else {
                n
            }
        }
        0xFF => {
            if n == 0xFFFE || n == 0xFFFF {
                -1
            } else {
                n
            }
        }
        _ => n,
    }
}

/// An encoding of the document entity (or the internal UTF-8 of entity replacement texts).
#[derive(Clone, Debug)]
pub(super) enum Enc {
    Utf8,
    /// `internal_utf8_encoding`: replacement texts (CR is whitespace there).
    Internal,
    Latin1,
    Ascii,
    Little2,
    Big2,
    /// An encoding of the unknown-encoding handler: byte types and the byte → code point map.
    Unknown(Rc<([Bt; 256], [i32; 256])>),
}

impl PartialEq for Enc {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unknown(a), Self::Unknown(b)) => Rc::ptr_eq(a, b),
            (Self::Utf8, Self::Utf8)
            | (Self::Internal, Self::Internal)
            | (Self::Latin1, Self::Latin1)
            | (Self::Ascii, Self::Ascii)
            | (Self::Little2, Self::Little2)
            | (Self::Big2, Self::Big2) => true,
            _ => false,
        }
    }
}

/// `unicode_byte_type(hi, lo)`.
const fn unicode_byte_type(hi: u8, lo: u8) -> Bt {
    match hi {
        0xD8..=0xDB => Bt::Lead4,
        0xDC..=0xDF => Bt::Trail,
        0xFF if lo == 0xFE || lo == 0xFF => Bt::Nonxml,
        _ => Bt::Nonascii,
    }
}

/// `XmlInitUnknownEncoding` with pyexpat's map (single-byte: −1 or a code point); `None` when expat refuses it.
pub(super) fn unknown_encoding(map: &[i32; 256]) -> Option<Enc> {
    let mut types = [Bt::Other; 256];
    for i in 0..128u8 {
        let t = latin1_type(i);
        if t != Bt::Other
            && t != Bt::Nonxml
            && map.get(usize::from(i)).copied() != Some(i32::from(i))
        {
            return None;
        }
    }
    for (i, &c) in map.iter().enumerate() {
        let t = if c == -1 {
            Bt::Malform
        } else if c < 0 {
            return None;
        } else if c < 0x80 {
            let t = u8::try_from(c).map_or(Bt::Other, latin1_type);
            if t != Bt::Other && t != Bt::Nonxml && usize::try_from(c).ok() != Some(i) {
                return None;
            }
            t
        } else if check_char_ref_number(i64::from(c)) < 0 {
            Bt::Nonxml
        } else if c > 0xFFFF {
            return None;
        } else {
            let cp = u32::try_from(c).unwrap_or(0);
            if is_nmstrt_cp(cp) {
                Bt::Nmstrt
            } else if is_name_cp(cp) {
                Bt::Name
            } else {
                Bt::Other
            }
        };
        if let Some(slot) = types.get_mut(i) {
            *slot = t;
        }
    }
    Some(Enc::Unknown(Rc::new((types, *map))))
}

/// `UTF8_INVALID2/3/4`.
fn utf8_invalid(p: &[u8]) -> bool {
    let g = |i: usize| p.get(i).copied().unwrap_or(0);
    let (p0, p1, p2, p3) = (g(0), g(1), g(2), g(3));
    match p.len() {
        2 => p0 < 0xC2 || (p1 & 0x80) == 0 || (p1 & 0xC0) == 0xC0,
        3 => {
            (p2 & 0x80) == 0
                || (if p0 == 0xEF && p1 == 0xBF {
                    p2 > 0xBD
                } else {
                    (p2 & 0xC0) == 0xC0
                })
                || (if p0 == 0xE0 {
                    p1 < 0xA0 || (p1 & 0xC0) == 0xC0
                } else {
                    (p1 & 0x80) == 0
                        || (if p0 == 0xED {
                            p1 > 0x9F
                        } else {
                            (p1 & 0xC0) == 0xC0
                        })
                })
        }
        _ => {
            (p3 & 0x80) == 0
                || (p3 & 0xC0) == 0xC0
                || (p2 & 0x80) == 0
                || (p2 & 0xC0) == 0xC0
                || (if p0 == 0xF0 {
                    p1 < 0x90 || (p1 & 0xC0) == 0xC0
                } else {
                    (p1 & 0x80) == 0
                        || (if p0 == 0xF4 {
                            p1 > 0x8F
                        } else {
                            (p1 & 0xC0) == 0xC0
                        })
                })
        }
    }
}

/// The code point of a valid 2/3-byte UTF-8 sequence.
fn utf8_cp(p: &[u8]) -> u32 {
    let g = |i: usize| u32::from(p.get(i).copied().unwrap_or(0));
    match p.len() {
        2 => ((g(0) & 0x1F) << 6) | (g(1) & 0x3F),
        3 => ((g(0) & 0x0F) << 12) | ((g(1) & 0x3F) << 6) | (g(2) & 0x3F),
        _ => ((g(0) & 0x07) << 18) | ((g(1) & 0x3F) << 12) | ((g(2) & 0x3F) << 6) | (g(3) & 0x3F),
    }
}

impl Enc {
    /// `MINBPC(enc)`.
    pub(super) const fn minbpc(&self) -> usize {
        match self {
            Self::Little2 | Self::Big2 => 2,
            _ => 1,
        }
    }

    /// `BYTE_TYPE(enc, p)`; `buf[p..]` holds at least `minbpc` bytes.
    pub(super) fn bt(&self, buf: &[u8], p: usize) -> Bt {
        let b0 = buf.get(p).copied().unwrap_or(0);
        match self {
            Self::Utf8 | Self::Internal => {
                if b0 < 0x80 {
                    ascii_type(b0, matches!(self, Self::Internal))
                } else {
                    match b0 {
                        0x80..=0xBF => Bt::Trail,
                        0xC0..=0xDF => Bt::Lead2,
                        0xE0..=0xEF => Bt::Lead3,
                        0xF0..=0xF4 => Bt::Lead4,
                        _ => Bt::Nonxml,
                    }
                }
            }
            Self::Latin1 => latin1_type(b0),
            Self::Ascii => {
                if b0 < 0x80 {
                    ascii_type(b0, false)
                } else {
                    Bt::Nonxml
                }
            }
            Self::Little2 | Self::Big2 => {
                let b1 = buf.get(p.saturating_add(1)).copied().unwrap_or(0);
                let (hi, lo) = if matches!(self, Self::Little2) {
                    (b1, b0)
                } else {
                    (b0, b1)
                };
                if hi == 0 {
                    latin1_type(lo)
                } else {
                    unicode_byte_type(hi, lo)
                }
            }
            Self::Unknown(t) => t.0.get(usize::from(b0)).copied().unwrap_or(Bt::Malform),
        }
    }

    /// The 16-bit unit at `p` of a UTF-16 encoding (or the byte of an 8-bit one).
    fn unit(&self, buf: &[u8], p: usize) -> u32 {
        let b0 = u32::from(buf.get(p).copied().unwrap_or(0));
        let b1 = u32::from(buf.get(p.saturating_add(1)).copied().unwrap_or(0));
        match self {
            Self::Little2 => (b1 << 8) | b0,
            Self::Big2 => (b0 << 8) | b1,
            _ => b0,
        }
    }

    /// `IS_INVALID_CHAR(enc, p, n)` for a lead byte of an `n`-byte sequence.
    pub(super) fn is_invalid_char(&self, buf: &[u8], p: usize, n: usize) -> bool {
        match self {
            Self::Utf8 | Self::Internal => {
                utf8_invalid(buf.get(p..p.saturating_add(n)).unwrap_or_default())
            }
            _ => false,
        }
    }

    /// `IS_NAME_CHAR(enc, p, n)` (multi-byte sequences; never for 4-byte ones).
    pub(super) fn is_name_char(&self, buf: &[u8], p: usize, n: usize) -> bool {
        match self {
            Self::Utf8 | Self::Internal if n < 4 => {
                is_name_cp(utf8_cp(buf.get(p..p.saturating_add(n)).unwrap_or_default()))
            }
            _ => false,
        }
    }

    /// `IS_NMSTRT_CHAR(enc, p, n)`.
    pub(super) fn is_nmstrt_char(&self, buf: &[u8], p: usize, n: usize) -> bool {
        match self {
            Self::Utf8 | Self::Internal if n < 4 => {
                is_nmstrt_cp(utf8_cp(buf.get(p..p.saturating_add(n)).unwrap_or_default()))
            }
            _ => false,
        }
    }

    /// `IS_NAME_CHAR_MINBPC(enc, p)` for `BT_NONASCII` (UTF-16).
    pub(super) fn is_name_char_minbpc(&self, buf: &[u8], p: usize) -> bool {
        is_name_cp(self.unit(buf, p))
    }

    /// `IS_NMSTRT_CHAR_MINBPC(enc, p)`.
    pub(super) fn is_nmstrt_char_minbpc(&self, buf: &[u8], p: usize) -> bool {
        is_nmstrt_cp(self.unit(buf, p))
    }

    /// The ASCII byte of the character at `p` (`BYTE_TO_ASCII`), 0 when it is not ASCII.
    pub(super) fn ascii_at(&self, buf: &[u8], p: usize) -> u8 {
        match self {
            Self::Little2 | Self::Big2 => u8::try_from(self.unit(buf, p))
                .ok()
                .filter(u8::is_ascii)
                .unwrap_or(0),
            Self::Unknown(t) => {
                t.1.get(usize::from(buf.get(p).copied().unwrap_or(0)))
                    .and_then(|&c| u8::try_from(c).ok())
                    .filter(u8::is_ascii)
                    .unwrap_or(0)
            }
            _ => buf.get(p).copied().filter(u8::is_ascii).unwrap_or(0),
        }
    }

    /// `CHAR_MATCHES(enc, p, c)`.
    pub(super) fn char_matches(&self, buf: &[u8], p: usize, c: u8) -> bool {
        match self {
            Self::Little2 | Self::Big2 => self.unit(buf, p) == u32::from(c),
            _ => buf.get(p).copied() == Some(c),
        }
    }

    /// `toAscii(enc, ptr, end)`: the ASCII value of one character, −1 otherwise.
    pub(super) fn to_ascii(&self, buf: &[u8], p: usize, end: usize) -> i32 {
        if p >= end {
            return -1;
        }
        match self {
            Self::Little2 | Self::Big2 => {
                let u = self.unit(buf, p);
                if u < 0x80 {
                    i32::try_from(u).unwrap_or(-1)
                } else {
                    -1
                }
            }
            Self::Unknown(t) => {
                let c =
                    t.1.get(usize::from(buf.get(p).copied().unwrap_or(0)))
                        .copied()
                        .unwrap_or(-1);
                if (0..0x80).contains(&c) { c } else { -1 }
            }
            _ => {
                let b = buf.get(p).copied().unwrap_or(0);
                if b < 0x80 { i32::from(b) } else { -1 }
            }
        }
    }

    /// `XmlUtf8Convert`: the characters of `buf[start..end]` in UTF-8 (code points as expat converts them).
    pub(super) fn to_utf8(&self, buf: &[u8], start: usize, end: usize) -> Vec<u8> {
        let s = buf.get(start..end.min(buf.len())).unwrap_or_default();
        match self {
            Self::Utf8 | Self::Internal | Self::Ascii => s.to_vec(),
            Self::Latin1 => {
                let mut out = Vec::new();
                for &b in s {
                    crate::pystr::push_cp(&mut out, u32::from(b));
                }
                out
            }
            Self::Unknown(t) => {
                let mut out = Vec::new();
                for &b in s {
                    let c = t.1.get(usize::from(b)).copied().unwrap_or(-1);
                    crate::pystr::push_cp(&mut out, u32::try_from(c).unwrap_or(0xFFFD));
                }
                out
            }
            Self::Little2 | Self::Big2 => {
                let mut out = Vec::new();
                let mut i = start;
                while i.saturating_add(1) < end.min(buf.len()).saturating_add(1) && i < end {
                    let u = self.unit(buf, i);
                    i = i.saturating_add(2);
                    if (0xD800..0xDC00).contains(&u) && i < end {
                        let lo = self.unit(buf, i);
                        if (0xDC00..0xE000).contains(&lo) {
                            i = i.saturating_add(2);
                            crate::pystr::push_cp(
                                &mut out,
                                0x1_0000u32
                                    .saturating_add(u.saturating_sub(0xD800) << 10)
                                    .saturating_add(lo.saturating_sub(0xDC00)),
                            );
                            continue;
                        }
                    }
                    crate::pystr::push_cp(&mut out, u);
                }
                out
            }
        }
    }
}

/// expat's built-in encodings by name (`getEncodingIndex`, ASCII case-insensitive).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Builtin {
    Latin1,
    Ascii,
    Utf8,
    Utf16,
    Utf16Be,
    Utf16Le,
}

pub(super) fn builtin(name: &[u8]) -> Option<Builtin> {
    let eq = |k: &str| name.eq_ignore_ascii_case(k.as_bytes());
    if eq("ISO-8859-1") {
        Some(Builtin::Latin1)
    } else if eq("US-ASCII") {
        Some(Builtin::Ascii)
    } else if eq("UTF-8") {
        Some(Builtin::Utf8)
    } else if eq("UTF-16") {
        Some(Builtin::Utf16)
    } else if eq("UTF-16BE") {
        Some(Builtin::Utf16Be)
    } else if eq("UTF-16LE") {
        Some(Builtin::Utf16Le)
    } else {
        None
    }
}
