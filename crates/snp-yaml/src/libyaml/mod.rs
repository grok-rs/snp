//! libyaml 0.2.5's reader, scanner and parser, ported to safe, panic-free Rust (the ADR-002 fork of libyaml-safer).
//!
//! PyYAML's `CSafeLoader` scans and parses with libyaml 0.2.5; snp reproduces its behaviour, error texts and marks.
//! The port follows libyaml-safer 0.3.0 (itself a line-by-line port of libyaml via unsafe-libyaml; MIT, see
//! `LICENSE-libyaml`), with the differences from C libyaml that libyaml-safer has fixed here:
//!
//! * no panics: libyaml-safer 0.3.0 panics on a double-quoted scalar ending the input with `\`
//!   (`scanner.rs:1541`) and on a tag followed by `,` in a flow collection (`scanner.rs:1112`, a `TODO`); C libyaml
//!   reads the NUL that terminates its buffer in the first case (`found unknown escape character`) and ends the
//!   tag in the second;
//! * marks count characters, as in C (`index` and the 1024-character limit of simple keys); byte offsets are kept
//!   alongside for spans;
//! * the input is a `&str` whose leading BOM is skipped like C's `yaml_parser_determine_encoding` (libyaml-safer
//!   rejects any first character in `U+F000..U+FFFF` as an "invalid byte order marker");
//! * `%XX` escapes in tags produce bytes, decoded as UTF-8 by the composer as PyYAML does (libyaml-safer turns each
//!   octet into a Latin-1 character);
//! * control characters are found when the reader decodes the 16 KiB chunk that holds them, exactly when C's
//!   `yaml_parser_update_buffer` does, so reader and scanner errors are reported in C's order.
//!
//! Owner: WP-14.

pub(crate) mod parser;
pub(crate) mod scanner;

/// A position: `index` and `column` count characters (libyaml's `yaml_mark_t`), `byte` is the offset in the input
/// after a leading BOM.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Mark {
    pub(crate) index: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) byte: usize,
}

/// Scalar presentation style (`yaml_scalar_style_t`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Plain,
    Single,
    Double,
    Literal,
    Folded,
}

/// A scanner, parser or reader error (`yaml_parser_t.error` and its fields).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    /// `YAML_SCANNER_ERROR` / `YAML_PARSER_ERROR`: `context` is `None` when libyaml set none.
    Marked {
        scanner: bool,
        context: Option<(&'static str, Mark)>,
        problem: &'static str,
        problem_mark: Mark,
    },
    /// `YAML_READER_ERROR` "control characters are not allowed": the character and its byte offset in the input
    /// (a leading BOM included, as libyaml's `parser->offset`).
    Reader { value: u32, offset: usize },
    /// An internal limit (an input longer than `usize`, which a `&str` cannot be).
    Overflow,
}

impl Error {
    pub(crate) const fn scanner(
        context: &'static str,
        context_mark: Mark,
        problem: &'static str,
        at: Mark,
    ) -> Self {
        Self::Marked {
            scanner: true,
            context: if context.is_empty() {
                None
            } else {
                Some((context, context_mark))
            },
            problem,
            problem_mark: at,
        }
    }

    pub(crate) const fn parser(
        context: Option<(&'static str, Mark)>,
        problem: &'static str,
        at: Mark,
    ) -> Self {
        Self::Marked {
            scanner: false,
            context,
            problem,
            problem_mark: at,
        }
    }
}

/// libyaml's `INPUT_RAW_BUFFER_SIZE`: the reader decodes (and checks) the input in chunks of this many bytes.
const RAW_CHUNK: usize = 16_384;

/// The reader: the input text, the decoded window and libyaml's `unread` count.
///
/// `yaml_parser_update_buffer(parser, n)` decodes whole raw chunks until `n` characters are available; at the end
/// of the input it appends one NUL. Characters are read here straight from the `&str`; "decoding" a chunk means
/// checking every character of it against libyaml's allowed set, so a control character is reported at the same
/// `CACHE` call as in C.
#[derive(Debug)]
pub(crate) struct Reader<'a> {
    /// The input after a leading BOM.
    body: &'a str,
    /// Length of the skipped BOM (0 or 3).
    bom: usize,
    /// Byte offset in `body` of `buffer[0]`.
    pos: usize,
    /// Byte offset in `body` up to which characters have been decoded.
    decoded: usize,
    /// Bytes of the whole input (BOM included) read into the raw buffer so far.
    raw_read: usize,
    /// `parser->unread`: decoded characters not consumed yet (the final NUL included).
    unread: usize,
    /// The input is exhausted and the NUL has been appended.
    eof: bool,
}

/// libyaml's reader check (`yaml_parser_update_buffer`): `#x9 | #xA | #xD | [#x20-#x7E] | #x85 | [#xA0-#xD7FF] |
/// [#xE000-#xFFFD] | [#x10000-#x10FFFF]`.
const fn allowed(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\r'
            | ' '..='~'
            | '\u{85}'
            | '\u{a0}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}'
    )
}

impl<'a> Reader<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        let body = text.strip_prefix('\u{feff}').unwrap_or(text);
        Self {
            body,
            bom: text.len().saturating_sub(body.len()),
            pos: 0,
            decoded: 0,
            raw_read: 0,
            unread: 0,
            eof: false,
        }
    }

    /// `CACHE(parser, n)`.
    pub(crate) fn cache(&mut self, n: usize) -> Result<(), Error> {
        if self.unread >= n || self.eof {
            return Ok(());
        }
        let total = self.body.len().saturating_add(self.bom);
        loop {
            if self.raw_read >= total {
                // The read handler returned 0 bytes: append the NUL.
                self.eof = true;
                self.unread = self.unread.saturating_add(1);
                return Ok(());
            }
            self.raw_read = self.raw_read.saturating_add(RAW_CHUNK).min(total);
            let limit = self.raw_read.saturating_sub(self.bom);
            let rest = self.body.get(self.decoded..).unwrap_or("");
            for c in rest.chars() {
                let end = self.decoded.saturating_add(c.len_utf8());
                if end > limit {
                    // An incomplete character stays in the raw buffer.
                    break;
                }
                if !allowed(c) {
                    return Err(Error::Reader {
                        value: u32::from(c),
                        offset: self.decoded.saturating_add(self.bom),
                    });
                }
                self.decoded = end;
                self.unread = self.unread.saturating_add(1);
            }
            if self.unread >= n {
                return Ok(());
            }
        }
    }

    /// `buffer[k]`: the `k`-th unread character; NUL past the end of the input (libyaml's terminator).
    pub(crate) fn peek(&self, k: usize) -> char {
        let bytes = self.body.as_bytes();
        if k == 0
            && let Some(&b) = bytes.get(self.pos)
            && b.is_ascii()
        {
            return char::from(b);
        }
        // Fast path: ASCII up to the wanted character (one byte per character).
        if let Some(window) = bytes.get(self.pos..self.pos.saturating_add(k).saturating_add(1))
            && window.is_ascii()
            && let Some(&b) = window.last()
        {
            return char::from(b);
        }
        self.body
            .get(self.pos..)
            .and_then(|s| s.chars().nth(k))
            .unwrap_or('\0')
    }

    /// Consumes `buffer[0]` (no-op at the end of the input) and returns it.
    pub(crate) fn advance(&mut self) -> char {
        let c = match self.body.as_bytes().get(self.pos) {
            Some(&b) if b.is_ascii() => char::from(b),
            Some(_) => self.peek(0),
            None => '\0',
        };
        if self.pos < self.body.len() {
            self.pos = self.pos.saturating_add(c.len_utf8());
        }
        self.unread = self.unread.saturating_sub(1);
        c
    }

    /// The byte offset in the body of `buffer[0]`.
    pub(crate) const fn pos(&self) -> usize {
        self.pos
    }

    /// The input between two byte offsets of the body (empty if they are not character boundaries).
    pub(crate) fn slice(&self, start: usize, end: usize) -> &'a str {
        self.body.get(start..end).unwrap_or("")
    }
}
