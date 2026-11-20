//! The quoted and plain scalar scanners as they were before alloc-7 (ADR-138): the quoted scanner copies every
//! character into its value, both keep libyaml's `leading_break` and `whitespaces` `String` buffers. Test-only: the
//! differential test in [`super`] compares the current scanners against these on generated inputs (values, styles,
//! marks, errors and the scanner state after).

use super::{
    HEX_ESCAPE_PROBLEM, R, Scanner, Style, Token, TokenKind, hex, is_blank, is_blankz, is_break,
};

impl Scanner<'_> {
    pub(super) fn scan_flow_scalar_reference(&mut self, single: bool) -> R<Token> {
        let mut s = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        let mut whitespaces = String::new();
        let quote = if single { '\'' } else { '"' };
        let start = self.mark;
        self.skip();
        loop {
            self.cache(4)?;
            if self.mark.column == 0
                && ((self.ch(0) == '-' && self.ch(1) == '-' && self.ch(2) == '-')
                    || (self.ch(0) == '.' && self.ch(1) == '.' && self.ch(2) == '.'))
                && is_blankz(self.ch(3))
            {
                return self.err(
                    "while scanning a quoted scalar",
                    start,
                    "found unexpected document indicator",
                );
            }
            if self.ch(0) == '\0' {
                return self.err(
                    "while scanning a quoted scalar",
                    start,
                    "found unexpected end of stream",
                );
            }
            self.cache(2)?;
            let mut leading_blanks = false;
            while !is_blankz(self.ch(0)) {
                if single && self.ch(0) == '\'' && self.ch(1) == '\'' {
                    s.push('\'');
                    self.skip();
                    self.skip();
                } else {
                    if self.ch(0) == quote {
                        break;
                    }
                    if !single && self.ch(0) == '\\' && is_break(self.ch(1)) {
                        self.cache(3)?;
                        self.skip();
                        self.skip_line();
                        leading_blanks = true;
                        break;
                    } else if !single && self.ch(0) == '\\' {
                        let mut code_length = 0usize;
                        match self.ch(1) {
                            '0' => s.push('\0'),
                            'a' => s.push('\x07'),
                            'b' => s.push('\x08'),
                            't' | '\t' => s.push('\t'),
                            'n' => s.push('\n'),
                            'v' => s.push('\x0b'),
                            'f' => s.push('\x0c'),
                            'r' => s.push('\r'),
                            'e' => s.push('\x1b'),
                            ' ' => s.push(' '),
                            '"' => s.push('"'),
                            '/' => s.push('/'),
                            '\\' => s.push('\\'),
                            'N' => s.push('\u{85}'),
                            '_' => s.push('\u{a0}'),
                            'L' => s.push('\u{2028}'),
                            'P' => s.push('\u{2029}'),
                            'x' => code_length = 2,
                            'u' => code_length = 4,
                            'U' => code_length = 8,
                            // Any other character, and the NUL that ends the input in libyaml.
                            _ => {
                                return self.err(
                                    "while parsing a quoted scalar",
                                    start,
                                    "found unknown escape character",
                                );
                            }
                        }
                        self.skip();
                        self.skip();
                        if code_length != 0 {
                            self.cache(code_length)?;
                            let mut value: u32 = 0;
                            for k in 0..code_length {
                                let c = self.ch(k);
                                if !c.is_ascii_hexdigit() {
                                    return self.err(
                                        "while parsing a quoted scalar",
                                        start,
                                        HEX_ESCAPE_PROBLEM,
                                    );
                                }
                                value = value.wrapping_shl(4).wrapping_add(hex(c));
                            }
                            let Some(c) = char::from_u32(value) else {
                                return self.err(
                                    "while parsing a quoted scalar",
                                    start,
                                    "found invalid Unicode character escape code",
                                );
                            };
                            s.push(c);
                            for _ in 0..code_length {
                                self.skip();
                            }
                        }
                    } else {
                        self.read(&mut s);
                    }
                }
                self.cache(2)?;
            }
            self.cache(1)?;
            if self.ch(0) == quote {
                break;
            }
            self.cache(1)?;
            while is_blank(self.ch(0)) || is_break(self.ch(0)) {
                if is_blank(self.ch(0)) {
                    if leading_blanks {
                        self.skip();
                    } else {
                        self.read(&mut whitespaces);
                    }
                } else {
                    self.cache(2)?;
                    if leading_blanks {
                        self.read_line(&mut trailing_breaks);
                    } else {
                        whitespaces.clear();
                        self.read_line(&mut leading_break);
                        leading_blanks = true;
                    }
                }
                self.cache(1)?;
            }
            if leading_blanks {
                if leading_break.starts_with('\n') {
                    if trailing_breaks.is_empty() {
                        s.push(' ');
                    } else {
                        s.push_str(&trailing_breaks);
                        trailing_breaks.clear();
                    }
                    leading_break.clear();
                } else {
                    s.push_str(&leading_break);
                    s.push_str(&trailing_breaks);
                    leading_break.clear();
                    trailing_breaks.clear();
                }
            } else {
                s.push_str(&whitespaces);
                whitespaces.clear();
            }
        }
        self.skip();
        Ok(Token {
            kind: TokenKind::Scalar {
                value: s,
                style: if single { Style::Single } else { Style::Double },
            },
            start,
            end: self.mark,
        })
    }

    pub(super) fn scan_plain_scalar_reference(&mut self) -> R<Token> {
        // `None` while the value is still the input between `start` and `end` verbatim (no line folding yet):
        // it is then copied once instead of character by character.
        let mut s: Option<String> = None;
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        let mut whitespaces = String::new();
        let mut leading_blanks = false;
        let indent = self.indent.saturating_add(1);
        let start = self.mark;
        let mut end = self.mark;
        loop {
            self.cache(4)?;
            if self.mark.column == 0
                && ((self.ch(0) == '-' && self.ch(1) == '-' && self.ch(2) == '-')
                    || (self.ch(0) == '.' && self.ch(1) == '.' && self.ch(2) == '.'))
                && is_blankz(self.ch(3))
            {
                break;
            }
            if self.ch(0) == '#' {
                break;
            }
            let flow = self.flow_level != 0;
            loop {
                let c = self.ch(0);
                if is_blankz(c) {
                    break;
                }
                if flow && c == ':' && matches!(self.ch(1), ',' | '?' | '[' | ']' | '{' | '}') {
                    return self.err(
                        "while scanning a plain scalar",
                        start,
                        "found unexpected ':'",
                    );
                }
                if (c == ':' && is_blankz(self.ch(1)))
                    || (flow && matches!(c, ',' | '[' | ']' | '{' | '}'))
                {
                    break;
                }
                if leading_blanks {
                    let buf =
                        s.get_or_insert_with(|| self.reader.slice(start.byte, end.byte).to_owned());
                    if leading_break.starts_with('\n') {
                        if trailing_breaks.is_empty() {
                            buf.push(' ');
                        } else {
                            buf.push_str(&trailing_breaks);
                            trailing_breaks.clear();
                        }
                        leading_break.clear();
                    } else {
                        buf.push_str(&leading_break);
                        buf.push_str(&trailing_breaks);
                        leading_break.clear();
                        trailing_breaks.clear();
                    }
                    leading_blanks = false;
                } else if !whitespaces.is_empty() {
                    // Blanks inside a line are kept verbatim.
                    if let Some(buf) = s.as_mut() {
                        buf.push_str(&whitespaces);
                    }
                    whitespaces.clear();
                }
                match s.as_mut() {
                    Some(buf) => self.read(buf),
                    None => self.skip(),
                }
                end = self.mark;
                self.cache(2)?;
            }
            if !(is_blank(self.ch(0)) || is_break(self.ch(0))) {
                break;
            }
            self.cache(1)?;
            while is_blank(self.ch(0)) || is_break(self.ch(0)) {
                if is_blank(self.ch(0)) {
                    if leading_blanks && self.column_i64() < indent && self.ch(0) == '\t' {
                        return self.err(
                            "while scanning a plain scalar",
                            start,
                            "found a tab character that violates indentation",
                        );
                    } else if !leading_blanks {
                        self.read(&mut whitespaces);
                    } else {
                        self.skip();
                    }
                } else {
                    self.cache(2)?;
                    if leading_blanks {
                        self.read_line(&mut trailing_breaks);
                    } else {
                        whitespaces.clear();
                        self.read_line(&mut leading_break);
                        leading_blanks = true;
                    }
                }
                self.cache(1)?;
            }
            if self.flow_level == 0 && self.column_i64() < indent {
                break;
            }
        }
        if leading_blanks {
            self.simple_key_allowed = true;
        }
        let value = s.unwrap_or_else(|| self.reader.slice(start.byte, end.byte).to_owned());
        Ok(Token {
            kind: TokenKind::Scalar {
                value,
                style: Style::Plain,
            },
            start,
            end,
        })
    }
}
