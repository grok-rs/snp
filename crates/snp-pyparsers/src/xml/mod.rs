//! `check-xml`: expat well-formedness with expat's messages (09 §4.C; `check_xml.py`).
//!
//! A port of the parts of expat 2.6 (`xmltok_impl.c`, `xmlrole.c`, `xmlparse.c`) that decide well-formedness
//! as `xml.sax.parse(f, ContentHandler())` runs it: a non-namespace parser, parameter-entity parsing "unless
//! standalone", external entities accepted without being read (pyexpat's handler returns 1), the encoding
//! detected from the first bytes or declared (expat's built-in encodings, else pyexpat's unknown-encoding handler
//! with Python's single-byte codecs: a multi-byte codec raises `ValueError`, an unknown name `LookupError`), and
//! the input-amplification limit. The message is `str(SAXParseException)`: `{systemId}:{line}:{column}: {text}`
//! with expat's 0-based column in characters.
//!
//! Owner: WP-17.

mod enc;
mod generated;
mod parse;
mod role;
mod tok;

use crate::CheckError;
use crate::json::PyMinor;

/// How one module of Python's `encodings` package decodes `bytes(range(256))` with `errors='replace'`.
#[derive(Copy, Clone, Debug)]
pub(crate) enum Codec {
    /// A single-byte table (index into the minor's `TABLES`).
    Table(usize),
    /// Fewer than 256 characters: pyexpat's `ValueError`.
    MultiByte,
    /// The exception the decode raises.
    Error(&'static str, &'static str),
}

/// The codec tables of a CPython minor.
type CodecTables = (
    &'static [(&'static str, Codec)],
    &'static [(&'static str, &'static str)],
    &'static [[i32; 256]],
);

fn codec_tables(minor: PyMinor) -> CodecTables {
    match minor {
        PyMinor::P312 => (
            generated::codecs_312::MODULES,
            generated::codecs_312::ALIASES,
            generated::codecs_312::TABLES,
        ),
        PyMinor::P313 => (
            generated::codecs_313::MODULES,
            generated::codecs_313::ALIASES,
            generated::codecs_313::TABLES,
        ),
    }
}

/// The expat version the Name classes were probed from (`pyexpat.EXPAT_VERSION`), which the messages and
/// limits are those of.
pub const EXPAT_VERSION: &str = generated::names::EXPAT_VERSION;

/// A failure that is not an expat error (a Python exception escaping `xml.sax.parse`).
#[derive(Debug)]
enum Fail {
    Crash(&'static str, String),
}

/// `xml.sax.parse(…)` (expat); `Err` is the `ExpatError` text.
///
/// The `str()` of the `SAXParseException` for a file without a system id (`<unknown>:{line}:{col}: {msg}`);
/// [`check_ex`] names the file and keeps the crash path apart.
pub fn check(bytes: &[u8]) -> Result<(), String> {
    check_ex(bytes, "<unknown>", PyMinor::P313).map_err(CheckError::into_message)
}

/// [`check`] for the file `name` under CPython `v` (ARCHITECTURE §1.4.11 addition): `Err(Reported)` is
/// `str(SAXParseException)` (`{name}:{line}:{column}: {message}`), which `check_xml.py` prints as
/// `{name}: Failed to xml parse ({…})`; `Err(Crash)` is the exception pyexpat raises from the unknown-encoding
/// handler (`ValueError: multi-byte encodings are not supported`, `LookupError: unknown encoding: …`).
pub fn check_ex(bytes: &[u8], name: &str, v: PyMinor) -> Result<(), CheckError> {
    let mut p = parse::Parser::new(bytes, v);
    match p.run() {
        Ok(()) => Ok(()),
        Err(parse::Stop::Py(Fail::Crash(ty, text))) => Err(CheckError::crash(ty, text)),
        Err(parse::Stop::Xml(e)) => {
            let (line, col) = p.position(p.event);
            Err(CheckError::Reported(format!(
                "{name}:{}:{col}: {}",
                line.saturating_add(1),
                e.text()
            )))
        }
    }
}
