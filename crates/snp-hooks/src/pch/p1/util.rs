//! Helpers shared by the P1, P2 and caveat natives: Python's text-mode reads (strict UTF-8 with CPython's
//! chunk-relative `UnicodeDecodeError` positions, universal newlines), text-mode writes (`\n` → `\r\n` on Windows),
//! the `rb+`/`r+` read of the sorters and fixers (D-CRASH-23), `git ls-files -z --stage` records and raw-byte
//! `OsString`s.
//!
//! Owner: WP-27.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};

use snp_core::failure::PyExc;
use snp_pycompat::codec::decode_utf8;

use crate::HookCx;
use crate::framework;
use crate::pch::p0::common::{cmd_output, read_all};

/// `TextIOWrapper._CHUNK_SIZE`: a text-mode `readline`/iteration decodes the file in chunks of this many bytes.
const CHUNK: usize = 8192;

/// `OsString` of raw bytes (a git output path or a file name).
#[cfg(unix)]
pub(crate) fn os_from(b: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStrExt as _;
    OsStr::from_bytes(b).to_os_string()
}

/// `OsString` of raw bytes (git writes UTF-8 on Windows).
#[cfg(not(unix))]
pub(crate) fn os_from(b: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(b).into_owned())
}

/// The `UnicodeDecodeError` of `bytes.decode()` (whole buffer: `open(f, encoding='UTF-8').read()`, `tomllib`).
fn decode_exc(e: &snp_pycompat::codec::Utf8DecodeError) -> PyExc {
    PyExc::new(e.py_type(), e.to_string())
}

/// `open(f, encoding='UTF-8').read()`: the whole file decoded at once (strict UTF-8, a BOM kept as U+FEFF), then
/// universal newlines (`\r\n` and `\r` → `\n`). `Err` is the `UnicodeDecodeError` with positions in the file.
pub(crate) fn text_read(bytes: &[u8]) -> Result<String, PyExc> {
    let s = decode_utf8(bytes).map_err(|e| decode_exc(&e))?;
    Ok(snp_pycompat::textio::universal_newlines(s).into_owned())
}

/// The first `UnicodeDecodeError` a text-mode *line* read of `bytes` raises (`f.readlines()`, `for line in f`):
/// CPython's `TextIOWrapper` decodes 8192-byte chunks with the incremental UTF-8 decoder, which holds back an
/// incomplete trailing sequence; the error's positions are relative to the decoder call's input (held-back bytes
/// plus the chunk), and a sequence still incomplete at end of file fails in the final, empty decode.
pub(crate) fn chunked_decode_error(bytes: &[u8]) -> Option<PyExc> {
    if core::str::from_utf8(bytes).is_ok() {
        return None;
    }
    let mut held = 0_usize;
    let mut pos = 0_usize;
    loop {
        let last = pos >= bytes.len();
        let end = pos.saturating_add(CHUNK).min(bytes.len());
        let data = bytes.get(held..end).unwrap_or_default();
        match core::str::from_utf8(data) {
            Ok(_) => held = end,
            Err(e) if e.error_len().is_none() && !last => {
                held = held.saturating_add(e.valid_up_to());
            }
            Err(_) => return decode_utf8(data).err().map(|e| decode_exc(&e)),
        }
        if last {
            return None;
        }
        pos = end;
    }
}

/// A text-mode line read of a whole file (`open(f).readlines()` / iteration, UTF-8 locale): the decoded text with
/// universal newlines, or the chunk-relative `UnicodeDecodeError` ([`chunked_decode_error`]).
pub(crate) fn text_read_lines(bytes: &[u8]) -> Result<String, PyExc> {
    if let Some(e) = chunked_decode_error(bytes) {
        return Err(e);
    }
    let s = decode_utf8(bytes).map_err(|e| decode_exc(&e))?;
    Ok(snp_pycompat::textio::universal_newlines(s).into_owned())
}

/// A text-mode write's newline translation (`open(f, 'w')`, `newline=None`): `\n` → `\r\n` when `windows`.
pub(crate) fn text_write_bytes(text: &str, windows: bool) -> Cow<'_, str> {
    if windows {
        snp_pycompat::textio::windows_text_write(text)
    } else {
        Cow::Borrowed(text)
    }
}

/// `open(name, 'rb+')` followed by a full read, as the sorters and fixers do before deciding whether to rewrite.
///
/// The file is read through a read-only handle, so a read-only file that needs no change passes (D-CRASH-23,
/// register row "and the other fixers"); one that does need a change fails with Python's `PermissionError` when
/// it is rewritten. A missing file or a directory raises the same exception `open` would.
pub(crate) fn read_for_update(cx: &HookCx<'_>, name: &OsStr) -> Result<Vec<u8>, PyExc> {
    read_all(cx, name)
}

/// One `git ls-files -z --stage` record: `(mode, path)`.
pub(crate) type LsFile = (String, String);

/// `check_executables_have_shebangs.git_ls_files(paths)` (v4.0.0+): `cmd_output('git', 'ls-files', '-z',
/// '--stage', '--', *paths)`, `zsplit`, then per record `metadata, filename = out.split('\t')` and `mode, _, _ =
/// metadata.split()`.
///
/// Python's function is a generator: the records before a malformed one are handled before the `ValueError` is
/// raised, so the entries parsed so far come back together with the pending error. A failing git call (or
/// undecodable output) is raised before any record.
pub(crate) fn git_ls_files(
    cx: &HookCx<'_>,
    paths: &[&OsStr],
) -> Result<(Vec<LsFile>, Option<PyExc>), PyExc> {
    let mut args: Vec<&OsStr> = vec![
        OsStr::new("ls-files"),
        OsStr::new("-z"),
        OsStr::new("--stage"),
        OsStr::new("--"),
    ];
    args.extend_from_slice(paths);
    let outs = cmd_output(cx, &args, Some(0))?;
    Ok(parse_ls_files(&outs))
}

/// The record parse of [`git_ls_files`] (pure; see there).
pub(crate) fn parse_ls_files(outs: &str) -> (Vec<LsFile>, Option<PyExc>) {
    let mut entries = Vec::new();
    let trimmed = outs.trim_matches('\0');
    if trimmed.is_empty() {
        return (entries, None);
    }
    for rec in trimmed.split('\0') {
        let tabs = rec.split('\t').count();
        let mut parts = rec.split('\t');
        let (Some(meta), Some(path), None) = (parts.next(), parts.next(), parts.next()) else {
            return (entries, Some(unpack_error(tabs, 2)));
        };
        let fields = snp_pycompat::text::split_whitespace(meta, None).collect::<Vec<&str>>();
        let [mode, _, _] = fields.as_slice() else {
            return (entries, Some(unpack_error(fields.len(), 3)));
        };
        entries.push(((*mode).to_owned(), path.to_owned()));
    }
    (entries, None)
}

/// Python's tuple-unpacking `ValueError` for `got` items into `expected` names.
pub(crate) fn unpack_error(got: usize, expected: usize) -> PyExc {
    PyExc::new(
        "ValueError",
        if got > expected {
            format!("too many values to unpack (expected {expected})")
        } else {
            format!("not enough values to unpack (expected {expected}, got {got})")
        },
    )
}

/// `any(b in EXECUTABLE_VALUES for b in mode[-3:])` (`check_executables_have_shebangs.py`).
pub(crate) fn mode_is_executable(mode: &str) -> bool {
    let tail = mode
        .char_indices()
        .rev()
        .nth(2)
        .map_or(mode, |(i, _)| mode.get(i..).unwrap_or(mode));
    tail.chars().any(|c| matches!(c, '1' | '3' | '5' | '7'))
}

/// `has_shebang(path)`: `open(path, 'rb').read(2) == b'#!'`.
pub(crate) fn has_shebang(cx: &HookCx<'_>, path: &OsStr) -> Result<bool, PyExc> {
    Ok(crate::pch::p0::common::read_head(cx, path, 2)? == b"#!")
}

/// The raw bytes of an argv string.
pub(crate) fn bytes_of(s: &OsStr) -> &[u8] {
    framework::os_bytes(s)
}

/// `str(s)` of an argv string for messages that format it into a Python `str` (lossy only for undecodable bytes
/// on platforms where argv is not raw bytes).
pub(crate) fn str_of(s: &OsStr) -> Cow<'_, str> {
    s.to_string_lossy()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_positions_are_relative_to_the_decoder_input() {
        assert!(chunked_decode_error(b"plain\n").is_none());
        let e = chunked_decode_error(b"ab\xffc").unwrap();
        assert_eq!(
            String::from_utf8(e.text).unwrap(),
            "'utf-8' codec can't decode byte 0xff in position 2: invalid start byte"
        );
        // Second chunk: the position restarts.
        let mut v = vec![b'a'; CHUNK];
        v.extend_from_slice(b"xy\xff");
        let e = chunked_decode_error(&v).unwrap();
        assert_eq!(
            String::from_utf8(e.text).unwrap(),
            "'utf-8' codec can't decode byte 0xff in position 2: invalid start byte"
        );
        // A sequence split across chunks is held back; its continuation is checked with the next chunk.
        let mut v = vec![b'a'; CHUNK - 1];
        v.extend_from_slice(b"\xe2\x82\xac\xff");
        let e = chunked_decode_error(&v).unwrap();
        assert_eq!(
            String::from_utf8(e.text).unwrap(),
            "'utf-8' codec can't decode byte 0xff in position 3: invalid start byte"
        );
        // Incomplete at end of file: the final, empty decode reports the held bytes.
        let mut v = vec![b'a'; CHUNK + 2];
        v.extend_from_slice(b"\xe2\x82");
        let e = chunked_decode_error(&v).unwrap();
        assert_eq!(
            String::from_utf8(e.text).unwrap(),
            "'utf-8' codec can't decode bytes in position 0-1: unexpected end of data"
        );
    }

    #[test]
    fn ls_files_records() {
        let (e, err) = parse_ls_files("100644 abc 0\ta b\0100755 def 0\tx\0");
        assert!(err.is_none());
        assert_eq!(
            e,
            vec![
                ("100644".into(), "a b".into()),
                ("100755".into(), "x".into())
            ]
        );
        let (e, err) = parse_ls_files("100644 abc 0\ta\0100644 abc 0\tb\tc\0");
        assert_eq!(e.len(), 1);
        assert_eq!(
            String::from_utf8(err.unwrap().text).unwrap(),
            "too many values to unpack (expected 2)"
        );
        assert!(parse_ls_files("").0.is_empty());
        assert!(mode_is_executable("100755"));
        assert!(!mode_is_executable("100644"));
        assert!(!mode_is_executable(""));
    }
}
