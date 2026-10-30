//! cfgv's `open(…, encoding='utf-8').read()`.
//!
//! Owner: WP-14.

use std::borrow::Cow;

use snp_pycompat::codec::{Utf8DecodeError, decode_utf8};
use snp_pycompat::textio::universal_newlines;

/// cfgv `open(…, encoding='utf-8').read()` parity (`cfgv.py:439`, 01 §12.9): strict UTF-8 (CPython error text) +
/// universal newlines.
///
/// `TextIOWrapper.read()` decodes the whole file in one call, so the error's position is the byte offset of the
/// first undecodable byte in the file (`'utf-8' codec can't decode byte 0xff in position 0: invalid start byte`).
/// A UTF-8 byte-order mark is kept as U+FEFF (the `utf-8` codec, unlike `utf-8-sig`, does not strip it; libyaml
/// skips it later). `\r\n` and a lone `\r` become `\n`; the result borrows `bytes` when there is no `\r`.
pub fn decode_text(bytes: &[u8]) -> Result<Cow<'_, str>, Utf8DecodeError> {
    decode_utf8(bytes).map(universal_newlines)
}
