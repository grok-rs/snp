//! `bytes.decode()` (strict UTF-8) with CPython's exact `UnicodeDecodeError` text,
//! `errors='backslashreplace'`, the `surrogateescape` view of `os.fsdecode` and the
//! `UnicodeEncodeError` of a strict re-encode (13 X.2.3, 15 §A4.3).

use std::borrow::Cow;
use std::fmt::{self, Write as _};

use crate::chars::PyChar;

/// Why CPython's UTF-8 decoder stopped.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Utf8Reason {
    /// A byte that can never start a UTF-8 sequence.
    InvalidStartByte,
    /// A valid start byte followed by a byte that cannot continue it.
    InvalidContinuationByte,
    /// A valid but truncated sequence at the end of the input.
    UnexpectedEndOfData,
}

/// CPython's `UnicodeDecodeError` for codec `'utf-8'`. `start..end` is the byte range CPython
/// reports (the maximal invalid subpart, identical to `Utf8Error::valid_up_to`/`error_len`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Utf8DecodeError {
    /// Offset of the first undecodable byte within the whole buffer.
    pub start: usize,
    /// End (exclusive) of the invalid range.
    pub end: usize,
    /// The byte at `start`.
    pub first_byte: u8,
    /// The decoder's reason.
    pub reason: Utf8Reason,
}

impl Utf8DecodeError {
    /// The Python exception class: `UnicodeDecodeError`.
    pub const fn py_type(&self) -> &'static str {
        "UnicodeDecodeError"
    }
}

impl fmt::Display for Utf8DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.reason {
            Utf8Reason::InvalidStartByte => "invalid start byte",
            Utf8Reason::InvalidContinuationByte => "invalid continuation byte",
            Utf8Reason::UnexpectedEndOfData => "unexpected end of data",
        };
        if self.end.saturating_sub(self.start) <= 1 {
            write!(
                f,
                "'utf-8' codec can't decode byte 0x{:02x} in position {}: {reason}",
                self.first_byte, self.start
            )
        } else {
            write!(
                f,
                "'utf-8' codec can't decode bytes in position {}-{}: {reason}",
                self.start,
                self.end.saturating_sub(1)
            )
        }
    }
}

impl std::error::Error for Utf8DecodeError {}

/// `str.encode('utf-8')` of a string holding a lone surrogate (a `surrogateescape`d argv byte):
/// `'utf-8' codec can't encode character '\udcff' in position N: surrogates not allowed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Utf8EncodeError {
    /// Character position of the surrogate.
    pub position: usize,
    /// The surrogate code point.
    pub surrogate: u16,
}

impl Utf8EncodeError {
    /// The Python exception class: `UnicodeEncodeError`.
    pub const fn py_type(&self) -> &'static str {
        "UnicodeEncodeError"
    }
}

impl fmt::Display for Utf8EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'utf-8' codec can't encode character '\\u{:04x}' in position {}: surrogates not allowed",
            self.surrogate, self.position
        )
    }
}

impl std::error::Error for Utf8EncodeError {}

const fn is_start_byte(b: u8) -> bool {
    matches!(b, 0x00..=0x7f | 0xc2..=0xf4)
}

/// `b.decode()` (strict UTF-8). A BOM is kept as U+FEFF (codec `utf-8`, not `utf-8-sig`).
pub fn decode_utf8(b: &[u8]) -> Result<&str, Utf8DecodeError> {
    core::str::from_utf8(b).map_err(|e| {
        let start = e.valid_up_to();
        let first_byte = b.get(start).copied().unwrap_or(0);
        match e.error_len() {
            None => Utf8DecodeError {
                start,
                end: b.len(),
                first_byte,
                reason: Utf8Reason::UnexpectedEndOfData,
            },
            Some(n) => Utf8DecodeError {
                start,
                end: start.saturating_add(n),
                first_byte,
                reason: if n == 1 && !is_start_byte(first_byte) {
                    Utf8Reason::InvalidStartByte
                } else {
                    Utf8Reason::InvalidContinuationByte
                },
            },
        }
    })
}

/// `b.decode(errors='backslashreplace')`: every undecodable byte becomes `\xNN`.
#[must_use]
pub fn decode_utf8_backslashreplace(b: &[u8]) -> Cow<'_, str> {
    if let Ok(s) = core::str::from_utf8(b) {
        return Cow::Borrowed(s);
    }
    let mut out = String::new();
    for chunk in b.utf8_chunks() {
        out.push_str(chunk.valid());
        for x in chunk.invalid() {
            write!(out, "\\x{x:02x}").unwrap_or_default();
        }
    }
    Cow::Owned(out)
}

/// `os.fsdecode(b)` as Python characters (POSIX: UTF-8 + `surrogateescape`): every byte of an
/// undecodable range becomes the lone surrogate U+DC00 + byte.
#[derive(Clone, Debug)]
pub struct FsDecodeChars<'a> {
    chunks: core::str::Utf8Chunks<'a>,
    valid: core::str::Chars<'a>,
    invalid: core::slice::Iter<'a, u8>,
}

impl Iterator for FsDecodeChars<'_> {
    type Item = PyChar;

    fn next(&mut self) -> Option<PyChar> {
        loop {
            if let Some(c) = self.valid.next() {
                return Some(PyChar::Scalar(c));
            }
            if let Some(&b) = self.invalid.next() {
                return Some(PyChar::Surrogate(0xdc00 | u16::from(b)));
            }
            let chunk = self.chunks.next()?;
            self.valid = chunk.valid().chars();
            self.invalid = chunk.invalid().iter();
        }
    }
}

impl std::iter::FusedIterator for FsDecodeChars<'_> {}

/// `os.fsdecode(b)` viewed as Python characters (POSIX `surrogateescape`).
#[must_use]
pub fn fsdecode_chars(b: &[u8]) -> FsDecodeChars<'_> {
    FsDecodeChars {
        chunks: b.utf8_chunks(),
        valid: "".chars(),
        invalid: [].iter(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(b: &[u8]) -> String {
        decode_utf8(b).map_or_else(|e| e.to_string(), str::to_owned)
    }

    #[test]
    fn decode_errors_match_cpython() {
        assert_eq!(
            err(b"\xe2\x82"),
            "'utf-8' codec can't decode bytes in position 0-1: unexpected end of data"
        );
        assert_eq!(
            err(b"\xe2"),
            "'utf-8' codec can't decode byte 0xe2 in position 0: unexpected end of data"
        );
        assert_eq!(
            err(b"ab\x80"),
            "'utf-8' codec can't decode byte 0x80 in position 2: invalid start byte"
        );
        assert_eq!(
            err(b"\xc0\x80"),
            "'utf-8' codec can't decode byte 0xc0 in position 0: invalid start byte"
        );
        assert_eq!(
            err(b"\xed\xa0\x80"),
            "'utf-8' codec can't decode byte 0xed in position 0: invalid continuation byte"
        );
        assert_eq!(
            err(b"\xe2\x82("),
            "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte"
        );
        assert_eq!(err(b"caf\xc3\xa9"), "café");
        assert_eq!(err(b"\xef\xbb\xbfx"), "\u{feff}x");
        assert_eq!(
            decode_utf8(b"\xff").map_err(|e| e.py_type()),
            Err("UnicodeDecodeError")
        );
    }

    #[test]
    fn backslashreplace() {
        assert_eq!(
            decode_utf8_backslashreplace(b"a\xffb\xe2\x82"),
            "a\\xffb\\xe2\\x82"
        );
        assert!(matches!(
            decode_utf8_backslashreplace(b"ok"),
            Cow::Borrowed("ok")
        ));
    }

    #[test]
    fn fsdecode_view() {
        let v: Vec<PyChar> = fsdecode_chars(b"a\xffb\xc3\xa9").collect();
        assert_eq!(
            v,
            vec![
                PyChar::Scalar('a'),
                PyChar::Surrogate(0xdcff),
                PyChar::Scalar('b'),
                PyChar::Scalar('é')
            ]
        );
        assert_eq!(fsdecode_chars(b"").count(), 0);
    }

    #[test]
    fn encode_error_text() {
        let e = Utf8EncodeError {
            position: 3,
            surrogate: 0xdcff,
        };
        assert_eq!(
            e.to_string(),
            "'utf-8' codec can't encode character '\\udcff' in position 3: surrogates not allowed"
        );
        assert_eq!(e.py_type(), "UnicodeEncodeError");
    }
}
