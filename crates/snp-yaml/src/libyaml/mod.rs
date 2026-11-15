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
        let body = text.trim_prefix('\u{feff}');
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

    /// `CACHE(parser, n)`: nothing to do while `n` characters are decoded and unread, or once the input is
    /// exhausted; otherwise [`Self::fill`].
    #[inline]
    pub(crate) fn cache(&mut self, n: usize) -> Result<(), Error> {
        if self.unread >= n || self.eof {
            return Ok(());
        }
        self.fill(n)
    }

    /// `yaml_parser_update_buffer(parser, n)`: decodes the next raw chunks until `n` characters are unread, or
    /// appends the NUL at the end of the input. Runs about once per 16 KiB chunk, out of line.
    #[cold]
    #[inline(never)]
    fn fill(&mut self, n: usize) -> Result<(), Error> {
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

    /// `buffer[k]`: the `k`-th unread character; NUL past the end of the input (libyaml's terminator). The
    /// current character when it is ASCII is read inline; everything else is [`Self::peek_slow`].
    #[inline]
    pub(crate) fn peek(&self, k: usize) -> char {
        if k == 0
            && let Some(&b) = self.body.as_bytes().get(self.pos)
            && b.is_ascii()
        {
            return char::from(b);
        }
        self.peek_slow(k)
    }

    /// [`Self::peek`] for lookahead (`k > 0`), a non-ASCII character or the end of the input.
    #[inline(never)]
    fn peek_slow(&self, k: usize) -> char {
        // ASCII up to the wanted character (one byte per character).
        if let Some(window) = self
            .body
            .as_bytes()
            .get(self.pos..self.pos.saturating_add(k).saturating_add(1))
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
    #[inline]
    pub(crate) fn advance(&mut self) -> char {
        let c = match self.body.as_bytes().get(self.pos) {
            Some(&b) if b.is_ascii() => char::from(b),
            Some(_) => self.peek_slow(0),
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

#[cfg(test)]
mod tests {
    use super::{Error, RAW_CHUNK, Reader};

    /// `s` repeated `n` times (`str::repeat` is a disallowed method).
    fn rep(s: &str, n: usize) -> String {
        std::iter::repeat_n(s, n).collect()
    }

    /// `peek(k)` and `advance` against the text's characters at every position (the one-byte fast path, the
    /// ASCII-window and `chars().nth` slow paths, the NUL past the end), with `CACHE` interleaved as the scanner
    /// does.
    #[test]
    fn peek_and_advance_match_the_text_on_every_path() {
        let long = format!("{}\u{e9}{}", rep("a", RAW_CHUNK - 1), rep("b", 3));
        let texts = [
            "",
            "a",
            "ab\n",
            "\u{e9}",
            "a\u{e9}",
            "\u{e9}a",
            "a\u{20ac}b",
            "x\u{10000}y\u{85}z",
            "\u{feff}x\u{e9}",
            "\u{feff}",
            &long,
        ];
        for text in texts {
            let chars: Vec<char> = text.trim_prefix('\u{feff}').chars().collect();
            let mut r = Reader::new(text);
            for i in 0..=chars.len().saturating_add(1) {
                assert_eq!(r.cache(4), Ok(()), "{text:?} at {i}");
                for k in 0..5 {
                    let want = chars.get(i.saturating_add(k)).copied().unwrap_or('\0');
                    assert_eq!(r.peek(k), want, "{text:?}: peek({k}) at {i}");
                }
                let want = chars.get(i).copied().unwrap_or('\0');
                assert_eq!(r.advance(), want, "{text:?}: advance at {i}");
            }
        }
    }

    /// `CACHE(n)` decodes nothing while `n` characters are decoded and unread; a character that straddles the
    /// chunk end waits for the next chunk; the end of the input appends one NUL, once.
    #[test]
    fn cache_decodes_whole_chunks_on_demand() {
        let text = format!("{}\u{e9}{}", rep("a", RAW_CHUNK - 1), rep("b", 10));
        let mut r = Reader::new(&text);
        assert_eq!(r.cache(1), Ok(()));
        // `é` straddles the chunk end: only the `a`s are decoded.
        assert_eq!(
            (r.raw_read, r.decoded, r.unread),
            (RAW_CHUNK, RAW_CHUNK - 1, RAW_CHUNK - 1)
        );
        // Early return: enough characters are decoded, nothing more is read.
        assert_eq!(r.cache(RAW_CHUNK - 1), Ok(()));
        assert_eq!(r.raw_read, RAW_CHUNK);
        for _ in 0..RAW_CHUNK - 2 {
            r.advance();
        }
        assert_eq!(r.unread, 1);
        assert_eq!(r.cache(2), Ok(()));
        assert_eq!((r.decoded, r.unread, r.eof), (text.len(), 12, false));
        assert_eq!(r.cache(13), Ok(()));
        assert_eq!((r.unread, r.eof), (13, true));
        // At the end, CACHE never appends a second NUL.
        assert_eq!(r.cache(20), Ok(()));
        assert_eq!(r.unread, 13);
        let mut empty = Reader::new("");
        assert_eq!(empty.cache(1), Ok(()));
        assert_eq!((empty.unread, empty.eof), (1, true));
    }

    /// A control character is reported when the chunk holding it is decoded, at its byte offset in the input (a
    /// BOM counted), not before.
    #[test]
    fn control_characters_are_found_with_their_chunk() {
        for (prefix, bom) in [("", 0), ("\u{feff}", 3)] {
            for at in [RAW_CHUNK - 1, RAW_CHUNK, RAW_CHUNK + 1] {
                let body = at.saturating_sub(bom);
                let text = format!("{prefix}{}\u{1}z", rep("x", body));
                let mut r = Reader::new(&text);
                let first = r.cache(1);
                if at < RAW_CHUNK {
                    assert_eq!(
                        first,
                        Err(Error::Reader {
                            value: 1,
                            offset: at
                        }),
                        "{at}"
                    );
                    continue;
                }
                assert_eq!(first, Ok(()), "{at}: chunk 1 is clean");
                let unread = r.unread;
                for _ in 0..unread {
                    r.advance();
                }
                assert_eq!(
                    r.cache(1),
                    Err(Error::Reader {
                        value: 1,
                        offset: at
                    }),
                    "{at}"
                );
            }
        }
    }
}
