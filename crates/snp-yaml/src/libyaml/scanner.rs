//! libyaml 0.2.5's scanner (`scanner.c`): characters to tokens, with simple keys, indentation and flow levels.
//!
//! Function names and control flow follow libyaml (via libyaml-safer 0.3.0); see the module docs of
//! [`super`] for the fixed differences.
//!
//! Owner: WP-14.

use std::collections::VecDeque;

use super::{Error, Mark, Reader, Style};

/// `MAX_NUMBER_LENGTH` of `%YAML` version numbers.
const MAX_NUMBER_LENGTH: u32 = 9;

/// libyaml's text for a bad `\x`/`\u`/`\U` escape, spelling included (split so that spell checkers accept it).
const HEX_ESCAPE_PROBLEM: &str = concat!("did not find expected hex", "decimal number");

/// The token payloads (`yaml_token_type_t` + data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TokenKind {
    StreamStart,
    StreamEnd,
    VersionDirective {
        major: i32,
        minor: i32,
    },
    /// `%TAG handle prefix`; the prefix may hold `%XX` octets, so it is bytes.
    TagDirective {
        handle: Vec<u8>,
        prefix: Vec<u8>,
    },
    DocumentStart,
    DocumentEnd,
    BlockSequenceStart,
    BlockMappingStart,
    BlockEnd,
    FlowSequenceStart,
    FlowSequenceEnd,
    FlowMappingStart,
    FlowMappingEnd,
    BlockEntry,
    FlowEntry,
    Key,
    Value,
    Alias(String),
    Anchor(String),
    /// A tag; both parts may hold `%XX` octets.
    Tag {
        handle: Vec<u8>,
        suffix: Vec<u8>,
    },
    Scalar {
        value: String,
        style: Style,
    },
}

/// One token with its marks.
#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) start: Mark,
    pub(crate) end: Mark,
}

/// A potential simple key (`yaml_simple_key_t`).
#[derive(Copy, Clone, Debug, Default)]
struct SimpleKey {
    possible: bool,
    required: bool,
    token_number: usize,
    mark: Mark,
}

/// `IS_ALPHA`: `[0-9A-Za-z_-]`.
const fn is_alpha(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// `IS_BLANK`.
const fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// `IS_BREAK`.
const fn is_break(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// `IS_BREAKZ` (`Z` = the NUL terminator).
const fn is_breakz(c: char) -> bool {
    is_break(c) || c == '\0'
}

/// `IS_BLANKZ`.
const fn is_blankz(c: char) -> bool {
    is_blank(c) || is_breakz(c)
}

/// `AS_HEX`.
fn hex(c: char) -> u32 {
    c.to_digit(16).unwrap_or(0)
}

/// The scanner state (`yaml_parser_t`, scanner part).
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "libyaml's scanner state is a set of flags"
)]
pub(crate) struct Scanner<'a> {
    reader: Reader<'a>,
    mark: Mark,
    stream_start_produced: bool,
    stream_end_produced: bool,
    flow_level: usize,
    tokens: VecDeque<Token>,
    tokens_parsed: usize,
    token_available: bool,
    indents: Vec<i64>,
    indent: i64,
    simple_key_allowed: bool,
    simple_keys: Vec<SimpleKey>,
}

type R<T> = Result<T, Error>;

impl<'a> Scanner<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self {
            reader: Reader::new(text),
            mark: Mark::default(),
            stream_start_produced: false,
            stream_end_produced: false,
            flow_level: 0,
            tokens: VecDeque::new(),
            tokens_parsed: 0,
            token_available: false,
            indents: Vec::new(),
            indent: -1,
            simple_key_allowed: false,
            simple_keys: Vec::new(),
        }
    }

    // --- buffer primitives (libyaml's macros) -------------------------------------------------------------------

    fn cache(&mut self, n: usize) -> R<()> {
        self.reader.cache(n)
    }

    fn ch(&self, k: usize) -> char {
        self.reader.peek(k)
    }

    fn column_i64(&self) -> i64 {
        i64::try_from(self.mark.column).unwrap_or(i64::MAX)
    }

    /// `SKIP`.
    fn skip(&mut self) {
        self.reader.advance();
        self.mark.index = self.mark.index.saturating_add(1);
        self.mark.column = self.mark.column.saturating_add(1);
        self.mark.byte = self.reader.pos();
    }

    /// `SKIP_LINE`.
    fn skip_line(&mut self) {
        if self.ch(0) == '\r' && self.ch(1) == '\n' {
            self.reader.advance();
            self.reader.advance();
            self.mark.index = self.mark.index.saturating_add(2);
            self.mark.column = 0;
            self.mark.line = self.mark.line.saturating_add(1);
        } else if is_break(self.ch(0)) {
            self.reader.advance();
            self.mark.index = self.mark.index.saturating_add(1);
            self.mark.column = 0;
            self.mark.line = self.mark.line.saturating_add(1);
        }
        self.mark.byte = self.reader.pos();
    }

    /// `READ`.
    fn read(&mut self, s: &mut String) {
        let c = self.reader.advance();
        s.push(c);
        self.mark.index = self.mark.index.saturating_add(1);
        self.mark.column = self.mark.column.saturating_add(1);
        self.mark.byte = self.reader.pos();
    }

    /// `READ` into bytes (tag URIs).
    fn read_bytes(&mut self, s: &mut Vec<u8>) {
        let c = self.reader.advance();
        let mut buf = [0u8; 4];
        s.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        self.mark.index = self.mark.index.saturating_add(1);
        self.mark.column = self.mark.column.saturating_add(1);
        self.mark.byte = self.reader.pos();
    }

    /// `READ_LINE`: CR LF, CR, LF and NEL become `\n`; LS and PS are kept.
    fn read_line(&mut self, s: &mut String) {
        let c = self.ch(0);
        if c == '\r' && self.ch(1) == '\n' {
            s.push('\n');
            self.reader.advance();
            self.reader.advance();
            self.mark.index = self.mark.index.saturating_add(2);
            self.mark.column = 0;
            self.mark.line = self.mark.line.saturating_add(1);
        } else if is_break(c) {
            self.reader.advance();
            s.push(if c == '\u{2028}' || c == '\u{2029}' {
                c
            } else {
                '\n'
            });
            self.mark.index = self.mark.index.saturating_add(1);
            self.mark.column = 0;
            self.mark.line = self.mark.line.saturating_add(1);
        }
        self.mark.byte = self.reader.pos();
    }

    fn err<T>(&self, context: &'static str, context_mark: Mark, problem: &'static str) -> R<T> {
        Err(Error::scanner(context, context_mark, problem, self.mark))
    }

    // --- the token queue ----------------------------------------------------------------------------------------

    /// `PEEK_TOKEN`.
    pub(crate) fn peek(&mut self) -> R<&Token> {
        if !self.token_available {
            self.fetch_more_tokens()?;
        }
        self.tokens.front().ok_or(Error::Overflow)
    }

    /// `SKIP_TOKEN`, returning the token.
    pub(crate) fn take(&mut self) -> R<Token> {
        if !self.token_available {
            self.fetch_more_tokens()?;
        }
        self.token_available = false;
        self.tokens_parsed = self.tokens_parsed.saturating_add(1);
        let token = self.tokens.pop_front().ok_or(Error::Overflow)?;
        if token.kind == TokenKind::StreamEnd {
            self.stream_end_produced = true;
        }
        Ok(token)
    }

    /// Whether STREAM-END has been taken.
    pub(crate) const fn stream_end_produced(&self) -> bool {
        self.stream_end_produced
    }

    fn fetch_more_tokens(&mut self) -> R<()> {
        loop {
            let need_more = if self.tokens.is_empty() {
                true
            } else {
                self.stale_simple_keys()?;
                self.simple_keys
                    .iter()
                    .any(|sk| sk.possible && sk.token_number == self.tokens_parsed)
            };
            if !need_more {
                break;
            }
            if self.stream_end_produced && self.tokens.is_empty() {
                // Nothing follows STREAM-END (libyaml returns no token); keep returning STREAM-END.
                self.tokens.push_back(Token {
                    kind: TokenKind::StreamEnd,
                    start: self.mark,
                    end: self.mark,
                });
                break;
            }
            self.fetch_next_token()?;
        }
        self.token_available = true;
        Ok(())
    }

    fn fetch_next_token(&mut self) -> R<()> {
        self.cache(1)?;
        if !self.stream_start_produced {
            self.fetch_stream_start();
            return Ok(());
        }
        self.scan_to_next_token()?;
        self.stale_simple_keys()?;
        self.unroll_indent(self.column_i64());
        self.cache(4)?;
        let c = self.ch(0);
        if c == '\0' {
            return self.fetch_stream_end();
        }
        let column0 = self.mark.column == 0;
        if column0 && c == '%' {
            return self.fetch_directive();
        }
        if column0 && c == '-' && self.ch(1) == '-' && self.ch(2) == '-' && is_blankz(self.ch(3)) {
            return self.fetch_document_indicator(TokenKind::DocumentStart);
        }
        if column0 && c == '.' && self.ch(1) == '.' && self.ch(2) == '.' && is_blankz(self.ch(3)) {
            return self.fetch_document_indicator(TokenKind::DocumentEnd);
        }
        let flow = self.flow_level != 0;
        match c {
            '[' => return self.fetch_flow_collection_start(TokenKind::FlowSequenceStart),
            '{' => return self.fetch_flow_collection_start(TokenKind::FlowMappingStart),
            ']' => return self.fetch_flow_collection_end(TokenKind::FlowSequenceEnd),
            '}' => return self.fetch_flow_collection_end(TokenKind::FlowMappingEnd),
            ',' => return self.fetch_flow_entry(),
            '-' if is_blankz(self.ch(1)) => return self.fetch_block_entry(),
            '?' if flow || is_blankz(self.ch(1)) => return self.fetch_key(),
            ':' if flow || is_blankz(self.ch(1)) => return self.fetch_value(),
            '*' => return self.fetch_anchor(true),
            '&' => return self.fetch_anchor(false),
            '!' => return self.fetch_tag(),
            '|' if !flow => return self.fetch_block_scalar(true),
            '>' if !flow => return self.fetch_block_scalar(false),
            '\'' => return self.fetch_flow_scalar(true),
            '"' => return self.fetch_flow_scalar(false),
            _ => {}
        }
        let indicator = is_blankz(c)
            || matches!(
                c,
                '-' | '?'
                    | ':'
                    | ','
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '#'
                    | '&'
                    | '*'
                    | '!'
                    | '|'
                    | '>'
                    | '\''
                    | '"'
                    | '%'
                    | '@'
                    | '`'
            );
        if !indicator
            || (c == '-' && !is_blank(self.ch(1)))
            || (!flow && (c == '?' || c == ':') && !is_blankz(self.ch(1)))
        {
            return self.fetch_plain_scalar();
        }
        self.err(
            "while scanning for the next token",
            self.mark,
            "found character that cannot start any token",
        )
    }

    fn stale_simple_keys(&mut self) -> R<()> {
        let mark = self.mark;
        for sk in &mut self.simple_keys {
            if sk.possible
                && (sk.mark.line < mark.line || sk.mark.index.saturating_add(1024) < mark.index)
            {
                if sk.required {
                    return Err(Error::scanner(
                        "while scanning a simple key",
                        sk.mark,
                        "could not find expected ':'",
                        mark,
                    ));
                }
                sk.possible = false;
            }
        }
        Ok(())
    }

    fn save_simple_key(&mut self) -> R<()> {
        let required = self.flow_level == 0 && self.indent == self.column_i64();
        if self.simple_key_allowed {
            let sk = SimpleKey {
                possible: true,
                required,
                token_number: self.tokens_parsed.saturating_add(self.tokens.len()),
                mark: self.mark,
            };
            self.remove_simple_key()?;
            if let Some(last) = self.simple_keys.last_mut() {
                *last = sk;
            }
        }
        Ok(())
    }

    fn remove_simple_key(&mut self) -> R<()> {
        let mark = self.mark;
        if let Some(sk) = self.simple_keys.last_mut() {
            if sk.possible && sk.required {
                return Err(Error::scanner(
                    "while scanning a simple key",
                    sk.mark,
                    "could not find expected ':'",
                    mark,
                ));
            }
            sk.possible = false;
        }
        Ok(())
    }

    fn increase_flow_level(&mut self) {
        self.simple_keys.push(SimpleKey::default());
        self.flow_level = self.flow_level.saturating_add(1);
    }

    fn decrease_flow_level(&mut self) {
        if self.flow_level != 0 {
            self.flow_level = self.flow_level.saturating_sub(1);
            self.simple_keys.pop();
        }
    }

    /// `yaml_parser_roll_indent`: `number` is the token number to insert at, or `None` to append.
    fn roll_indent(&mut self, column: i64, number: Option<usize>, kind: TokenKind, mark: Mark) {
        if self.flow_level != 0 {
            return;
        }
        if self.indent < column {
            self.indents.push(self.indent);
            self.indent = column;
            let token = Token {
                kind,
                start: mark,
                end: mark,
            };
            match number {
                None => self.tokens.push_back(token),
                Some(n) => {
                    let at = n.saturating_sub(self.tokens_parsed);
                    if at <= self.tokens.len() {
                        self.tokens.insert(at, token);
                    } else {
                        self.tokens.push_back(token);
                    }
                }
            }
        }
    }

    fn unroll_indent(&mut self, column: i64) {
        if self.flow_level != 0 {
            return;
        }
        while self.indent > column {
            self.tokens.push_back(Token {
                kind: TokenKind::BlockEnd,
                start: self.mark,
                end: self.mark,
            });
            self.indent = self.indents.pop().unwrap_or(-1);
        }
    }

    fn push(&mut self, kind: TokenKind, start: Mark, end: Mark) {
        self.tokens.push_back(Token { kind, start, end });
    }

    fn fetch_stream_start(&mut self) {
        self.indent = -1;
        self.simple_keys.push(SimpleKey::default());
        self.simple_key_allowed = true;
        self.stream_start_produced = true;
        self.push(TokenKind::StreamStart, self.mark, self.mark);
    }

    fn fetch_stream_end(&mut self) -> R<()> {
        if self.mark.column != 0 {
            self.mark.column = 0;
            self.mark.line = self.mark.line.saturating_add(1);
        }
        self.unroll_indent(-1);
        self.remove_simple_key()?;
        self.simple_key_allowed = false;
        self.push(TokenKind::StreamEnd, self.mark, self.mark);
        Ok(())
    }

    fn fetch_directive(&mut self) -> R<()> {
        self.unroll_indent(-1);
        self.remove_simple_key()?;
        self.simple_key_allowed = false;
        let token = self.scan_directive()?;
        self.tokens.push_back(token);
        Ok(())
    }

    fn fetch_document_indicator(&mut self, kind: TokenKind) -> R<()> {
        self.unroll_indent(-1);
        self.remove_simple_key()?;
        self.simple_key_allowed = false;
        let start = self.mark;
        self.skip();
        self.skip();
        self.skip();
        self.push(kind, start, self.mark);
        Ok(())
    }

    fn fetch_flow_collection_start(&mut self, kind: TokenKind) -> R<()> {
        self.save_simple_key()?;
        self.increase_flow_level();
        self.simple_key_allowed = true;
        let start = self.mark;
        self.skip();
        self.push(kind, start, self.mark);
        Ok(())
    }

    fn fetch_flow_collection_end(&mut self, kind: TokenKind) -> R<()> {
        self.remove_simple_key()?;
        self.decrease_flow_level();
        self.simple_key_allowed = false;
        let start = self.mark;
        self.skip();
        self.push(kind, start, self.mark);
        Ok(())
    }

    fn fetch_flow_entry(&mut self) -> R<()> {
        self.remove_simple_key()?;
        self.simple_key_allowed = true;
        let start = self.mark;
        self.skip();
        self.push(TokenKind::FlowEntry, start, self.mark);
        Ok(())
    }

    fn fetch_block_entry(&mut self) -> R<()> {
        if self.flow_level == 0 {
            if !self.simple_key_allowed {
                return self.err(
                    "",
                    self.mark,
                    "block sequence entries are not allowed in this context",
                );
            }
            self.roll_indent(
                self.column_i64(),
                None,
                TokenKind::BlockSequenceStart,
                self.mark,
            );
        }
        self.remove_simple_key()?;
        self.simple_key_allowed = true;
        let start = self.mark;
        self.skip();
        self.push(TokenKind::BlockEntry, start, self.mark);
        Ok(())
    }

    fn fetch_key(&mut self) -> R<()> {
        if self.flow_level == 0 {
            if !self.simple_key_allowed {
                return self.err(
                    "",
                    self.mark,
                    "mapping keys are not allowed in this context",
                );
            }
            self.roll_indent(
                self.column_i64(),
                None,
                TokenKind::BlockMappingStart,
                self.mark,
            );
        }
        self.remove_simple_key()?;
        self.simple_key_allowed = self.flow_level == 0;
        let start = self.mark;
        self.skip();
        self.push(TokenKind::Key, start, self.mark);
        Ok(())
    }

    fn fetch_value(&mut self) -> R<()> {
        let sk = self.simple_keys.last().copied().unwrap_or_default();
        if sk.possible {
            let token = Token {
                kind: TokenKind::Key,
                start: sk.mark,
                end: sk.mark,
            };
            let at = sk.token_number.saturating_sub(self.tokens_parsed);
            if at <= self.tokens.len() {
                self.tokens.insert(at, token);
            } else {
                self.tokens.push_back(token);
            }
            self.roll_indent(
                i64::try_from(sk.mark.column).unwrap_or(i64::MAX),
                Some(sk.token_number),
                TokenKind::BlockMappingStart,
                sk.mark,
            );
            if let Some(last) = self.simple_keys.last_mut() {
                last.possible = false;
            }
            self.simple_key_allowed = false;
        } else {
            if self.flow_level == 0 {
                if !self.simple_key_allowed {
                    return self.err(
                        "",
                        self.mark,
                        "mapping values are not allowed in this context",
                    );
                }
                self.roll_indent(
                    self.column_i64(),
                    None,
                    TokenKind::BlockMappingStart,
                    self.mark,
                );
            }
            self.simple_key_allowed = self.flow_level == 0;
        }
        let start = self.mark;
        self.skip();
        self.push(TokenKind::Value, start, self.mark);
        Ok(())
    }

    fn fetch_anchor(&mut self, alias: bool) -> R<()> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let token = self.scan_anchor(alias)?;
        self.tokens.push_back(token);
        Ok(())
    }

    fn fetch_tag(&mut self) -> R<()> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let token = self.scan_tag()?;
        self.tokens.push_back(token);
        Ok(())
    }

    fn fetch_block_scalar(&mut self, literal: bool) -> R<()> {
        self.remove_simple_key()?;
        self.simple_key_allowed = true;
        let token = self.scan_block_scalar(literal)?;
        self.tokens.push_back(token);
        Ok(())
    }

    fn fetch_flow_scalar(&mut self, single: bool) -> R<()> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let token = self.scan_flow_scalar(single)?;
        self.tokens.push_back(token);
        Ok(())
    }

    fn fetch_plain_scalar(&mut self) -> R<()> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let token = self.scan_plain_scalar()?;
        self.tokens.push_back(token);
        Ok(())
    }

    fn scan_to_next_token(&mut self) -> R<()> {
        loop {
            self.cache(1)?;
            if self.mark.column == 0 && self.ch(0) == '\u{feff}' {
                self.skip();
            }
            self.cache(1)?;
            while self.ch(0) == ' '
                || ((self.flow_level != 0 || !self.simple_key_allowed) && self.ch(0) == '\t')
            {
                self.skip();
                self.cache(1)?;
            }
            if self.ch(0) == '#' {
                while !is_breakz(self.ch(0)) {
                    self.skip();
                    self.cache(1)?;
                }
            }
            if !is_break(self.ch(0)) {
                break;
            }
            self.cache(2)?;
            self.skip_line();
            if self.flow_level == 0 {
                self.simple_key_allowed = true;
            }
        }
        Ok(())
    }

    fn scan_directive(&mut self) -> R<Token> {
        let start = self.mark;
        self.skip();
        let name = self.scan_directive_name(start)?;
        let kind = if name == "YAML" {
            let (major, minor) = self.scan_version_directive_value(start)?;
            TokenKind::VersionDirective { major, minor }
        } else if name == "TAG" {
            let (handle, prefix) = self.scan_tag_directive_value(start)?;
            TokenKind::TagDirective { handle, prefix }
        } else {
            return self.err(
                "while scanning a directive",
                start,
                "found unknown directive name",
            );
        };
        let end = self.mark;
        self.cache(1)?;
        while is_blank(self.ch(0)) {
            self.skip();
            self.cache(1)?;
        }
        if self.ch(0) == '#' {
            while !is_breakz(self.ch(0)) {
                self.skip();
                self.cache(1)?;
            }
        }
        if !is_breakz(self.ch(0)) {
            return self.err(
                "while scanning a directive",
                start,
                "did not find expected comment or line break",
            );
        }
        if is_break(self.ch(0)) {
            self.cache(2)?;
            self.skip_line();
        }
        Ok(Token { kind, start, end })
    }

    fn scan_directive_name(&mut self, start: Mark) -> R<String> {
        let mut s = String::new();
        self.cache(1)?;
        while is_alpha(self.ch(0)) {
            self.read(&mut s);
            self.cache(1)?;
        }
        if s.is_empty() {
            return self.err(
                "while scanning a directive",
                start,
                "could not find expected directive name",
            );
        }
        if !is_blankz(self.ch(0)) {
            return self.err(
                "while scanning a directive",
                start,
                "found unexpected non-alphabetical character",
            );
        }
        Ok(s)
    }

    fn scan_version_directive_value(&mut self, start: Mark) -> R<(i32, i32)> {
        self.cache(1)?;
        while is_blank(self.ch(0)) {
            self.skip();
            self.cache(1)?;
        }
        let major = self.scan_version_directive_number(start)?;
        if self.ch(0) != '.' {
            return self.err(
                "while scanning a %YAML directive",
                start,
                "did not find expected digit or '.' character",
            );
        }
        self.skip();
        let minor = self.scan_version_directive_number(start)?;
        Ok((major, minor))
    }

    fn scan_version_directive_number(&mut self, start: Mark) -> R<i32> {
        let mut value: i32 = 0;
        let mut length: u32 = 0;
        self.cache(1)?;
        while let Some(d) = self.ch(0).to_digit(10) {
            length = length.saturating_add(1);
            if length > MAX_NUMBER_LENGTH {
                return self.err(
                    "while scanning a %YAML directive",
                    start,
                    "found extremely long version number",
                );
            }
            value = value
                .saturating_mul(10)
                .saturating_add(i32::try_from(d).unwrap_or(0));
            self.skip();
            self.cache(1)?;
        }
        if length == 0 {
            return self.err(
                "while scanning a %YAML directive",
                start,
                "did not find expected version number",
            );
        }
        Ok(value)
    }

    fn scan_tag_directive_value(&mut self, start: Mark) -> R<(Vec<u8>, Vec<u8>)> {
        self.cache(1)?;
        while is_blank(self.ch(0)) {
            self.skip();
            self.cache(1)?;
        }
        let handle = self.scan_tag_handle(true, start)?;
        self.cache(1)?;
        if !is_blank(self.ch(0)) {
            return self.err(
                "while scanning a %TAG directive",
                start,
                "did not find expected whitespace",
            );
        }
        while is_blank(self.ch(0)) {
            self.skip();
            self.cache(1)?;
        }
        let prefix = self.scan_tag_uri(true, true, None, start)?;
        self.cache(1)?;
        if !is_blankz(self.ch(0)) {
            return self.err(
                "while scanning a %TAG directive",
                start,
                "did not find expected whitespace or line break",
            );
        }
        Ok((handle, prefix))
    }

    fn scan_anchor(&mut self, alias: bool) -> R<Token> {
        let mut s = String::new();
        let start = self.mark;
        self.skip();
        self.cache(1)?;
        while is_alpha(self.ch(0)) {
            self.read(&mut s);
            self.cache(1)?;
        }
        let end = self.mark;
        let c = self.ch(0);
        if s.is_empty()
            || !(is_blankz(c) || matches!(c, '?' | ':' | ',' | ']' | '}' | '%' | '@' | '`'))
        {
            return self.err(
                if alias {
                    "while scanning an alias"
                } else {
                    "while scanning an anchor"
                },
                start,
                "did not find expected alphabetic or numeric character",
            );
        }
        Ok(Token {
            kind: if alias {
                TokenKind::Alias(s)
            } else {
                TokenKind::Anchor(s)
            },
            start,
            end,
        })
    }

    fn scan_tag(&mut self) -> R<Token> {
        let start = self.mark;
        self.cache(2)?;
        let (handle, suffix) = if self.ch(1) == '<' {
            self.skip();
            self.skip();
            let suffix = self.scan_tag_uri(true, false, None, start)?;
            if self.ch(0) != '>' {
                return self.err(
                    "while scanning a tag",
                    start,
                    "did not find the expected '>'",
                );
            }
            self.skip();
            (Vec::new(), suffix)
        } else {
            let handle = self.scan_tag_handle(false, start)?;
            if handle.len() > 1 && handle.first() == Some(&b'!') && handle.last() == Some(&b'!') {
                let suffix = self.scan_tag_uri(false, false, None, start)?;
                (handle, suffix)
            } else {
                let suffix = self.scan_tag_uri(false, false, Some(&handle), start)?;
                if suffix.is_empty() {
                    // The `!` tag: the handle is '' and the suffix '!'.
                    (Vec::new(), b"!".to_vec())
                } else {
                    (b"!".to_vec(), suffix)
                }
            }
        };
        self.cache(1)?;
        // A tag ends at a blank, a break, the end, or (in a flow collection) a `,`.
        if !is_blankz(self.ch(0)) && (self.flow_level == 0 || self.ch(0) != ',') {
            return self.err(
                "while scanning a tag",
                start,
                "did not find expected whitespace or line break",
            );
        }
        Ok(Token {
            kind: TokenKind::Tag { handle, suffix },
            start,
            end: self.mark,
        })
    }

    fn scan_tag_handle(&mut self, directive: bool, start: Mark) -> R<Vec<u8>> {
        let mut s = Vec::new();
        self.cache(1)?;
        if self.ch(0) != '!' {
            return self.err(
                if directive {
                    "while scanning a tag directive"
                } else {
                    "while scanning a tag"
                },
                start,
                "did not find expected '!'",
            );
        }
        self.read_bytes(&mut s);
        self.cache(1)?;
        while is_alpha(self.ch(0)) {
            self.read_bytes(&mut s);
            self.cache(1)?;
        }
        if self.ch(0) == '!' {
            self.read_bytes(&mut s);
        } else if directive && s != b"!" {
            return self.err(
                "while parsing a tag directive",
                start,
                "did not find expected '!'",
            );
        }
        Ok(s)
    }

    /// `yaml_parser_scan_tag_uri`; `head` (a scanned handle that was not one) is copied without its `!`.
    fn scan_tag_uri(
        &mut self,
        uri_char: bool,
        directive: bool,
        head: Option<&[u8]>,
        start: Mark,
    ) -> R<Vec<u8>> {
        let head = head.unwrap_or(&[]);
        let mut length = head.len();
        let mut s = Vec::new();
        if length > 1 {
            s.extend_from_slice(head.get(1..).unwrap_or(&[]));
        }
        self.cache(1)?;
        loop {
            let c = self.ch(0);
            let ok = is_alpha(c)
                || matches!(
                    c,
                    ';' | '/'
                        | '?'
                        | ':'
                        | '@'
                        | '&'
                        | '='
                        | '+'
                        | '$'
                        | '.'
                        | '%'
                        | '!'
                        | '~'
                        | '*'
                        | '\''
                        | '('
                        | ')'
                )
                || (uri_char && matches!(c, ',' | '[' | ']'));
            if !ok {
                break;
            }
            if c == '%' {
                self.scan_uri_escapes(directive, start, &mut s)?;
            } else {
                self.read_bytes(&mut s);
            }
            length = length.saturating_add(1);
            self.cache(1)?;
        }
        if length == 0 {
            return self.err(
                if directive {
                    "while parsing a %TAG directive"
                } else {
                    "while parsing a tag"
                },
                start,
                "did not find expected tag URI",
            );
        }
        Ok(s)
    }

    /// `yaml_parser_scan_uri_escapes`: the octets of one UTF-8 character.
    fn scan_uri_escapes(&mut self, directive: bool, start: Mark, s: &mut Vec<u8>) -> R<()> {
        let context = if directive {
            "while parsing a %TAG directive"
        } else {
            "while parsing a tag"
        };
        let mut width: u32 = 0;
        loop {
            self.cache(3)?;
            let (h, l) = (self.ch(1), self.ch(2));
            if !(self.ch(0) == '%' && h.is_ascii_hexdigit() && l.is_ascii_hexdigit()) {
                return self.err(context, start, "did not find URI escaped octet");
            }
            let octet = u8::try_from(hex(h).wrapping_shl(4) | hex(l)).unwrap_or(0);
            if width == 0 {
                width = if octet & 0x80 == 0 {
                    1
                } else if octet & 0xE0 == 0xC0 {
                    2
                } else if octet & 0xF0 == 0xE0 {
                    3
                } else if octet & 0xF8 == 0xF0 {
                    4
                } else {
                    0
                };
                if width == 0 {
                    return self.err(context, start, "found an incorrect leading UTF-8 octet");
                }
            } else if octet & 0xC0 != 0x80 {
                return self.err(context, start, "found an incorrect trailing UTF-8 octet");
            }
            s.push(octet);
            self.skip();
            self.skip();
            self.skip();
            width = width.saturating_sub(1);
            if width == 0 {
                return Ok(());
            }
        }
    }

    fn scan_block_scalar(&mut self, literal: bool) -> R<Token> {
        let mut s = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        let mut chomping: i8 = 0;
        let mut increment: i64 = 0;
        let mut indent: i64 = 0;
        let mut leading_blank = false;
        let start = self.mark;
        self.skip();
        self.cache(1)?;
        if matches!(self.ch(0), '+' | '-') {
            chomping = if self.ch(0) == '+' { 1 } else { -1 };
            self.skip();
            self.cache(1)?;
            if let Some(d) = self.ch(0).to_digit(10) {
                if d == 0 {
                    return self.err(
                        "while scanning a block scalar",
                        start,
                        "found an indentation indicator equal to 0",
                    );
                }
                increment = i64::from(d);
                self.skip();
            }
        } else if let Some(d) = self.ch(0).to_digit(10) {
            if d == 0 {
                return self.err(
                    "while scanning a block scalar",
                    start,
                    "found an indentation indicator equal to 0",
                );
            }
            increment = i64::from(d);
            self.skip();
            self.cache(1)?;
            if matches!(self.ch(0), '+' | '-') {
                chomping = if self.ch(0) == '+' { 1 } else { -1 };
                self.skip();
            }
        }
        self.cache(1)?;
        while is_blank(self.ch(0)) {
            self.skip();
            self.cache(1)?;
        }
        if self.ch(0) == '#' {
            while !is_breakz(self.ch(0)) {
                self.skip();
                self.cache(1)?;
            }
        }
        if !is_breakz(self.ch(0)) {
            return self.err(
                "while scanning a block scalar",
                start,
                "did not find expected comment or line break",
            );
        }
        if is_break(self.ch(0)) {
            self.cache(2)?;
            self.skip_line();
        }
        let mut end = self.mark;
        if increment != 0 {
            indent = if self.indent >= 0 {
                self.indent.saturating_add(increment)
            } else {
                increment
            };
        }
        self.scan_block_scalar_breaks(&mut indent, &mut trailing_breaks, start, &mut end)?;
        self.cache(1)?;
        while self.column_i64() == indent && self.ch(0) != '\0' {
            let trailing_blank = is_blank(self.ch(0));
            if !literal && leading_break.starts_with('\n') && !leading_blank && !trailing_blank {
                if trailing_breaks.is_empty() {
                    s.push(' ');
                }
            } else {
                s.push_str(&leading_break);
            }
            leading_break.clear();
            s.push_str(&trailing_breaks);
            trailing_breaks.clear();
            leading_blank = is_blank(self.ch(0));
            while !is_breakz(self.ch(0)) {
                self.read(&mut s);
                self.cache(1)?;
            }
            self.cache(2)?;
            self.read_line(&mut leading_break);
            self.scan_block_scalar_breaks(&mut indent, &mut trailing_breaks, start, &mut end)?;
        }
        if chomping != -1 {
            s.push_str(&leading_break);
        }
        if chomping == 1 {
            s.push_str(&trailing_breaks);
        }
        Ok(Token {
            kind: TokenKind::Scalar {
                value: s,
                style: if literal {
                    Style::Literal
                } else {
                    Style::Folded
                },
            },
            start,
            end,
        })
    }

    fn scan_block_scalar_breaks(
        &mut self,
        indent: &mut i64,
        breaks: &mut String,
        start: Mark,
        end: &mut Mark,
    ) -> R<()> {
        let mut max_indent: i64 = 0;
        *end = self.mark;
        loop {
            self.cache(1)?;
            while (*indent == 0 || self.column_i64() < *indent) && self.ch(0) == ' ' {
                self.skip();
                self.cache(1)?;
            }
            if self.column_i64() > max_indent {
                max_indent = self.column_i64();
            }
            if (*indent == 0 || self.column_i64() < *indent) && self.ch(0) == '\t' {
                return self.err(
                    "while scanning a block scalar",
                    start,
                    "found a tab character where an indentation space is expected",
                );
            }
            if !is_break(self.ch(0)) {
                break;
            }
            self.cache(2)?;
            self.read_line(breaks);
            *end = self.mark;
        }
        if *indent == 0 {
            *indent = max_indent.max(self.indent.saturating_add(1)).max(1);
        }
        Ok(())
    }

    fn scan_flow_scalar(&mut self, single: bool) -> R<Token> {
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

    fn scan_plain_scalar(&mut self) -> R<Token> {
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
