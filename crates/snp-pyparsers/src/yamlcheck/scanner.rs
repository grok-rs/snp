//! `ruamel/yaml/scanner.py` (`Scanner`, with the `comment_handling = None` paths of the safe loader), method for
//! method, including the simple-key bookkeeping that decides how far the scanner looks ahead.
//!
//! Owner: WP-17.

use std::collections::VecDeque;
use std::rc::Rc;

use snp_pycompat::repr::repr_str;

use super::{Fail, Loader, Mark, R, Ver, marked};
use crate::pystr;

/// `_THE_END`.
const THE_END: &str = "\n\0\r\u{85}\u{2028}\u{2029}";
/// `_THE_END_SPACE_TAB`.
const THE_END_SPACE_TAB: &str = " \n\0\t\r\u{85}\u{2028}\u{2029}";

fn is_in(c: char, set: &str) -> bool {
    set.contains(c)
}

/// The token classes, by their `id`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum TokId {
    Directive,
    DocStart,
    DocEnd,
    StreamStart,
    StreamEnd,
    BlockSeqStart,
    BlockMapStart,
    BlockEnd,
    FlowSeqStart,
    FlowMapStart,
    FlowSeqEnd,
    FlowMapEnd,
    Key,
    Value,
    BlockEntry,
    FlowEntry,
    Alias,
    Anchor,
    Tag,
    Scalar,
}

impl TokId {
    /// `Token.id`.
    pub(super) const fn id(self) -> &'static str {
        match self {
            Self::Directive => "<directive>",
            Self::DocStart => "<document start>",
            Self::DocEnd => "<document end>",
            Self::StreamStart => "<stream start>",
            Self::StreamEnd => "<stream end>",
            Self::BlockSeqStart => "<block sequence start>",
            Self::BlockMapStart => "<block mapping start>",
            Self::BlockEnd => "<block end>",
            Self::FlowSeqStart => "[",
            Self::FlowMapStart => "{",
            Self::FlowSeqEnd => "]",
            Self::FlowMapEnd => "}",
            Self::Key => "?",
            Self::Value => ":",
            Self::BlockEntry => "-",
            Self::FlowEntry => ",",
            Self::Alias => "<alias>",
            Self::Anchor => "<anchor>",
            Self::Tag => "<tag>",
            Self::Scalar => "<scalar>",
        }
    }
}

/// A directive's value.
#[derive(Debug, Clone)]
pub(super) enum DirValue {
    Yaml(Ver),
    Tag(String, String),
}

/// Token payloads.
#[derive(Debug, Clone)]
pub(super) enum TokData {
    None,
    Directive(String, Option<DirValue>),
    Name(String),
    Tag(Option<String>, String),
    Scalar { value: Vec<u8>, plain: bool },
}

/// A token; `comment` is whether ruamel attached a comment (only frame counts depend on it).
#[derive(Debug, Clone)]
pub(super) struct Token {
    pub(super) id: TokId,
    pub(super) start: Mark,
    pub(super) end: Mark,
    pub(super) data: TokData,
    pub(super) comment: bool,
}

impl Token {
    fn new(id: TokId, start: Mark, end: Mark) -> Self {
        Self {
            id,
            start,
            end,
            data: TokData::None,
            comment: false,
        }
    }
}

#[derive(Debug, Clone)]
struct SimpleKey {
    token_number: usize,
    required: bool,
    index: usize,
    line: usize,
    column: usize,
    mark: Mark,
}

/// Scanner fields of [`Loader`] (`reset_scanner`).
#[derive(Debug)]
pub(super) struct ScannerState {
    done: bool,
    flow_context: Vec<char>,
    pub(super) tokens: VecDeque<Token>,
    tokens_taken: usize,
    indent: i64,
    indents: Vec<i64>,
    allow_simple_key: bool,
    /// `possible_simple_keys` in dict insertion order (at most one per flow level; the current level's is
    /// searched from the back, where it is).
    possible: VecDeque<(usize, SimpleKey)>,
    pub(super) yaml_version: Option<Ver>,
}

impl Default for ScannerState {
    fn default() -> Self {
        Self {
            done: false,
            flow_context: Vec::new(),
            tokens: VecDeque::new(),
            tokens_taken: 0,
            indent: -1,
            indents: Vec::new(),
            allow_simple_key: true,
            possible: VecDeque::new(),
            yaml_version: None,
        }
    }
}

/// `int(digits)` of an ASCII digit string (normalised; the 4300-digit limit raises `ValueError`).
fn py_int_digits(digits: &str) -> R<String> {
    if digits.len() > 4300 {
        return Err(Fail::crash(
            "ValueError",
            format!(
                "Exceeds the limit (4300 digits) for integer string conversion: value has {} digits; use \
                 sys.set_int_max_str_digits() to increase the limit",
                digits.len()
            ),
        ));
    }
    let t = digits.trim_start_matches('0');
    Ok(if t.is_empty() {
        "0".to_owned()
    } else {
        t.to_owned()
    })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the loader's methods live next to the ruamel module they port"
)]
impl Loader<'_, '_> {
    /// The stream-start token `reset_scanner` queues.
    pub(super) fn scanner_start(&mut self) {
        let m = Mark {
            name: Rc::clone(&self.name),
            line: 0,
            column: 0,
        };
        self.sc
            .tokens
            .push_back(Token::new(TokId::StreamStart, m.clone(), m));
    }

    fn flow_level(&mut self) -> R<usize> {
        self.touch(1)?;
        Ok(self.sc.flow_context.len())
    }

    /// `Scanner.scanner_processing_version`.
    fn spv(&mut self) -> R<Ver> {
        self.frame(Self::resolver_pv)
    }

    fn scan_err(
        &mut self,
        context: Option<&str>,
        context_mark: Option<&Mark>,
        problem: &str,
        mark: &Mark,
        note: Option<&str>,
    ) -> R<Fail> {
        self.touch(1)?; // MarkedYAMLError.__init__
        Ok(marked(
            context,
            context_mark,
            Some(problem.as_bytes()),
            Some(mark),
            note,
        ))
    }

    /// `ScannerError(context, start_mark, problem, self.reader.get_mark())`.
    fn scan_err_here<T>(
        &mut self,
        context: Option<&str>,
        start: Option<&Mark>,
        problem: &str,
    ) -> R<T> {
        let m = self.get_mark()?;
        Err(self.scan_err(context, start, problem, &m, None)?)
    }

    /// `check_token(*choices)`: `None` choices = any token.
    pub(super) fn check_token(&mut self, choices: &[TokId]) -> R<bool> {
        self.frame(|s| {
            while s.need_more_tokens()? {
                s.fetch_more_tokens()?;
            }
            Ok(match s.sc.tokens.front() {
                Some(t) => choices.is_empty() || choices.contains(&t.id),
                None => false,
            })
        })
    }

    /// `peek_token()`.
    pub(super) fn peek_token(&mut self) -> R<Option<Token>> {
        self.frame(|s| {
            while s.need_more_tokens()? {
                s.fetch_more_tokens()?;
            }
            Ok(s.sc.tokens.front().cloned())
        })
    }

    /// `get_token()`.
    pub(super) fn get_token(&mut self) -> R<Option<Token>> {
        self.frame(|s| {
            while s.need_more_tokens()? {
                s.fetch_more_tokens()?;
            }
            let t = s.sc.tokens.pop_front();
            if t.is_some() {
                s.sc.tokens_taken = s.sc.tokens_taken.saturating_add(1);
            }
            Ok(t)
        })
    }

    fn need_more_tokens(&mut self) -> R<bool> {
        self.frame(|s| {
            if s.sc.done {
                return Ok(false);
            }
            if s.sc.tokens.is_empty() {
                return Ok(true);
            }
            s.stale_possible_simple_keys()?;
            Ok(s.next_possible_simple_key()? == Some(s.sc.tokens_taken))
        })
    }

    fn fetch_more_tokens(&mut self) -> R<()> {
        self.frame(|s| {
            s.scan_to_next_token()?;
            s.stale_possible_simple_keys()?;
            s.touch(1)?;
            let col = s.r.column;
            s.unwind_indent(i64::try_from(col).unwrap_or(i64::MAX))?;
            let ch = s.peek(0)?;
            if ch == '\0' {
                return s.fetch_stream_end();
            }
            if ch == '%' && s.check_directive()? {
                return s.fetch_directive();
            }
            if ch == '-' && s.check_document_start()? {
                return s.fetch_document_indicator(TokId::DocStart);
            }
            if ch == '.' && s.check_document_end()? {
                return s.fetch_document_indicator(TokId::DocEnd);
            }
            match ch {
                '[' => return s.frame(|s| s.fetch_flow_collection_start(TokId::FlowSeqStart, '[')),
                '{' => return s.frame(|s| s.fetch_flow_collection_start(TokId::FlowMapStart, '{')),
                ']' => return s.frame(|s| s.fetch_flow_collection_end(TokId::FlowSeqEnd)),
                '}' => return s.frame(|s| s.fetch_flow_collection_end(TokId::FlowMapEnd)),
                ',' => return s.fetch_flow_entry(),
                _ => {}
            }
            if ch == '-' && s.check_block_entry()? {
                return s.fetch_block_entry();
            }
            if ch == '?' && s.check_key()? {
                return s.fetch_key();
            }
            if ch == ':' && s.check_value()? {
                return s.fetch_value();
            }
            match ch {
                '*' => return s.fetch_anchor_like(TokId::Alias),
                '&' => return s.fetch_anchor_like(TokId::Anchor),
                '!' => return s.fetch_tag(),
                _ => {}
            }
            if ch == '|' && s.flow_level()? == 0 {
                return s.frame(|s| s.fetch_block_scalar('|'));
            }
            if ch == '>' && s.flow_level()? == 0 {
                return s.frame(|s| s.fetch_block_scalar('>'));
            }
            if ch == '\'' {
                return s.frame(|s| s.fetch_flow_scalar('\''));
            }
            if ch == '"' {
                return s.frame(|s| s.fetch_flow_scalar('"'));
            }
            if s.check_plain()? {
                return s.fetch_plain();
            }
            let problem = format!(
                "found character {} that cannot start any token",
                repr_str(&ch.to_string())
            );
            s.scan_err_here(Some("while scanning for the next token"), None, &problem)
        })
    }

    /// `next_possible_simple_key`: keys are kept in insertion (time) order, so their token numbers never
    /// decrease and the first one is the minimum.
    fn next_possible_simple_key(&mut self) -> R<Option<usize>> {
        self.frame(|s| Ok(s.sc.possible.front().map(|(_, k)| k.token_number)))
    }

    /// `stale_possible_simple_keys`. In insertion order the keys' lines and indices never decrease, so once a
    /// key is fresh every later one is too and the scan stops there (the keys Python would visit after it are
    /// all kept).
    fn stale_possible_simple_keys(&mut self) -> R<()> {
        self.frame(|s| {
            loop {
                if s.sc.possible.is_empty() {
                    break;
                }
                s.touch(1)?; // self.reader
                let Some((_, key)) = s.sc.possible.front() else {
                    break;
                };
                if key.line == s.r.line && s.r.index.saturating_sub(key.index) <= 1024 {
                    break;
                }
                if key.required {
                    let km = key.mark.clone();
                    return s.scan_err_here(
                        Some("while scanning a simple key"),
                        Some(&km),
                        "could not find expected ':'",
                    );
                }
                s.sc.possible.pop_front();
            }
            Ok(())
        })
    }

    fn save_possible_simple_key(&mut self) -> R<()> {
        self.frame(|s| {
            let fl = s.flow_level()?;
            let required = fl == 0 && s.sc.indent == i64::try_from(s.r.column).unwrap_or(i64::MAX);
            if s.sc.allow_simple_key {
                s.remove_possible_simple_key()?;
                let token_number = s.sc.tokens_taken.saturating_add(s.sc.tokens.len());
                let mark = s.get_mark()?;
                s.touch(1)?; // SimpleKey.__init__
                let key = SimpleKey {
                    token_number,
                    required,
                    index: s.r.index,
                    line: s.r.line,
                    column: s.r.column,
                    mark,
                };
                let fl = s.flow_level()?;
                if let Some(pos) = s.sc.possible.iter().rposition(|(l, _)| *l == fl) {
                    s.sc.possible.remove(pos);
                }
                s.sc.possible.push_back((fl, key));
            }
            Ok(())
        })
    }

    fn remove_possible_simple_key(&mut self) -> R<()> {
        self.frame(|s| {
            let fl = s.flow_level()?;
            if let Some(pos) = s.sc.possible.iter().rposition(|(l, _)| *l == fl) {
                let required = s.sc.possible.get(pos).is_some_and(|(_, k)| k.required);
                if required {
                    let km = s.sc.possible.get(pos).map(|(_, k)| k.mark.clone());
                    return s.scan_err_here(
                        Some("while scanning a simple key"),
                        km.as_ref(),
                        "could not find expected ':'",
                    );
                }
                s.sc.possible.remove(pos);
            }
            Ok(())
        })
    }

    fn unwind_indent(&mut self, column: i64) -> R<()> {
        self.frame(|s| {
            if s.flow_level()? != 0 {
                return Ok(());
            }
            while s.sc.indent > column {
                let mark = s.get_mark()?;
                s.sc.indent = s.sc.indents.pop().unwrap_or(-1);
                s.touch(1)?;
                s.sc.tokens
                    .push_back(Token::new(TokId::BlockEnd, mark.clone(), mark));
            }
            Ok(())
        })
    }

    fn add_indent(&mut self, column: usize) -> R<bool> {
        self.frame(|s| {
            let c = i64::try_from(column).unwrap_or(i64::MAX);
            if s.sc.indent < c {
                s.sc.indents.push(s.sc.indent);
                s.sc.indent = c;
                return Ok(true);
            }
            Ok(false)
        })
    }

    fn fetch_stream_end(&mut self) -> R<()> {
        self.frame(|s| {
            s.unwind_indent(-1)?;
            s.remove_possible_simple_key()?;
            s.sc.allow_simple_key = false;
            s.sc.possible.clear();
            let mark = s.get_mark()?;
            s.touch(1)?;
            s.sc.tokens
                .push_back(Token::new(TokId::StreamEnd, mark.clone(), mark));
            s.sc.done = true;
            Ok(())
        })
    }

    fn fetch_directive(&mut self) -> R<()> {
        self.frame(|s| {
            s.unwind_indent(-1)?;
            s.remove_possible_simple_key()?;
            s.sc.allow_simple_key = false;
            let t = s.scan_directive()?;
            s.sc.tokens.push_back(t);
            Ok(())
        })
    }

    /// `fetch_document_start` / `fetch_document_end` → `fetch_document_indicator`.
    fn fetch_document_indicator(&mut self, id: TokId) -> R<()> {
        self.frame(|s| {
            s.frame(|s| {
                s.unwind_indent(-1)?;
                s.remove_possible_simple_key()?;
                s.sc.allow_simple_key = false;
                let start = s.get_mark()?;
                s.forward(3)?;
                let end = s.get_mark()?;
                s.touch(1)?;
                s.sc.tokens.push_back(Token::new(id, start, end));
                Ok(())
            })
        })
    }

    fn fetch_flow_collection_start(&mut self, id: TokId, push: char) -> R<()> {
        self.frame(|s| {
            s.save_possible_simple_key()?;
            s.sc.flow_context.push(push);
            s.sc.allow_simple_key = true;
            let start = s.get_mark()?;
            s.fwd()?;
            let end = s.get_mark()?;
            s.touch(1)?;
            s.sc.tokens.push_back(Token::new(id, start, end));
            Ok(())
        })
    }

    fn fetch_flow_collection_end(&mut self, id: TokId) -> R<()> {
        self.frame(|s| {
            s.remove_possible_simple_key()?;
            s.sc.flow_context.pop();
            s.sc.allow_simple_key = false;
            let start = s.get_mark()?;
            s.fwd()?;
            let end = s.get_mark()?;
            s.touch(1)?;
            s.sc.tokens.push_back(Token::new(id, start, end));
            Ok(())
        })
    }

    fn fetch_flow_entry(&mut self) -> R<()> {
        self.frame(|s| {
            s.sc.allow_simple_key = true;
            s.remove_possible_simple_key()?;
            let start = s.get_mark()?;
            s.fwd()?;
            let end = s.get_mark()?;
            s.touch(1)?;
            s.sc.tokens
                .push_back(Token::new(TokId::FlowEntry, start, end));
            Ok(())
        })
    }

    fn fetch_block_entry(&mut self) -> R<()> {
        self.frame(|s| {
            if s.flow_level()? == 0 {
                if !s.sc.allow_simple_key {
                    let m = s.get_mark()?;
                    return Err(s.scan_err(
                        None,
                        None,
                        "sequence entries are not allowed here",
                        &m,
                        None,
                    )?);
                }
                let col = s.r.column;
                if s.add_indent(col)? {
                    let mark = s.get_mark()?;
                    s.touch(1)?;
                    s.sc.tokens
                        .push_back(Token::new(TokId::BlockSeqStart, mark.clone(), mark));
                }
            }
            s.sc.allow_simple_key = true;
            s.remove_possible_simple_key()?;
            let start = s.get_mark()?;
            s.fwd()?;
            let end = s.get_mark()?;
            s.touch(1)?;
            s.sc.tokens
                .push_back(Token::new(TokId::BlockEntry, start, end));
            Ok(())
        })
    }

    fn fetch_key(&mut self) -> R<()> {
        self.frame(|s| {
            if s.flow_level()? == 0 {
                if !s.sc.allow_simple_key {
                    let m = s.get_mark()?;
                    return Err(s.scan_err(
                        None,
                        None,
                        "mapping keys are not allowed here",
                        &m,
                        None,
                    )?);
                }
                let col = s.r.column;
                if s.add_indent(col)? {
                    let mark = s.get_mark()?;
                    s.touch(1)?;
                    s.sc.tokens
                        .push_back(Token::new(TokId::BlockMapStart, mark.clone(), mark));
                }
            }
            s.sc.allow_simple_key = s.flow_level()? == 0;
            s.remove_possible_simple_key()?;
            let start = s.get_mark()?;
            s.fwd()?;
            let end = s.get_mark()?;
            s.touch(1)?;
            s.sc.tokens.push_back(Token::new(TokId::Key, start, end));
            Ok(())
        })
    }

    fn fetch_value(&mut self) -> R<()> {
        self.frame(|s| {
            let fl = s.flow_level()?;
            s.touch(1)?;
            if let Some(pos) = s.sc.possible.iter().rposition(|(l, _)| *l == fl) {
                s.touch(1)?;
                let Some((_, key)) = s.sc.possible.remove(pos) else {
                    return Ok(());
                };
                s.touch(1)?;
                let at = key.token_number.saturating_sub(s.sc.tokens_taken);
                s.touch(1)?;
                let at = at.min(s.sc.tokens.len());
                s.sc.tokens.insert(
                    at,
                    Token::new(TokId::Key, key.mark.clone(), key.mark.clone()),
                );
                if s.flow_level()? == 0 && s.add_indent(key.column)? {
                    s.touch(1)?;
                    s.sc.tokens.insert(
                        at,
                        Token::new(TokId::BlockMapStart, key.mark.clone(), key.mark),
                    );
                }
                s.sc.allow_simple_key = false;
            } else {
                if s.flow_level()? == 0 && !s.sc.allow_simple_key {
                    let m = s.get_mark()?;
                    return Err(s.scan_err(
                        None,
                        None,
                        "mapping values are not allowed here",
                        &m,
                        None,
                    )?);
                }
                if s.flow_level()? == 0 {
                    let col = s.r.column;
                    if s.add_indent(col)? {
                        let mark = s.get_mark()?;
                        s.touch(1)?;
                        s.sc.tokens
                            .push_back(Token::new(TokId::BlockMapStart, mark.clone(), mark));
                    }
                }
                s.sc.allow_simple_key = s.flow_level()? == 0;
                s.remove_possible_simple_key()?;
            }
            let start = s.get_mark()?;
            s.fwd()?;
            let end = s.get_mark()?;
            s.touch(1)?;
            s.sc.tokens.push_back(Token::new(TokId::Value, start, end));
            Ok(())
        })
    }

    /// `fetch_alias` / `fetch_anchor`.
    fn fetch_anchor_like(&mut self, id: TokId) -> R<()> {
        self.frame(|s| {
            s.save_possible_simple_key()?;
            s.sc.allow_simple_key = false;
            let t = s.scan_anchor(id)?;
            s.sc.tokens.push_back(t);
            Ok(())
        })
    }

    fn fetch_tag(&mut self) -> R<()> {
        self.frame(|s| {
            s.save_possible_simple_key()?;
            s.sc.allow_simple_key = false;
            let t = s.scan_tag()?;
            s.sc.tokens.push_back(t);
            Ok(())
        })
    }

    /// `fetch_literal`/`fetch_folded` (the caller's frame) → `fetch_block_scalar`.
    fn fetch_block_scalar(&mut self, style: char) -> R<()> {
        self.frame(|s| {
            s.sc.allow_simple_key = true;
            s.remove_possible_simple_key()?;
            let t = s.scan_block_scalar(style)?;
            s.sc.tokens.push_back(t);
            Ok(())
        })
    }

    /// `fetch_single`/`fetch_double` (the caller's frame) → `fetch_flow_scalar`.
    fn fetch_flow_scalar(&mut self, style: char) -> R<()> {
        self.frame(|s| {
            s.save_possible_simple_key()?;
            s.sc.allow_simple_key = false;
            let t = s.scan_flow_scalar(style)?;
            s.sc.tokens.push_back(t);
            Ok(())
        })
    }

    fn fetch_plain(&mut self) -> R<()> {
        self.frame(|s| {
            s.save_possible_simple_key()?;
            s.sc.allow_simple_key = false;
            let t = s.scan_plain()?;
            s.sc.tokens.push_back(t);
            Ok(())
        })
    }

    fn check_directive(&mut self) -> R<bool> {
        self.frame(|s| {
            s.touch(1)?;
            Ok(s.r.column == 0)
        })
    }

    /// `check_document_start` (`'---'`) / `check_document_end` (`'...'`) as a Python call.
    fn check_doc_marker(&mut self, marker: &str) -> R<bool> {
        self.frame(|s| {
            s.touch(1)?;
            if s.r.column == 0 && s.prefix_is(marker)? {
                let c = s.peek(3)?;
                return Ok(is_in(c, THE_END_SPACE_TAB));
            }
            Ok(false)
        })
    }

    pub(super) fn check_document_start(&mut self) -> R<bool> {
        self.check_doc_marker("---")
    }

    pub(super) fn check_document_end(&mut self) -> R<bool> {
        self.check_doc_marker("...")
    }

    fn check_block_entry(&mut self) -> R<bool> {
        self.frame(|s| Ok(is_in(s.peek(1)?, THE_END_SPACE_TAB)))
    }

    fn check_key(&mut self) -> R<bool> {
        self.frame(|s| {
            if s.flow_level()? != 0 {
                return Ok(true);
            }
            Ok(is_in(s.peek(1)?, THE_END_SPACE_TAB))
        })
    }

    fn check_value(&mut self) -> R<bool> {
        self.frame(|s| {
            if s.spv()?.is11() {
                if s.flow_level()? != 0 {
                    return Ok(true);
                }
            } else if s.flow_level()? != 0 {
                if s.sc.flow_context.last() == Some(&'[') {
                    if !is_in(s.peek(1)?, THE_END_SPACE_TAB) {
                        return Ok(false);
                    }
                } else if s.sc.tokens.back().is_some_and(|t| t.id == TokId::Value)
                    && !is_in(s.peek(1)?, THE_END_SPACE_TAB)
                {
                    return Ok(false);
                }
                return Ok(true);
            }
            Ok(is_in(s.peek(1)?, THE_END_SPACE_TAB))
        })
    }

    fn check_plain(&mut self) -> R<bool> {
        const IND: &str = "\0 \t\r\n\u{85}\u{2028}\u{2029}-?:,[]{}#&*!|>'\"%@`";
        self.frame(|s| {
            let ch = s.peek(0)?;
            if s.spv()?.is11() {
                if !is_in(ch, IND) {
                    return Ok(true);
                }
                let c1 = s.peek(1)?;
                return Ok(!is_in(c1, THE_END_SPACE_TAB)
                    && (ch == '-' || s.flow_level()? == 0 && is_in(ch, "?:")));
            }
            if !is_in(ch, IND) {
                return Ok(true);
            }
            let ch1 = s.peek(1)?;
            if ch == '-' && !is_in(ch1, THE_END_SPACE_TAB) {
                return Ok(true);
            }
            if ch == ':' && s.flow_level()? != 0 && !is_in(ch1, " \t") {
                return Ok(true);
            }
            Ok(!is_in(s.peek(1)?, THE_END_SPACE_TAB)
                && (ch == '-' || s.flow_level()? == 0 && is_in(ch, "?:")))
        })
    }

    pub(super) fn scan_to_next_token(&mut self) -> R<()> {
        self.frame(|s| {
            s.touch(1)?;
            if s.r.index == 0 && s.peek(0)? == '\u{feff}' {
                s.fwd()?;
            }
            let white: &str = if s.flow_level()? > 0 { " \t" } else { " " };
            loop {
                while is_in(s.peek(0)?, white) {
                    s.fwd()?;
                }
                if s.peek(0)? == '#' {
                    while !is_in(s.peek(0)?, THE_END) {
                        s.fwd()?;
                    }
                }
                if s.scan_line_break()?.is_empty() {
                    return Ok(());
                }
                if s.flow_level()? == 0 {
                    s.sc.allow_simple_key = true;
                }
            }
        })
    }

    fn scan_directive(&mut self) -> R<Token> {
        self.frame(|s| {
            let start = s.get_mark()?;
            s.fwd()?;
            let name = s.scan_directive_name(&start)?;
            let value;
            let end;
            if name == "YAML" {
                value = Some(DirValue::Yaml(s.scan_yaml_directive_value(&start)?));
                end = s.get_mark()?;
            } else if name == "TAG" {
                let (h, p) = s.scan_tag_directive_value(&start)?;
                value = Some(DirValue::Tag(h, p));
                end = s.get_mark()?;
            } else {
                value = None;
                end = s.get_mark()?;
                while !is_in(s.peek(0)?, THE_END) {
                    s.fwd()?;
                }
            }
            s.scan_directive_ignored_line(&start)?;
            s.touch(2)?;
            let mut t = Token::new(TokId::Directive, start, end);
            t.data = TokData::Directive(name, value);
            Ok(t)
        })
    }

    fn scan_directive_name(&mut self, start: &Mark) -> R<String> {
        self.frame(|s| {
            let mut length = 0usize;
            let mut ch = s.peek(length)?;
            while ch.is_ascii_alphanumeric() || is_in(ch, "-_:.") {
                length = length.saturating_add(1);
                ch = s.peek(length)?;
            }
            if length == 0 {
                let p = format!(
                    "expected alphabetic or numeric character, but found {}",
                    repr_str(&ch.to_string())
                );
                return s.scan_err_here(Some("while scanning a directive"), Some(start), &p);
            }
            let value = s.prefix(length)?;
            s.forward(length)?;
            let ch = s.peek(0)?;
            if !is_in(ch, "\0 \r\n\u{85}\u{2028}\u{2029}") {
                let p = format!(
                    "expected alphabetic or numeric character, but found {}",
                    repr_str(&ch.to_string())
                );
                return s.scan_err_here(Some("while scanning a directive"), Some(start), &p);
            }
            Ok(value)
        })
    }

    fn scan_yaml_directive_value(&mut self, start: &Mark) -> R<Ver> {
        self.frame(|s| {
            while s.peek(0)? == ' ' {
                s.fwd()?;
            }
            let major = s.scan_yaml_directive_number(start)?;
            if s.peek(0)? != '.' {
                let c = s.peek(0)?;
                let p = format!(
                    "expected a digit or '.', but found {}",
                    repr_str(&c.to_string())
                );
                return s.scan_err_here(Some("while scanning a directive"), Some(start), &p);
            }
            s.fwd()?;
            let minor = s.scan_yaml_directive_number(start)?;
            if !is_in(s.peek(0)?, "\0 \r\n\u{85}\u{2028}\u{2029}") {
                let c = s.peek(0)?;
                let p = format!(
                    "expected a digit or '.', but found {}",
                    repr_str(&c.to_string())
                );
                return s.scan_err_here(Some("while scanning a directive"), Some(start), &p);
            }
            let v = Ver(major, minor);
            s.sc.yaml_version = Some(v.clone());
            s.touch(1)?; // Version(major, minor)
            Ok(v)
        })
    }

    fn scan_yaml_directive_number(&mut self, start: &Mark) -> R<String> {
        self.frame(|s| {
            let ch = s.peek(0)?;
            if !ch.is_ascii_digit() {
                let p = format!("expected a digit, but found {}", repr_str(&ch.to_string()));
                return s.scan_err_here(Some("while scanning a directive"), Some(start), &p);
            }
            let mut length = 0usize;
            while s.peek(length)?.is_ascii_digit() {
                length = length.saturating_add(1);
            }
            let digits = s.prefix(length)?;
            let value = py_int_digits(&digits)?;
            s.forward(length)?;
            Ok(value)
        })
    }

    fn scan_tag_directive_value(&mut self, start: &Mark) -> R<(String, String)> {
        self.frame(|s| {
            while s.peek(0)? == ' ' {
                s.fwd()?;
            }
            let handle = s.frame(|s| {
                let value = s.scan_tag_handle("directive", start)?;
                let ch = s.peek(0)?;
                if ch != ' ' {
                    let p = format!("expected ' ', but found {}", repr_str(&ch.to_string()));
                    return s.scan_err_here(Some("while scanning a directive"), Some(start), &p);
                }
                Ok(value)
            })?;
            while s.peek(0)? == ' ' {
                s.fwd()?;
            }
            let prefix = s.frame(|s| {
                let value = s.scan_tag_uri("directive", start)?;
                let ch = s.peek(0)?;
                if !is_in(ch, "\0 \r\n\u{85}\u{2028}\u{2029}") {
                    let p = format!("expected ' ', but found {}", repr_str(&ch.to_string()));
                    return s.scan_err_here(Some("while scanning a directive"), Some(start), &p);
                }
                Ok(value)
            })?;
            Ok((handle, prefix))
        })
    }

    fn scan_directive_ignored_line(&mut self, start: &Mark) -> R<()> {
        self.frame(|s| {
            while s.peek(0)? == ' ' {
                s.fwd()?;
            }
            if s.peek(0)? == '#' {
                while !is_in(s.peek(0)?, THE_END) {
                    s.fwd()?;
                }
            }
            let ch = s.peek(0)?;
            if !is_in(ch, THE_END) {
                let p = format!(
                    "expected a comment or a line break, but found {}",
                    repr_str(&ch.to_string())
                );
                return s.scan_err_here(Some("while scanning a directive"), Some(start), &p);
            }
            s.scan_line_break()?;
            Ok(())
        })
    }

    fn scan_anchor(&mut self, id: TokId) -> R<Token> {
        self.frame(|s| {
            let start = s.get_mark()?;
            let indicator = s.peek(0)?;
            let name = if indicator == '*' { "alias" } else { "anchor" };
            s.fwd()?;
            let mut length = 0usize;
            let mut ch = s.peek(length)?;
            while check_anchorname_char(ch) {
                s.touch(2)?; // check_anchorname_char -> check_namespace_char
                length = length.saturating_add(1);
                ch = s.peek(length)?;
            }
            s.touch(1)?;
            let ctx = format!("while scanning an {name}");
            if length == 0 {
                let p = format!(
                    "expected alphabetic or numeric character, but found {}",
                    repr_str(&ch.to_string())
                );
                return s.scan_err_here(Some(&ctx), Some(&start), &p);
            }
            let value = s.prefix(length)?;
            s.forward(length)?;
            if !is_in(ch, "\0 \t\r\n\u{85}\u{2028}\u{2029}?:,[]{}%@`") {
                let p = format!(
                    "expected alphabetic or numeric character, but found {}",
                    repr_str(&ch.to_string())
                );
                return s.scan_err_here(Some(&ctx), Some(&start), &p);
            }
            let end = s.get_mark()?;
            s.touch(2)?;
            let mut t = Token::new(id, start, end);
            t.data = TokData::Name(value);
            Ok(t)
        })
    }

    fn scan_tag(&mut self) -> R<Token> {
        self.frame(|s| {
            let start = s.get_mark()?;
            let mut ch = s.peek(1)?;
            let mut short_handle = "!";
            if ch == '!' {
                short_handle = "!!";
                s.fwd()?;
                ch = s.peek(1)?;
            }
            let handle: Option<String>;
            let suffix: String;
            if ch == '<' {
                handle = None;
                s.forward(2)?;
                suffix = s.scan_tag_uri("tag", &start)?;
                if s.peek(0)? != '>' {
                    let c = s.peek(0)?;
                    let p = format!("expected '>' but found {}", repr_str(&c.to_string()));
                    return s.scan_err_here(Some("while parsing a tag"), Some(&start), &p);
                }
                s.fwd()?;
            } else if is_in(ch, THE_END_SPACE_TAB) {
                handle = None;
                suffix = short_handle.to_owned();
                s.fwd()?;
            } else {
                let mut length = 1usize;
                let mut use_handle = false;
                while !is_in(ch, "\0 \r\n\u{85}\u{2028}\u{2029}") {
                    if ch == '!' {
                        use_handle = true;
                        break;
                    }
                    length = length.saturating_add(1);
                    ch = s.peek(length)?;
                }
                if use_handle {
                    handle = Some(s.scan_tag_handle("tag", &start)?);
                } else {
                    handle = Some(short_handle.to_owned());
                    s.fwd()?;
                }
                suffix = s.scan_tag_uri("tag", &start)?;
            }
            let ch = s.peek(0)?;
            if !is_in(ch, "\0 \r\n\u{85}\u{2028}\u{2029}") {
                let p = format!("expected ' ', but found {}", repr_str(&ch.to_string()));
                return s.scan_err_here(Some("while scanning a tag"), Some(&start), &p);
            }
            let end = s.get_mark()?;
            s.touch(2)?;
            let mut t = Token::new(TokId::Tag, start, end);
            t.data = TokData::Tag(handle, suffix);
            Ok(t)
        })
    }

    fn scan_block_scalar(&mut self, style: char) -> R<Token> {
        self.frame(|s| {
            let folded = style == '>';
            let mut chunks: Vec<u8> = Vec::new();
            let start = s.get_mark()?;
            s.fwd()?;
            let (chomping, increment) = s.scan_block_scalar_indicators(&start)?;
            let block_scalar_comment = s.scan_block_scalar_ignored_line(&start)?;
            let mut min_indent = s.sc.indent.saturating_add(1);
            let indent: i64;
            let mut breaks: Vec<String>;
            let mut end;
            if let Some(inc) = increment {
                if min_indent < 1 {
                    min_indent = 1;
                }
                indent = min_indent.saturating_add(inc).saturating_sub(1);
                (breaks, end) = s.scan_block_scalar_breaks(indent)?;
            } else {
                // `min_indent < 1 and (style not in '|>' or (spv == (1, 1)) and getattr(…, False))`: the style is
                // always '|' or '>' and the attribute is unset, so only the property call remains.
                if min_indent < 1 {
                    s.spv()?;
                }
                let max_indent;
                (breaks, max_indent, end) = s.scan_block_scalar_indentation()?;
                indent = min_indent.max(max_indent);
            }
            let mut line_break = String::new();
            loop {
                s.touch(1)?;
                let col = i64::try_from(s.r.column).unwrap_or(i64::MAX);
                if !(col == indent && s.peek(0)? != '\0') {
                    break;
                }
                for b in &breaks {
                    chunks.extend_from_slice(b.as_bytes());
                }
                let leading_non_space = !is_in(s.peek(0)?, " \t");
                let mut length = 0usize;
                while !is_in(s.peek(length)?, THE_END) {
                    length = length.saturating_add(1);
                }
                let text = s.prefix(length)?;
                chunks.extend_from_slice(text.as_bytes());
                s.forward(length)?;
                line_break = s.scan_line_break()?;
                (breaks, end) = s.scan_block_scalar_breaks(indent)?;
                if min_indent == 0 && (s.check_document_start()? || s.check_document_end()?) {
                    break;
                }
                s.touch(1)?;
                let col = i64::try_from(s.r.column).unwrap_or(i64::MAX);
                if col == indent && s.peek(0)? != '\0' {
                    if folded
                        && line_break == "\n"
                        && leading_non_space
                        && !is_in(s.peek(0)?, " \t")
                    {
                        if breaks.is_empty() {
                            chunks.push(b' ');
                        }
                    } else {
                        chunks.extend_from_slice(line_break.as_bytes());
                    }
                } else {
                    break;
                }
            }
            let mut trailing: Vec<String> = Vec::new();
            if chomping != Some(false) {
                chunks.extend_from_slice(line_break.as_bytes());
            }
            if chomping == Some(true) {
                for b in &breaks {
                    chunks.extend_from_slice(b.as_bytes());
                }
            } else {
                trailing.extend(breaks);
            }
            s.touch(2)?; // ScalarToken.__init__ -> Token.__init__
            let mut token = Token::new(TokId::Scalar, start, end);
            token.data = TokData::Scalar {
                value: chunks,
                plain: false,
            };
            if block_scalar_comment {
                s.touch(1)?; // add_pre_comments
                token.comment = true;
            }
            if !trailing.is_empty() {
                // `comment = self.scan_to_next_token()` (always None for the base scanner) and the post comment
                s.scan_to_next_token()?;
                s.get_mark()?;
                s.touch(2)?; // CommentToken.__init__ -> Token.__init__
                s.touch(1)?; // add_post_comment
                token.comment = true;
            }
            Ok(token)
        })
    }

    fn scan_block_scalar_indicators(&mut self, start: &Mark) -> R<(Option<bool>, Option<i64>)> {
        self.frame(|s| {
            let mut chomping = None;
            let mut increment = None;
            let mut ch = s.peek(0)?;
            let zero = |s: &mut Self| -> R<(Option<bool>, Option<i64>)> {
                s.scan_err_here(
                    Some("while scanning a block scalar"),
                    Some(start),
                    "expected indentation indicator in the range 1-9, but found 0",
                )
            };
            if is_in(ch, "+-") {
                chomping = Some(ch == '+');
                s.fwd()?;
                ch = s.peek(0)?;
                if let Some(d) = ch.to_digit(10) {
                    if d == 0 {
                        return zero(s);
                    }
                    increment = Some(i64::from(d));
                    s.fwd()?;
                }
            } else if let Some(d) = ch.to_digit(10) {
                if d == 0 {
                    return zero(s);
                }
                increment = Some(i64::from(d));
                s.fwd()?;
                ch = s.peek(0)?;
                if is_in(ch, "+-") {
                    chomping = Some(ch == '+');
                    s.fwd()?;
                }
            }
            let ch = s.peek(0)?;
            if !is_in(ch, "\0 \r\n\u{85}\u{2028}\u{2029}") {
                let p = format!(
                    "expected chomping or indentation indicators, but found {}",
                    repr_str(&ch.to_string())
                );
                return s.scan_err_here(Some("while scanning a block scalar"), Some(start), &p);
            }
            Ok((chomping, increment))
        })
    }

    /// Whether a comment was found on the indicator line.
    fn scan_block_scalar_ignored_line(&mut self, start: &Mark) -> R<bool> {
        self.frame(|s| {
            let mut comment = false;
            while s.peek(0)? == ' ' {
                s.peek(0)?;
                s.fwd()?;
            }
            if s.peek(0)? == '#' {
                comment = true;
                while !is_in(s.peek(0)?, THE_END) {
                    s.peek(0)?;
                    s.fwd()?;
                }
            }
            let ch = s.peek(0)?;
            if !is_in(ch, THE_END) {
                let p = format!(
                    "expected a comment or a line break, but found {}",
                    repr_str(&ch.to_string())
                );
                return s.scan_err_here(Some("while scanning a block scalar"), Some(start), &p);
            }
            s.scan_line_break()?;
            Ok(comment)
        })
    }

    fn scan_block_scalar_indentation(&mut self) -> R<(Vec<String>, i64, Mark)> {
        self.frame(|s| {
            let mut chunks = Vec::new();
            let mut first_indent: i64 = -1;
            let mut max_indent: i64 = 0;
            let mut end = s.get_mark()?;
            while is_in(s.peek(0)?, " \r\n\u{85}\u{2028}\u{2029}") {
                if s.peek(0)? == ' ' {
                    s.fwd()?;
                    s.touch(1)?;
                    let col = i64::try_from(s.r.column).unwrap_or(i64::MAX);
                    if col > max_indent {
                        max_indent = col;
                    }
                } else {
                    if first_indent < 0 {
                        s.touch(1)?;
                        first_indent = i64::try_from(s.r.column).unwrap_or(i64::MAX);
                    }
                    chunks.push(s.scan_line_break()?);
                    end = s.get_mark()?;
                }
            }
            if first_indent > 0 && max_indent > first_indent {
                let m = s.get_mark()?;
                s.touch(1)?;
                return Err(marked(
                    Some("more indented follow up line than first in a block scalar"),
                    Some(&m),
                    None,
                    None,
                    None,
                ));
            }
            Ok((chunks, max_indent, end))
        })
    }

    fn scan_block_scalar_breaks(&mut self, indent: i64) -> R<(Vec<String>, Mark)> {
        self.frame(|s| {
            let mut chunks = Vec::new();
            let mut end = s.get_mark()?;
            let col = |s: &Self| i64::try_from(s.r.column).unwrap_or(i64::MAX);
            while {
                s.touch(1)?;
                col(s) < indent
            } && s.peek(0)? == ' '
            {
                s.fwd()?;
            }
            while is_in(s.peek(0)?, "\r\n\u{85}\u{2028}\u{2029}") {
                chunks.push(s.scan_line_break()?);
                end = s.get_mark()?;
                while {
                    s.touch(1)?;
                    col(s) < indent
                } && s.peek(0)? == ' '
                {
                    s.fwd()?;
                }
            }
            Ok((chunks, end))
        })
    }

    fn scan_flow_scalar(&mut self, style: char) -> R<Token> {
        self.frame(|s| {
            let double = style == '"';
            let mut chunks: Vec<u8> = Vec::new();
            let start = s.get_mark()?;
            let quote = s.peek(0)?;
            s.fwd()?;
            s.scan_flow_scalar_non_spaces(double, &start, &mut chunks)?;
            while s.peek(0)? != quote {
                s.scan_flow_scalar_spaces(double, &start, &mut chunks)?;
                s.scan_flow_scalar_non_spaces(double, &start, &mut chunks)?;
            }
            s.fwd()?;
            let end = s.get_mark()?;
            s.touch(2)?;
            let mut t = Token::new(TokId::Scalar, start, end);
            t.data = TokData::Scalar {
                value: chunks,
                plain: false,
            };
            Ok(t)
        })
    }

    fn scan_flow_scalar_non_spaces(
        &mut self,
        double: bool,
        start: &Mark,
        out: &mut Vec<u8>,
    ) -> R<()> {
        self.frame(|s| {
            loop {
                let mut length = 0usize;
                while !is_in(s.peek(length)?, " \n'\"\\\0\t\r\u{85}\u{2028}\u{2029}") {
                    length = length.saturating_add(1);
                }
                if length != 0 {
                    let t = s.prefix(length)?;
                    out.extend_from_slice(t.as_bytes());
                    s.forward(length)?;
                }
                let ch = s.peek(0)?;
                if !double && ch == '\'' && s.peek(1)? == '\'' {
                    out.push(b'\'');
                    s.forward(2)?;
                } else if double && ch == '\'' || !double && is_in(ch, "\"\\") {
                    let mut b = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                    s.fwd()?;
                } else if double && ch == '\\' {
                    s.fwd()?;
                    let ch = s.peek(0)?;
                    if let Some(rep) = escape_replacement(ch) {
                        pystr::push_cp(out, rep);
                        s.fwd()?;
                    } else if let Some(length) = escape_code_len(ch) {
                        s.fwd()?;
                        for k in 0..length {
                            let c = s.peek(k)?;
                            if !c.is_ascii_hexdigit() {
                                let p = format!(
                                    "expected escape sequence of {length} hexdecimal numbers, but found {}",
                                    repr_str(&c.to_string())
                                );
                                return s.scan_err_here(
                                    Some("while scanning a double-quoted scalar"),
                                    Some(start),
                                    &p,
                                );
                            }
                        }
                        let hex = s.prefix(length)?;
                        let code = u64::from_str_radix(&hex, 16).unwrap_or(u64::MAX);
                        if code > 0x10_FFFF {
                            return Err(Fail::crash("ValueError", "chr() arg not in range(0x110000)"));
                        }
                        pystr::push_cp(out, u32::try_from(code).unwrap_or(0xFFFD));
                        s.forward(length)?;
                    } else if is_in(ch, "\n\r\u{85}\u{2028}\u{2029}") {
                        s.scan_line_break()?;
                        let breaks = s.scan_flow_scalar_breaks(start)?;
                        for b in breaks {
                            out.extend_from_slice(b.as_bytes());
                        }
                    } else {
                        let p = format!("found unknown escape character {}", repr_str(&ch.to_string()));
                        return s.scan_err_here(Some("while scanning a double-quoted scalar"), Some(start), &p);
                    }
                } else {
                    return Ok(());
                }
            }
        })
    }

    fn scan_flow_scalar_spaces(&mut self, _double: bool, start: &Mark, out: &mut Vec<u8>) -> R<()> {
        self.frame(|s| {
            let mut length = 0usize;
            while is_in(s.peek(length)?, " \t") {
                length = length.saturating_add(1);
            }
            let whitespaces = s.prefix(length)?;
            s.forward(length)?;
            let ch = s.peek(0)?;
            if ch == '\0' {
                return s.scan_err_here(
                    Some("while scanning a quoted scalar"),
                    Some(start),
                    "found unexpected end of stream",
                );
            } else if is_in(ch, "\r\n\u{85}\u{2028}\u{2029}") {
                let line_break = s.scan_line_break()?;
                let breaks = s.scan_flow_scalar_breaks(start)?;
                if line_break != "\n" {
                    out.extend_from_slice(line_break.as_bytes());
                } else if breaks.is_empty() {
                    out.push(b' ');
                }
                for b in breaks {
                    out.extend_from_slice(b.as_bytes());
                }
            } else {
                out.extend_from_slice(whitespaces.as_bytes());
            }
            Ok(())
        })
    }

    fn scan_flow_scalar_breaks(&mut self, start: &Mark) -> R<Vec<String>> {
        self.frame(|s| {
            let mut chunks = Vec::new();
            loop {
                let prefix = s.prefix(3)?;
                if (prefix == "---" || prefix == "...") && is_in(s.peek(3)?, THE_END_SPACE_TAB) {
                    return s.scan_err_here(
                        Some("while scanning a quoted scalar"),
                        Some(start),
                        "found unexpected document separator",
                    );
                }
                while is_in(s.peek(0)?, " \t") {
                    s.fwd()?;
                }
                if is_in(s.peek(0)?, "\r\n\u{85}\u{2028}\u{2029}") {
                    chunks.push(s.scan_line_break()?);
                } else {
                    return Ok(chunks);
                }
            }
        })
    }

    fn scan_plain(&mut self) -> R<Token> {
        self.frame(|s| {
            let mut chunks: Vec<u8> = Vec::new();
            let start = s.get_mark()?;
            let mut end = start.clone();
            let indent = s.sc.indent.saturating_add(1);
            let mut spaces: Option<Vec<String>> = Some(Vec::new());
            loop {
                let mut length = 0usize;
                if s.peek(0)? == '#' {
                    break;
                }
                let mut ch;
                loop {
                    ch = s.peek(length)?;
                    let pass = ch == ':' && !is_in(s.peek(length.saturating_add(1))?, THE_END_SPACE_TAB)
                        || ch == '?' && !s.spv()?.is11();
                    if !pass
                        && (is_in(ch, THE_END_SPACE_TAB)
                        || s.flow_level()? == 0
                            && ch == ':'
                            && is_in(s.peek(length.saturating_add(1))?, THE_END_SPACE_TAB)
                        || s.flow_level()? != 0 && is_in(ch, ",:?[]{}"))
                    {
                        break;
                    }
                    length = length.saturating_add(1);
                }
                if s.flow_level()? != 0
                    && ch == ':'
                    && !is_in(
                        s.peek(length.saturating_add(1))?,
                        "\0 \t\r\n\u{85}\u{2028}\u{2029},[]{}",
                    )
                {
                    s.forward(length)?;
                    let m = s.get_mark()?;
                    return Err(s.scan_err(
                        Some("while scanning a plain scalar"),
                        Some(&start),
                        "found unexpected ':'",
                        &m,
                        Some("Please check http://pyyaml.org/wiki/YAMLColonInFlowContext for details."),
                    )?);
                }
                if length == 0 {
                    break;
                }
                s.sc.allow_simple_key = false;
                if let Some(sp) = &spaces {
                    for x in sp {
                        chunks.extend_from_slice(x.as_bytes());
                    }
                }
                let text = s.prefix(length)?;
                chunks.extend_from_slice(text.as_bytes());
                s.forward(length)?;
                end = s.get_mark()?;
                spaces = s.scan_plain_spaces(&start)?;
                let stop = match &spaces {
                    None => true,
                    Some(sp) => sp.is_empty(),
                };
                if stop || s.peek(0)? == '#' || {
                    let fl = s.flow_level()?;
                    s.touch(1)?;
                    fl == 0 && i64::try_from(s.r.column).unwrap_or(i64::MAX) < indent
                } {
                    break;
                }
            }
            s.touch(2)?;
            let mut token = Token::new(TokId::Scalar, start, end);
            token.data = TokData::Scalar {
                value: chunks,
                plain: true,
            };
            if spaces.as_ref().is_some_and(|sp| sp.first().is_some_and(|x| x == "\n")) {
                s.touch(2)?; // CommentToken(...)
                s.touch(1)?; // add_post_comment
                token.comment = true;
            }
            Ok(token)
        })
    }

    /// `scan_plain_spaces`: `None` is Python's bare `return` (a document marker follows the line break).
    fn scan_plain_spaces(&mut self, _start: &Mark) -> R<Option<Vec<String>>> {
        self.frame(|s| {
            let mut chunks = Vec::new();
            let mut length = 0usize;
            while s.peek(length)? == ' ' {
                length = length.saturating_add(1);
            }
            let whitespaces = s.prefix(length)?;
            s.forward(length)?;
            let ch = s.peek(0)?;
            if is_in(ch, "\r\n\u{85}\u{2028}\u{2029}") {
                let line_break = s.scan_line_break()?;
                s.sc.allow_simple_key = true;
                let prefix = s.prefix(3)?;
                if (prefix == "---" || prefix == "...") && is_in(s.peek(3)?, THE_END_SPACE_TAB) {
                    return Ok(None);
                }
                let mut breaks = Vec::new();
                while is_in(s.peek(0)?, " \r\n\u{85}\u{2028}\u{2029}") {
                    if s.peek(0)? == ' ' {
                        s.fwd()?;
                    } else {
                        breaks.push(s.scan_line_break()?);
                        let prefix = s.prefix(3)?;
                        if (prefix == "---" || prefix == "...")
                            && is_in(s.peek(3)?, THE_END_SPACE_TAB)
                        {
                            return Ok(None);
                        }
                    }
                }
                if line_break != "\n" {
                    chunks.push(line_break);
                } else if breaks.is_empty() {
                    chunks.push(" ".to_owned());
                }
                chunks.extend(breaks);
            } else if !whitespaces.is_empty() {
                chunks.push(whitespaces);
            }
            Ok(Some(chunks))
        })
    }

    fn scan_tag_handle(&mut self, name: &str, start: &Mark) -> R<String> {
        self.frame(|s| {
            let ch = s.peek(0)?;
            let ctx = format!("while scanning an {name}");
            if ch != '!' {
                let p = format!("expected '!', but found {}", repr_str(&ch.to_string()));
                return s.scan_err_here(Some(&ctx), Some(start), &p);
            }
            let mut length = 1usize;
            let mut ch = s.peek(length)?;
            if ch != ' ' {
                while ch.is_ascii_alphanumeric() || is_in(ch, "-_") {
                    length = length.saturating_add(1);
                    ch = s.peek(length)?;
                }
                if ch != '!' {
                    s.forward(length)?;
                    let p = format!("expected '!' but found {}", repr_str(&ch.to_string()));
                    return s.scan_err_here(Some(&ctx), Some(start), &p);
                }
                length = length.saturating_add(1);
            }
            let value = s.prefix(length)?;
            s.forward(length)?;
            Ok(value)
        })
    }

    fn scan_tag_uri(&mut self, name: &str, start: &Mark) -> R<String> {
        self.frame(|s| {
            let mut chunks = String::new();
            let mut any = false;
            let mut length = 0usize;
            let mut ch = s.peek(length)?;
            loop {
                let ok = ch.is_ascii_alphanumeric() || is_in(ch, "-;/?:@&=+$,_.!~*'()[]%") || {
                    let v = s.spv()?.gt11();
                    v && ch == '#'
                };
                if !ok {
                    break;
                }
                if ch == '%' {
                    chunks.push_str(&s.prefix(length)?);
                    s.forward(length)?;
                    length = 0;
                    chunks.push_str(&s.scan_uri_escapes(name, start)?);
                    any = true;
                } else {
                    length = length.saturating_add(1);
                }
                ch = s.peek(length)?;
            }
            if length != 0 {
                chunks.push_str(&s.prefix(length)?);
                s.forward(length)?;
                any = true;
            }
            if !any {
                let p = format!("expected URI, but found {}", repr_str(&ch.to_string()));
                return s.scan_err_here(Some(&format!("while parsing an {name}")), Some(start), &p);
            }
            Ok(chunks)
        })
    }

    fn scan_uri_escapes(&mut self, name: &str, start: &Mark) -> R<String> {
        self.frame(|s| {
            let mut code_bytes = Vec::new();
            let mark = s.get_mark()?;
            while s.peek(0)? == '%' {
                s.fwd()?;
                for k in 0..2 {
                    let c = s.peek(k)?;
                    if !c.is_ascii_hexdigit() {
                        let p = format!(
                            "expected URI escape sequence of 2 hexdecimal numbers, but found {}",
                            repr_str(&c.to_string())
                        );
                        return s.scan_err_here(
                            Some(&format!("while scanning an {name}")),
                            Some(start),
                            &p,
                        );
                    }
                }
                let hex = s.prefix(2)?;
                code_bytes.push(u8::from_str_radix(&hex, 16).unwrap_or(0));
                s.forward(2)?;
            }
            match snp_pycompat::codec::decode_utf8(&code_bytes) {
                Ok(v) => Ok(v.to_owned()),
                Err(e) => {
                    s.touch(1)?;
                    Err(marked(
                        Some(&format!("while scanning an {name}")),
                        Some(start),
                        Some(e.to_string().as_bytes()),
                        Some(&mark),
                        None,
                    ))
                }
            }
        })
    }

    pub(super) fn scan_line_break(&mut self) -> R<String> {
        self.frame(|s| {
            let ch = s.peek(0)?;
            if is_in(ch, "\r\n\u{85}") {
                if s.prefix_is("\r\n")? {
                    s.forward(2)?;
                } else {
                    s.fwd()?;
                }
                return Ok("\n".to_owned());
            } else if is_in(ch, "\u{2028}\u{2029}") {
                s.fwd()?;
                return Ok(ch.to_string());
            }
            Ok(String::new())
        })
    }
}

/// `compat.check_anchorname_char`.
fn check_anchorname_char(ch: char) -> bool {
    if is_in(ch, ",[]{}") {
        return false;
    }
    matches!(ch, '\x21'..='\x7e' | '\u{a0}'..='\u{d7ff}' | '\u{10000}'..='\u{10ffff}')
        || matches!(ch, '\u{e000}'..='\u{fffd}') && ch != '\u{feff}'
}

/// `Scanner.ESCAPE_REPLACEMENTS`.
fn escape_replacement(ch: char) -> Option<u32> {
    Some(match ch {
        '0' => 0,
        'a' => 7,
        'b' => 8,
        't' | '\t' => 9,
        'n' => 10,
        'v' => 11,
        'f' => 12,
        'r' => 13,
        'e' => 0x1b,
        ' ' => 0x20,
        '"' => 0x22,
        '/' => 0x2f,
        '\\' => 0x5c,
        'N' => 0x85,
        '_' => 0xa0,
        'L' => 0x2028,
        'P' => 0x2029,
        _ => return None,
    })
}

/// `Scanner.ESCAPE_CODES`.
const fn escape_code_len(ch: char) -> Option<usize> {
    match ch {
        'x' => Some(2),
        'u' => Some(4),
        'U' => Some(8),
        _ => None,
    }
}
