//! cfgv's `open(…, encoding='utf-8').read()`.
//!
//! Owner: WP-14.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-14")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-14")]

use std::borrow::Cow;

use snp_pycompat::codec::Utf8DecodeError;

/// cfgv `open(…, encoding='utf-8').read()` parity: strict UTF-8 (CPython error text) + universal newlines.
pub fn decode_text(bytes: &[u8]) -> Result<Cow<'_, str>, Utf8DecodeError> {
    todo!()
}
