//! `ruamel/yaml/reader.py` (`Reader`) over a text stream: 4096-character `read()` chunks pulled lazily as the
//! scanner looks ahead, each checked for non-printable characters when it is appended, so the order of reader
//! errors versus scanner errors (and `UnicodeDecodeError` crashes) is Python's.
//!
//! The buffer is kept absolute (`buf[i]` is character `i` of the stream; Python slices it at the pointer, which
//! changes no observable value): `index` is both Python's `pointer` into the sliced buffer and its `index`.
//!
//! Owner: WP-17.

use std::rc::Rc;

use super::{Fail, Loader, Mark, R, Rx};

/// `Reader.update_raw`'s default read size.
const READ: usize = 4096;

/// Reader fields of [`Loader`].
#[derive(Debug, Default)]
pub(super) struct ReaderState {
    /// Characters appended to the buffer (plus the `'\0'` terminator once EOF was seen).
    pub(super) buf: Vec<char>,
    pub(super) index: usize,
    pub(super) line: usize,
    pub(super) column: usize,
    /// `raw_buffer`: read but not yet checked and appended; `None` before the first read and after EOF.
    raw: Option<Vec<char>>,
    eof: bool,
}

/// `Reader.NON_PRINTABLE` (and the ASCII fast path, which finds the same character).
fn printable(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\x20'..='\x7e' | '\u{85}' | '\u{a0}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}')
        || u32::from(c) >= 0x1_0000
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the loader's methods live next to the ruamel module they port"
)]
impl Loader<'_, '_> {
    /// `stream.setter` for a file: `name`, `eof = False`, `determine_encoding()` (reads until two characters
    /// are buffered, then `update(1)`).
    pub(super) fn reader_open(&mut self) -> R<()> {
        self.frame(|s| {
            // determine_encoding
            s.frame(|s| {
                while !s.r.eof && s.r.raw.as_ref().is_none_or(|r| r.len() < 2) {
                    s.update_raw()?;
                }
                s.update(1)
            })
        })
    }

    /// `Reader.peek(index)`.
    pub(super) fn peek(&mut self, i: usize) -> R<char> {
        self.frame(|s| {
            let at = s.r.index.saturating_add(i);
            if let Some(&c) = s.r.buf.get(at) {
                return Ok(c);
            }
            s.update(i.saturating_add(1))?;
            s.r.buf
                .get(at)
                .copied()
                .ok_or_else(|| Fail::crash("IndexError", "string index out of range"))
        })
    }

    /// `Reader.prefix(length)` (one frame, the same `update`) with the characters handed to `f` in place.
    fn with_prefix<T>(&mut self, length: usize, f: impl FnOnce(&[char]) -> T) -> R<T> {
        self.frame(|s| {
            if s.r.index.saturating_add(length) >= s.r.buf.len() {
                s.update(length)?;
            }
            let end = s.r.index.saturating_add(length).min(s.r.buf.len());
            Ok(f(s.r.buf.get(s.r.index..end).unwrap_or_default()))
        })
    }

    /// `Reader.prefix(length)`.
    pub(super) fn prefix(&mut self, length: usize) -> R<String> {
        self.with_prefix(length, |p| p.iter().collect())
    }

    /// `out += self.reader.prefix(length)` on the UTF-8 bytes of a scalar.
    pub(super) fn prefix_into(&mut self, length: usize, out: &mut Vec<u8>) -> R<()> {
        self.with_prefix(length, |p| {
            for c in p {
                out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            }
        })
    }

    /// `prefix(length) == text` for an ASCII `text`, compared in place.
    pub(super) fn prefix_is(&mut self, text: &str) -> R<bool> {
        self.with_prefix(text.len(), |p| p.iter().copied().eq(text.chars()))
    }

    /// `prefix(3) == '---' or prefix(3) == '...'` (one `prefix` call), compared in place.
    pub(super) fn prefix_is_doc_marker(&mut self) -> R<bool> {
        self.with_prefix(3, |p| matches!(p, ['-', '-', '-'] | ['.', '.', '.']))
    }

    /// `Reader.forward(length)` (`'\r'` never reaches the reader: the text stream translated it).
    pub(super) fn forward(&mut self, length: usize) -> R<()> {
        self.frame(|s| {
            if s.r.index.saturating_add(length).saturating_add(1) >= s.r.buf.len() {
                s.update(length.saturating_add(1))?;
            }
            for _ in 0..length {
                let ch = s.r.buf.get(s.r.index).copied();
                let Some(ch) = ch else {
                    return Err(Fail::crash("IndexError", "string index out of range"));
                };
                s.r.index = s.r.index.saturating_add(1);
                if ch == '\n' || ch == '\r' && s.r.buf.get(s.r.index) != Some(&'\n') {
                    s.r.line = s.r.line.saturating_add(1);
                    s.r.column = 0;
                } else if ch != '\u{feff}' {
                    s.r.column = s.r.column.saturating_add(1);
                }
            }
            Ok(())
        })
    }

    /// `forward(1)`.
    pub(super) fn fwd(&mut self) -> R<()> {
        self.forward(1)
    }

    /// `Reader.get_mark()`: a `FileMark` (the `stream` property and `StreamMark.__init__` one frame below).
    pub(super) fn get_mark(&mut self) -> R<Mark> {
        self.frame(|s| {
            s.touch(1)?;
            Ok(s.mark_here())
        })
    }

    /// The current position as a mark, without Python frames.
    pub(super) fn mark_here(&self) -> Mark {
        Mark {
            name: Rc::clone(&self.name),
            line: self.r.line,
            column: self.r.column,
        }
    }

    /// `Reader.update(length)`.
    fn update(&mut self, length: usize) -> R<()> {
        self.frame(|s| {
            if s.r.raw.is_none() {
                return Ok(());
            }
            while s.r.buf.len().saturating_sub(s.r.index) < length {
                if !s.r.eof {
                    s.update_raw()?;
                }
                let data = s.r.raw.take().unwrap_or_default();
                let base = s.r.buf.len();
                s.check_printable(&data, base)?;
                s.r.buf.extend_from_slice(&data);
                if s.r.eof {
                    s.r.buf.push('\0');
                    s.r.raw = None;
                    break;
                }
                s.r.raw = Some(Vec::new());
            }
            Ok(())
        })
    }

    /// `Reader.update_raw()`: `self.stream.read(4096)`.
    fn update_raw(&mut self) -> R<()> {
        self.frame(|s| {
            s.touch(1)?; // the `stream` property
            let depth = s.depth;
            let data = s.io.read(READ, || {
                if depth.saturating_add(1) > super::LIMIT {
                    Err(Fail::recursion())
                } else {
                    Ok(())
                }
            })?;
            if data.is_empty() {
                s.r.eof = true;
            }
            match &mut s.r.raw {
                Some(raw) => raw.extend_from_slice(&data),
                None => s.r.raw = Some(data),
            }
            Ok(())
        })
    }

    /// `Reader.check_printable(data)`: `ReaderError` at the stream position of the first non-printable
    /// character of the chunk (`base` = characters before the chunk).
    fn check_printable(&mut self, data: &[char], base: usize) -> R<()> {
        self.frame(|s| {
            // _get_non_printable -> _get_non_printable_ascii (-> _get_non_printable_regex for non-ASCII data)
            s.frame(|s| {
                s.touch(1)?;
                if !data.iter().all(char::is_ascii) {
                    s.frame(|s| s.rx_call(Rx::NonPrintable))?;
                }
                Ok(())
            })?;
            if let Some((start, &ch)) = data.iter().enumerate().find(|(_, c)| !printable(**c)) {
                s.touch(1)?;
                let msg = format!(
                    "unacceptable character #x{:04x}: special characters are not allowed\n  in \"{}\", position {}",
                    u32::from(ch),
                    s.name,
                    base.saturating_add(start)
                );
                return Err(Fail::Yaml(msg.into_bytes()));
            }
            Ok(())
        })
    }
}
