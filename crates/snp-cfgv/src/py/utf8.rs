//! Strict UTF-8 decoding with CPython's error text.

use std::fmt;

/// Why CPython's UTF-8 decoder rejected the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Utf8ErrorReason {
    /// A byte that can never start a UTF-8 sequence (`0x80..=0xC1`,
    /// `0xF5..=0xFF`).
    InvalidStartByte,
    /// A valid lead byte followed by an invalid continuation byte (this also
    /// covers overlong encodings, surrogates and code points above
    /// U+10FFFF).
    InvalidContinuationByte,
    /// The input ended in the middle of an otherwise valid sequence.
    UnexpectedEndOfData,
}

impl Utf8ErrorReason {
    /// The reason text CPython uses (`"invalid start byte"`, ...).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidStartByte => "invalid start byte",
            Self::InvalidContinuationByte => "invalid continuation byte",
            Self::UnexpectedEndOfData => "unexpected end of data",
        }
    }
}

impl fmt::Display for Utf8ErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A UTF-8 decoding failure, displayed exactly like CPython's
/// `str(UnicodeDecodeError)` for the `'utf-8'` codec.
///
/// The reported range is CPython's: it starts at the first invalid byte and
/// covers the maximal invalid subpart, which coincides with Rust's
/// [`std::str::Utf8Error`].
///
/// ```
/// let err = cfgv::py::decode_utf8(b"ok\xff").unwrap_err();
/// assert_eq!(
///     err.to_string(),
///     "'utf-8' codec can't decode byte 0xff in position 2: invalid start byte",
/// );
/// let err = cfgv::py::decode_utf8(b"\xe2\x82").unwrap_err();
/// assert_eq!(
///     err.to_string(),
///     "'utf-8' codec can't decode bytes in position 0-1: unexpected end of data",
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Utf8DecodeError {
    start: usize,
    end: usize,
    first_byte: u8,
    reason: Utf8ErrorReason,
}

impl Utf8DecodeError {
    /// Byte offset of the first byte that could not be decoded.
    #[must_use]
    pub const fn start(&self) -> usize {
        self.start
    }

    /// Byte offset one past the end of the undecodable range.
    #[must_use]
    pub const fn end(&self) -> usize {
        self.end
    }

    /// The first undecodable byte.
    #[must_use]
    pub const fn first_byte(&self) -> u8 {
        self.first_byte
    }

    /// Why decoding failed.
    #[must_use]
    pub const fn reason(&self) -> Utf8ErrorReason {
        self.reason
    }
}

impl fmt::Display for Utf8DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.end.saturating_sub(self.start) == 1 {
            write!(
                f,
                "'utf-8' codec can't decode byte 0x{:02x} in position {}: {}",
                self.first_byte, self.start, self.reason
            )
        } else {
            write!(
                f,
                "'utf-8' codec can't decode bytes in position {}-{}: {}",
                self.start,
                self.end.saturating_sub(1),
                self.reason
            )
        }
    }
}

impl std::error::Error for Utf8DecodeError {}

/// Decodes `bytes` as strict UTF-8 like CPython's `bytes.decode('utf-8')`.
///
/// A leading byte order mark is kept as U+FEFF, as with Python's `'utf-8'`
/// codec.
///
/// # Errors
///
/// Returns a [`Utf8DecodeError`] describing the first invalid sequence.
pub fn decode_utf8(bytes: &[u8]) -> Result<&str, Utf8DecodeError> {
    std::str::from_utf8(bytes).map_err(|e| {
        let start = e.valid_up_to();
        let first_byte = bytes.get(start).copied().unwrap_or(0);
        match e.error_len() {
            None => Utf8DecodeError {
                start,
                end: bytes.len(),
                first_byte,
                reason: Utf8ErrorReason::UnexpectedEndOfData,
            },
            Some(len) => Utf8DecodeError {
                start,
                end: start.saturating_add(len),
                first_byte,
                reason: if len == 1 && !is_lead_byte(first_byte) {
                    Utf8ErrorReason::InvalidStartByte
                } else {
                    Utf8ErrorReason::InvalidContinuationByte
                },
            },
        }
    })
}

const fn is_lead_byte(b: u8) -> bool {
    matches!(b, 0x00..=0x7f | 0xc2..=0xf4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(b: &[u8]) -> String {
        decode_utf8(b).map_or_else(|e| e.to_string(), str::to_owned)
    }

    #[test]
    fn messages() {
        assert_eq!(msg(b"ok"), "ok");
        assert_eq!(
            msg(b"\x80"),
            "'utf-8' codec can't decode byte 0x80 in position 0: invalid start byte"
        );
        assert_eq!(
            msg(b"a\xc3"),
            "'utf-8' codec can't decode byte 0xc3 in position 1: unexpected end of data"
        );
        assert_eq!(
            msg(b"\xc3("),
            "'utf-8' codec can't decode byte 0xc3 in position 0: invalid continuation byte"
        );
        assert_eq!(
            msg(b"\xe2\x82("),
            "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte"
        );
        assert_eq!(
            msg(b"\xed\xa0\x80"),
            "'utf-8' codec can't decode byte 0xed in position 0: invalid continuation byte"
        );
        assert_eq!(msg(b"\xef\xbb\xbfbom"), "\u{feff}bom");
    }

    #[test]
    fn accessors() {
        let Err(err) = decode_utf8(b"abc\xe2\x82") else {
            panic!("input is invalid")
        };
        assert_eq!((err.start(), err.end(), err.first_byte()), (3, 5, 0xe2));
        assert_eq!(err.reason(), Utf8ErrorReason::UnexpectedEndOfData);
    }
}
