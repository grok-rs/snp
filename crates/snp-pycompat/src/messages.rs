//! Every user-visible text of this crate (ARCHITECTURE §1.3 "Messages"): CPython's own exception
//! wording, reproduced byte for byte. None of them names the tool, so they are identical under both
//! brands; `tests/messages.rs` holds the goldens and the vector files prove them against CPython.
//!
//! Owner: WP-11.

use std::fmt;

/// The class of every `os.path`, `shlex`, `int()` and unpacking error.
pub(crate) const VALUE_ERROR: &str = "ValueError";
/// The class of a failed strict UTF-8 decode.
pub(crate) const UNICODE_DECODE_ERROR: &str = "UnicodeDecodeError";
/// The class of a failed strict UTF-8 encode of a lone surrogate.
pub(crate) const UNICODE_ENCODE_ERROR: &str = "UnicodeEncodeError";

/// `posixpath.relpath` / `ntpath.relpath` with an empty path (`posixpath.py`, `ntpath.py`).
pub(crate) const NO_PATH_SPECIFIED: &str = "no path specified";
/// `commonpath([])` (CPython 3.12 text; 3.13 says "empty iterable").
pub(crate) const COMMONPATH_EMPTY: &str = "commonpath() arg is an empty sequence";
/// `commonpath` over absolute and relative paths.
pub(crate) const MIX_ABS_REL: &str = "Can't mix absolute and relative paths";
/// `ntpath.commonpath` over different drives.
pub(crate) const DIFFERENT_DRIVES: &str = "Paths don't have the same drive";

/// `shlex.py` `read_token`: end of input inside quotes.
pub(crate) const NO_CLOSING_QUOTATION: &str = "No closing quotation";
/// `shlex.py` `read_token`: end of input right after a backslash.
pub(crate) const NO_ESCAPED_CHARACTER: &str = "No escaped character";

/// The prefix of `int()`'s error (`Objects/longobject.c`, `"invalid literal for int() with base
/// %d: %.200R"`).
pub(crate) const INT_LITERAL_PREFIX: &str = "invalid literal for int() with base 10: ";
/// How many characters of the literal's repr `%.200R` keeps.
pub(crate) const INT_LITERAL_REPR_CHARS: usize = 200;

/// The `reason` of CPython's UTF-8 decoder (`Objects/stringlib/codecs.h`).
pub(crate) const INVALID_START_BYTE: &str = "invalid start byte";
/// See [`INVALID_START_BYTE`].
pub(crate) const INVALID_CONTINUATION_BYTE: &str = "invalid continuation byte";
/// See [`INVALID_START_BYTE`].
pub(crate) const UNEXPECTED_END_OF_DATA: &str = "unexpected end of data";

/// `ntpath.relpath` across drives: `"path is on mount %r, start on mount %r"`.
pub(crate) fn different_mounts(
    f: &mut fmt::Formatter<'_>,
    path_drive_repr: &str,
    start_drive_repr: &str,
) -> fmt::Result {
    write!(
        f,
        "path is on mount {path_drive_repr}, start on mount {start_drive_repr}"
    )
}

/// `UNPACK_SEQUENCE` with too few values (`Python/ceval.c`, 3.12 wording).
pub(crate) fn unpack_not_enough(
    f: &mut fmt::Formatter<'_>,
    expected: usize,
    got: usize,
) -> fmt::Result {
    write!(
        f,
        "not enough values to unpack (expected {expected}, got {got})"
    )
}

/// `UNPACK_SEQUENCE` with too many values (3.12 wording, without the 3.14 `, got N`).
pub(crate) fn unpack_too_many(f: &mut fmt::Formatter<'_>, expected: usize) -> fmt::Result {
    write!(f, "too many values to unpack (expected {expected})")
}

/// `str(UnicodeDecodeError)` for one undecodable byte (`Objects/exceptions.c`).
pub(crate) fn decode_byte(
    f: &mut fmt::Formatter<'_>,
    byte: u8,
    position: usize,
    reason: &str,
) -> fmt::Result {
    write!(
        f,
        "'utf-8' codec can't decode byte 0x{byte:02x} in position {position}: {reason}"
    )
}

/// `str(UnicodeDecodeError)` for a range of bytes (`end` inclusive, as CPython prints it).
pub(crate) fn decode_bytes(
    f: &mut fmt::Formatter<'_>,
    start: usize,
    end_inclusive: usize,
    reason: &str,
) -> fmt::Result {
    write!(
        f,
        "'utf-8' codec can't decode bytes in position {start}-{end_inclusive}: {reason}"
    )
}

/// `str(UnicodeEncodeError)` for one lone surrogate (`Objects/exceptions.c`).
pub(crate) fn encode_surrogate(
    f: &mut fmt::Formatter<'_>,
    surrogate: u16,
    position: usize,
) -> fmt::Result {
    write!(
        f,
        "'utf-8' codec can't encode character '\\u{surrogate:04x}' in position {position}: \
         surrogates not allowed"
    )
}

/// `subprocess.CalledProcessError.__str__` for a non-negative return code.
pub(crate) fn cpe_exit_status(cmd: &str, rc: i64) -> String {
    format!("Command '{cmd}' returned non-zero exit status {rc}.")
}

/// `subprocess.CalledProcessError.__str__` for a known signal (`%r` of `signal.Signals`).
pub(crate) fn cpe_died_with(cmd: &str, name: &str, sig: u64) -> String {
    format!("Command '{cmd}' died with <Signals.{name}: {sig}>.")
}

/// `subprocess.CalledProcessError.__str__` for a signal number `signal.Signals` does not know.
pub(crate) fn cpe_unknown_signal(cmd: &str, sig: u64) -> String {
    format!("Command '{cmd}' died with unknown signal {sig}.")
}
