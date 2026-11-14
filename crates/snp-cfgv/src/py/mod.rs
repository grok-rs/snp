//! Helpers that reproduce CPython semantics byte for byte.
//!
//! Python cfgv builds its error messages with `repr()`/`str()` of arbitrary
//! values and reads configuration files with Python's text I/O. These helpers
//! reproduce the observable behaviour of CPython 3.12 so that messages are
//! byte-identical to the ones Python cfgv produces:
//!
//! * [`repr_str`], [`repr_bytes`] and [`repr_float`]: `repr()` of `str`,
//!   `bytes` and `float` (quote selection, escapes, shortest round-trip float
//!   digits, `1e+16`, `1e-05`, `inf`, `nan`, `-0.0`);
//! * [`is_printable`]: `str.isprintable()` for one code point, driven by a
//!   table generated from the Unicode database of CPython 3.12
//!   ([`UNICODE_VERSION`]);
//! * [`decode_utf8`]: strict UTF-8 decoding whose error renders exactly like
//!   `str(UnicodeDecodeError)`;
//! * [`universal_newlines`]: the newline translation of Python text mode.
//!
//! `repr()` of the other Python types is available through
//! [`Value::repr`](crate::Value::repr).
//!
//! ```
//! use cfgv::py;
//!
//! assert_eq!(py::repr_str("it's"), r#""it's""#);
//! assert_eq!(py::repr_str("a\\b\tc"), r"'a\\b\tc'");
//! assert_eq!(py::repr_float(1e16), "1e+16");
//! assert_eq!(py::repr_float(1.0), "1.0");
//! assert_eq!(py::repr_bytes(b"\x00ab"), r"b'\x00ab'");
//! ```

mod repr;
mod tables;
mod text;
mod unicode;
mod utf8;

pub use repr::{
    repr_bytes, repr_float, repr_str, write_repr_bytes, write_repr_float, write_repr_str,
};
pub use text::universal_newlines;
pub use unicode::{UNICODE_VERSION, is_printable};
pub use utf8::{Utf8DecodeError, Utf8ErrorReason, decode_utf8};
