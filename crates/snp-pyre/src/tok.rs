//! The pattern tokenizer: a port of CPython 3.12's `re._parser.Tokenizer` (`re/_parser.py:230-310`).
//!
//! The source is a slice of code units: characters for str patterns, the UTF-8 bytes of the entry for bytes
//! patterns (Python decodes bytes patterns as Latin-1, so every byte is one "character"). A token is one unit,
//! or `\` plus the following unit; a trailing lone `\` raises `bad escape (end of pattern)` as soon as the
//! tokenizer looks ahead at it (01 Addendum §A.6.2 rule T1).
//!
//! Owner: WP-13.

use crate::error::PyReError;
use crate::messages;

/// `\`.
pub(crate) const BACKSLASH: u32 = 0x5C;

/// One token: a unit, or an escape (`\` + unit).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tok {
    /// Index of the first unit.
    pub(crate) start: usize,
    /// The unit, or the escaped unit when `esc` is set.
    pub(crate) c: u32,
    /// The token is `\` + `c`.
    pub(crate) esc: bool,
}

impl Tok {
    /// `len(token)`.
    pub(crate) const fn len(self) -> usize {
        if self.esc { 2 } else { 1 }
    }

    /// The token is the single unit `ch` (Python `this == "x"`).
    pub(crate) fn is(self, ch: char) -> bool {
        !self.esc && self.c == u32::from(ch)
    }

    /// The token is a single unit among `set` (Python `this in "…"` for one-character tokens).
    pub(crate) fn one_of(self, set: &str) -> bool {
        !self.esc && set.chars().any(|ch| self.c == u32::from(ch))
    }

    /// The token is a single ASCII decimal digit (`DIGITS`).
    pub(crate) fn is_digit(self) -> bool {
        !self.esc && (0x30..=0x39).contains(&self.c)
    }

    /// The token is a single octal digit (`OCTDIGITS`).
    pub(crate) fn is_octal(self) -> bool {
        !self.esc && (0x30..=0x37).contains(&self.c)
    }

    /// The token is a single hexadecimal digit (`HEXDIGITS`).
    pub(crate) fn is_hex(self) -> bool {
        !self.esc && char::from_u32(self.c).is_some_and(|c| c.is_ascii_hexdigit())
    }

    /// The token's text, as Python's `this` string.
    pub(crate) fn text(self) -> String {
        let c = unit_char(self.c);
        if self.esc {
            format!("\\{c}")
        } else {
            c.to_string()
        }
    }

    /// Appends the token's text to `out`.
    pub(crate) fn push_to(self, out: &mut String) {
        if self.esc {
            out.push('\\');
        }
        out.push(unit_char(self.c));
    }
}

/// A unit as a character (Latin-1 for bytes patterns; units of str patterns are always scalar values).
pub(crate) fn unit_char(u: u32) -> char {
    char::from_u32(u).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// The tokenizer state (`Tokenizer.index`, `Tokenizer.next`).
#[derive(Debug)]
pub(crate) struct Tokenizer<'a> {
    /// The pattern units.
    units: &'a [u32],
    /// `istext`: a str pattern.
    pub(crate) istext: bool,
    /// `index`: the next unread unit.
    index: usize,
    /// `next`: the lookahead token.
    next: Option<Tok>,
}

impl<'a> Tokenizer<'a> {
    /// `Tokenizer(string)`: reads the first token (which may raise T1).
    pub(crate) fn new(units: &'a [u32], istext: bool) -> Result<Self, PyReError> {
        let mut t = Self {
            units,
            istext,
            index: 0,
            next: None,
        };
        t.advance()?;
        Ok(t)
    }

    /// `__next` (`re/_parser.py:240-255`).
    fn advance(&mut self) -> Result<(), PyReError> {
        let index = self.index;
        let Some(&c) = self.units.get(index) else {
            self.next = None;
            return Ok(());
        };
        if c == BACKSLASH {
            let Some(&e) = self.units.get(index.saturating_add(1)) else {
                return Err(PyReError::at(
                    messages::BAD_ESCAPE_END.into(),
                    self.units.len().saturating_sub(1),
                    self.units,
                ));
            };
            self.next = Some(Tok {
                start: index,
                c: e,
                esc: true,
            });
            self.index = index.saturating_add(2);
        } else {
            self.next = Some(Tok {
                start: index,
                c,
                esc: false,
            });
            self.index = index.saturating_add(1);
        }
        Ok(())
    }

    /// `source.next`.
    pub(crate) const fn peek(&self) -> Option<Tok> {
        self.next
    }

    /// `source.next == ch` for a one-unit token.
    pub(crate) fn next_is(&self, ch: char) -> bool {
        self.next.is_some_and(|t| t.is(ch))
    }

    /// `match(char)`: consumes the lookahead when it is the single unit `ch`.
    pub(crate) fn eat(&mut self, ch: char) -> Result<bool, PyReError> {
        if self.next_is(ch) {
            self.advance()?;
            return Ok(true);
        }
        Ok(false)
    }

    /// `get()`: returns the lookahead and reads the next token.
    pub(crate) fn get(&mut self) -> Result<Option<Tok>, PyReError> {
        let this = self.next;
        self.advance()?;
        Ok(this)
    }

    /// `tell()` / `pos`: the index of the lookahead token.
    pub(crate) fn tell(&self) -> usize {
        self.next
            .map_or(self.index, |t| self.index.saturating_sub(t.len()))
    }

    /// `seek(index)`.
    pub(crate) fn seek(&mut self, index: usize) -> Result<(), PyReError> {
        self.index = index;
        self.advance()
    }

    /// `getwhile(n, charset)` over single-unit tokens accepted by `ok`: appends their text to `out`.
    pub(crate) fn get_while(
        &mut self,
        n: usize,
        ok: impl Fn(Tok) -> bool,
        out: &mut String,
    ) -> Result<usize, PyReError> {
        let mut taken = 0_usize;
        while taken < n {
            match self.next {
                Some(t) if ok(t) => {
                    t.push_to(out);
                    self.advance()?;
                    taken = taken.saturating_add(1);
                }
                _ => break,
            }
        }
        Ok(taken)
    }

    /// `getuntil(terminator, name)` (`re/_parser.py:274-289`): the text before `terminator` and its length
    /// in units.
    pub(crate) fn get_until(
        &mut self,
        terminator: char,
        name: &str,
    ) -> Result<(String, usize), PyReError> {
        let mut result = String::new();
        let mut len = 0_usize;
        loop {
            let c = self.next;
            self.advance()?;
            let Some(c) = c else {
                if len == 0 {
                    return Err(self.error(messages::missing(name), 0));
                }
                return Err(self.error(messages::unterminated_name(terminator), len));
            };
            if c.is(terminator) {
                if len == 0 {
                    return Err(self.error(messages::missing(name), 1));
                }
                return Ok((result, len));
            }
            c.push_to(&mut result);
            len = len.saturating_add(c.len());
        }
    }

    /// `source.error(msg, offset)` (`re/_parser.py:299-302`): position `tell() - offset`; bytes patterns
    /// render the message with `backslashreplace`.
    pub(crate) fn error(&self, msg: String, offset: usize) -> PyReError {
        let msg = if self.istext {
            msg
        } else {
            messages::backslashreplace(&msg)
        };
        PyReError::at(msg, self.tell().saturating_sub(offset), self.units)
    }
}
