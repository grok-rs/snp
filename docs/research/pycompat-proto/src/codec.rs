//! `bytes.decode()` (strict UTF-8) with CPython's exact `UnicodeDecodeError`
//! text, and `errors='backslashreplace'`.
use std::borrow::Cow;
use std::fmt::Write as _;

/// Why CPython's UTF-8 decoder stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Utf8Reason {
    InvalidStartByte,
    InvalidContinuationByte,
    UnexpectedEndOfData,
}

/// `UnicodeDecodeError` for the `'utf-8'` codec. `start..end` is the byte
/// range CPython reports (maximal invalid subpart, identical to Rust's
/// `Utf8Error::valid_up_to` / `error_len`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Utf8DecodeError {
    pub start: usize,
    pub end: usize,
    pub first_byte: u8,
    pub reason: Utf8Reason,
}

impl core::fmt::Display for Utf8DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let reason = match self.reason {
            Utf8Reason::InvalidStartByte => "invalid start byte",
            Utf8Reason::InvalidContinuationByte => "invalid continuation byte",
            Utf8Reason::UnexpectedEndOfData => "unexpected end of data",
        };
        if self.end - self.start == 1 {
            write!(f, "'utf-8' codec can't decode byte 0x{:02x} in position {}: {reason}", self.first_byte, self.start)
        } else {
            write!(f, "'utf-8' codec can't decode bytes in position {}-{}: {reason}", self.start, self.end - 1)
        }
    }
}
impl std::error::Error for Utf8DecodeError {}

const fn is_start_byte(b: u8) -> bool {
    matches!(b, 0x00..=0x7f | 0xc2..=0xf4)
}

/// `b.decode()` (strict UTF-8). The BOM is kept as U+FEFF (codec `utf-8`, not `utf-8-sig`).
pub fn decode_utf8(b: &[u8]) -> Result<&str, Utf8DecodeError> {
    core::str::from_utf8(b).map_err(|e| {
        let start = e.valid_up_to();
        let first_byte = b.get(start).copied().unwrap_or(0);
        match e.error_len() {
            None => Utf8DecodeError { start, end: b.len(), first_byte, reason: Utf8Reason::UnexpectedEndOfData },
            Some(n) => Utf8DecodeError {
                start,
                end: start + usize::from(n),
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

/// `b.decode(errors='backslashreplace')`: each undecodable byte becomes `\xNN`.
#[must_use]
pub fn decode_utf8_backslashreplace(mut b: &[u8]) -> Cow<'_, str> {
    if let Ok(s) = core::str::from_utf8(b) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(b.len() + 8);
    loop {
        match core::str::from_utf8(b) {
            Ok(s) => {
                out.push_str(s);
                return Cow::Owned(out);
            }
            Err(e) => {
                let (ok, rest) = b.split_at(e.valid_up_to());
                out.push_str(core::str::from_utf8(ok).unwrap_or_default());
                let n = e.error_len().map_or(rest.len(), usize::from).min(rest.len());
                let (bad, tail) = rest.split_at(n);
                for x in bad {
                    let _ = write!(out, "\\x{x:02x}");
                }
                b = tail;
            }
        }
    }
}
