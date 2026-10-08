//! Python `str` values that may hold lone surrogates, as "generalized UTF-8" bytes: every code point (surrogates
//! included) is encoded on its own with the UTF-8 scheme, so byte order equals code-point order and byte equality
//! equals `str` equality. Plus the codecs CPython's `json.loads(bytes)` uses (`surrogatepass`) with their exact
//! `UnicodeDecodeError` texts, and the `UnicodeEncodeError` text of printing a lone surrogate.
//!
//! Owner: WP-17.

use std::borrow::Cow;

/// Appends code point `cp` (≤ 0x10FFFF, surrogates allowed) in generalized UTF-8.
pub(crate) fn push_cp(buf: &mut Vec<u8>, cp: u32) {
    // Each byte below is masked into range, so the narrowing casts are exact.
    let b = |x: u32| u8::try_from(x & 0xFF).unwrap_or(0);
    if cp < 0x80 {
        buf.push(b(cp));
    } else if cp < 0x800 {
        buf.extend_from_slice(&[b(0xC0 | (cp >> 6)), b(0x80 | (cp & 0x3F))]);
    } else if cp < 0x1_0000 {
        buf.extend_from_slice(&[
            b(0xE0 | (cp >> 12)),
            b(0x80 | ((cp >> 6) & 0x3F)),
            b(0x80 | (cp & 0x3F)),
        ]);
    } else {
        buf.extend_from_slice(&[
            b(0xF0 | (cp >> 18)),
            b(0x80 | ((cp >> 12) & 0x3F)),
            b(0x80 | ((cp >> 6) & 0x3F)),
            b(0x80 | (cp & 0x3F)),
        ]);
    }
}

/// The number of code points in generalized UTF-8 `bytes` (bytes that are not continuation bytes).
pub(crate) fn char_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b & 0xC0 != 0x80).count()
}

/// The code points of generalized UTF-8 `bytes` (well-formed input assumed; stray bytes decode as themselves).
pub(crate) fn code_points(bytes: &[u8]) -> CodePoints<'_> {
    CodePoints { bytes, pos: 0 }
}

/// Iterator of [`code_points`].
#[derive(Debug, Clone)]
pub(crate) struct CodePoints<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Iterator for CodePoints<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let lead = *self.bytes.get(self.pos)?;
        let (len, init) = match lead {
            0xC0..=0xDF => (2, u32::from(lead & 0x1F)),
            0xE0..=0xEF => (3, u32::from(lead & 0x0F)),
            0xF0..=0xF7 => (4, u32::from(lead & 0x07)),
            _ => (1, u32::from(lead)),
        };
        let mut cp = init;
        let mut used = 1usize;
        while used < len {
            match self.bytes.get(self.pos.saturating_add(used)) {
                Some(&c) if c & 0xC0 == 0x80 => {
                    cp = (cp << 6) | u32::from(c & 0x3F);
                    used = used.saturating_add(1);
                }
                _ => break,
            }
        }
        self.pos = self.pos.saturating_add(used);
        Some(cp)
    }
}

/// Whether `cp` is a UTF-16 surrogate.
pub(crate) const fn is_surrogate(cp: u32) -> bool {
    matches!(cp, 0xD800..=0xDFFF)
}

/// `bytes` as a Rust string: lone surrogates become U+FFFD (one per code point).
pub(crate) fn to_string_lossy(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => code_points(bytes)
            .map(|cp| char::from_u32(cp).unwrap_or('\u{FFFD}'))
            .collect(),
    }
}

/// The first run of lone surrogates as (start, end) code-point positions, if any.
pub(crate) fn surrogate_run(bytes: &[u8]) -> Option<(usize, usize, u32)> {
    if std::str::from_utf8(bytes).is_ok() {
        return None;
    }
    let mut start = None;
    let mut first = 0;
    for (i, cp) in code_points(bytes).enumerate() {
        match (start, is_surrogate(cp)) {
            (None, true) => {
                start = Some(i);
                first = cp;
            }
            (Some(s), false) => return Some((s, i, first)),
            _ => {}
        }
    }
    start.map(|s| (s, char_count(bytes), first))
}

/// `str.encode('utf-8')` of a string with a lone surrogate (`print()` to a UTF-8 stream): CPython's
/// `UnicodeEncodeError` text, `None` when the string encodes. `offset` is added to the positions (the length of
/// whatever is printed before the string on the same `write`).
pub(crate) fn encode_error(bytes: &[u8], offset: usize) -> Option<String> {
    let (start, end, first) = surrogate_run(bytes)?;
    let (s, e) = (start.saturating_add(offset), end.saturating_add(offset));
    Some(if end.saturating_sub(start) == 1 {
        format!(
            "'utf-8' codec can't encode character '\\u{first:04x}' in position {s}: surrogates not allowed"
        )
    } else {
        format!(
            "'utf-8' codec can't encode characters in position {s}-{}: surrogates not allowed",
            e.saturating_sub(1)
        )
    })
}

/// A `UnicodeDecodeError` as CPython words it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodeError {
    /// The codec name in the message (`utf-8`, `utf-16-le`, …).
    pub(crate) codec: &'static str,
    /// The undecodable bytes (positions in the codec's input).
    pub(crate) start: usize,
    /// End of the undecodable range (exclusive).
    pub(crate) end: usize,
    /// The byte at `start`.
    pub(crate) byte: u8,
    /// `invalid start byte`, `truncated data`, …
    pub(crate) reason: &'static str,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.end.saturating_sub(self.start) <= 1 {
            write!(
                f,
                "'{}' codec can't decode byte 0x{:02x} in position {}: {}",
                self.codec, self.byte, self.start, self.reason
            )
        } else {
            write!(
                f,
                "'{}' codec can't decode bytes in position {}-{}: {}",
                self.codec,
                self.start,
                self.end.saturating_sub(1),
                self.reason
            )
        }
    }
}

/// `bytes.decode('utf-8', 'surrogatepass')`: encoded surrogates (`ED A0..BF 80..BF`) are accepted; any other error
/// is the strict decoder's (maximal-subpart ranges). The result is generalized UTF-8 (borrowed when unchanged).
pub(crate) fn decode_utf8_surrogatepass(b: &[u8]) -> Result<Cow<'_, [u8]>, DecodeError> {
    let mut pos = 0usize;
    loop {
        let rest = b.get(pos..).unwrap_or_default();
        let Err(e) = std::str::from_utf8(rest) else {
            return Ok(Cow::Borrowed(b));
        };
        let at = pos.saturating_add(e.valid_up_to());
        let tail = b.get(at..).unwrap_or_default();
        if let [0xED, 0xA0..=0xBF, 0x80..=0xBF, ..] = tail {
            pos = at.saturating_add(3);
            continue;
        }
        let byte = tail.first().copied().unwrap_or(0);
        return Err(match e.error_len() {
            None => DecodeError {
                codec: "utf-8",
                start: at,
                end: b.len(),
                byte,
                reason: "unexpected end of data",
            },
            Some(n) => DecodeError {
                codec: "utf-8",
                start: at,
                end: at.saturating_add(n),
                byte,
                reason: if n == 1 && !matches!(byte, 0x00..=0x7F | 0xC2..=0xF4) {
                    "invalid start byte"
                } else {
                    "invalid continuation byte"
                },
            },
        });
    }
}

/// `bytes.decode('utf-16-le'|'utf-16-be', 'surrogatepass')` from byte `offset` (a BOM already skipped by the
/// caller; positions stay those of the whole input). Lone surrogates pass, pairs join, an odd trailing byte is
/// `truncated data`.
pub(crate) fn decode_utf16_surrogatepass(
    b: &[u8],
    offset: usize,
    big_endian: bool,
) -> Result<Vec<u8>, DecodeError> {
    let codec = if big_endian { "utf-16-be" } else { "utf-16-le" };
    let data = b.get(offset..).unwrap_or_default();
    let mut out = Vec::new();
    let unit = |i: usize| -> Option<u32> {
        let (x, y) = (*data.get(i)?, *data.get(i.checked_add(1)?)?);
        Some(u32::from(if big_endian {
            u16::from_be_bytes([x, y])
        } else {
            u16::from_le_bytes([x, y])
        }))
    };
    let mut i = 0usize;
    while i < data.len() {
        let Some(u) = unit(i) else {
            let start = offset.saturating_add(i);
            return Err(DecodeError {
                codec,
                start,
                end: b.len(),
                byte: data.get(i).copied().unwrap_or(0),
                reason: "truncated data",
            });
        };
        i = i.saturating_add(2);
        if let (0xD800..=0xDBFF, Some(lo @ 0xDC00..=0xDFFF)) = (u, unit(i)) {
            push_cp(&mut out, join_surrogates(u, lo));
            i = i.saturating_add(2);
            continue;
        }
        push_cp(&mut out, u);
    }
    Ok(out)
}

/// `bytes.decode('utf-32-le'|'utf-32-be', 'surrogatepass')` from byte `offset`: surrogate code points pass, values
/// above U+10FFFF and a partial trailing unit are errors.
pub(crate) fn decode_utf32_surrogatepass(
    input: &[u8],
    offset: usize,
    big_endian: bool,
) -> Result<Vec<u8>, DecodeError> {
    let codec = if big_endian { "utf-32-be" } else { "utf-32-le" };
    let data = input.get(offset..).unwrap_or_default();
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let start = offset.saturating_add(pos);
        let Some(unit) = data
            .get(pos..pos.saturating_add(4))
            .and_then(|s| <[u8; 4]>::try_from(s).ok())
        else {
            return Err(DecodeError {
                codec,
                start,
                end: input.len(),
                byte: data.get(pos).copied().unwrap_or(0),
                reason: "truncated data",
            });
        };
        let cp = if big_endian {
            u32::from_be_bytes(unit)
        } else {
            u32::from_le_bytes(unit)
        };
        if cp > 0x10_FFFF {
            return Err(DecodeError {
                codec,
                start,
                end: start.saturating_add(4),
                byte: unit[0],
                reason: "code point not in range(0x110000)",
            });
        }
        push_cp(&mut out, cp);
        pos = pos.saturating_add(4);
    }
    Ok(out)
}

/// The code point of a UTF-16 surrogate pair (`hi` in 0xD800..=0xDBFF, `lo` in 0xDC00..=0xDFFF).
pub(crate) const fn join_surrogates(hi: u32, lo: u32) -> u32 {
    0x1_0000_u32
        .wrapping_add((hi & 0x3FF) << 10)
        .wrapping_add(lo & 0x3FF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generalized_utf8_round_trip() {
        let mut b = Vec::new();
        for cp in [0x41, 0xE9, 0x20AC, 0xD800, 0xDFFF, 0x1F600, 0x10_FFFF] {
            push_cp(&mut b, cp);
        }
        assert_eq!(
            code_points(&b).collect::<Vec<_>>(),
            [0x41, 0xE9, 0x20AC, 0xD800, 0xDFFF, 0x1F600, 0x10_FFFF]
        );
        assert_eq!(char_count(&b), 7);
        assert_eq!(to_string_lossy(&b), "Aé€\u{FFFD}\u{FFFD}😀\u{10FFFF}");
        assert_eq!(surrogate_run(&b), Some((3, 5, 0xD800)));
        assert_eq!(
            encode_error(&b, 2).as_deref(),
            Some("'utf-8' codec can't encode characters in position 5-6: surrogates not allowed")
        );
    }

    #[test]
    fn utf8_surrogatepass() {
        assert_eq!(
            decode_utf8_surrogatepass(b"a\xed\xa0\x80")
                .unwrap()
                .as_ref(),
            b"a\xed\xa0\x80"
        );
        let e = decode_utf8_surrogatepass(b"a\xed\xa0").unwrap_err();
        assert_eq!(
            e.to_string(),
            "'utf-8' codec can't decode byte 0xed in position 1: invalid continuation byte"
        );
        let e = decode_utf8_surrogatepass(b"\xf0\x90\x80").unwrap_err();
        assert_eq!(
            e.to_string(),
            "'utf-8' codec can't decode bytes in position 0-2: unexpected end of data"
        );
        let e = decode_utf8_surrogatepass(b"\xff").unwrap_err();
        assert_eq!(
            e.to_string(),
            "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"
        );
    }

    #[test]
    fn utf16_utf32() {
        assert_eq!(
            decode_utf16_surrogatepass(b"\x00\xd8\x00\xdc", 0, false).unwrap(),
            "\u{10000}".as_bytes()
        );
        let e = decode_utf16_surrogatepass(b"\xff\xfea\x00b", 2, false).unwrap_err();
        assert_eq!(
            e.to_string(),
            "'utf-16-le' codec can't decode byte 0x62 in position 4: truncated data"
        );
        let e = decode_utf32_surrogatepass(b"a\x00\x00\x00b\x00\x00", 0, false).unwrap_err();
        assert_eq!(
            e.to_string(),
            "'utf-32-le' codec can't decode bytes in position 4-6: truncated data"
        );
        let e = decode_utf32_surrogatepass(b"\x00\x11\x00\x00", 0, true).unwrap_err();
        assert_eq!(
            e.to_string(),
            "'utf-32-be' codec can't decode bytes in position 0-3: code point not in range(0x110000)"
        );
    }
}
