//! `ruamel/yaml/parser.py` (`Parser`): the LL(1) state machine over the scanner's tokens, its directive and tag
//! handling, and `ruamel/yaml/tag.py`'s `Tag` (`trval`, `uri_decoded_suffix`, `check_handle`).
//!
//! Owner: WP-17.

use snp_pycompat::repr::repr_str;

use super::scanner::{DirValue, TokData, TokId, Token};
use super::{Fail, Loader, Mark, R, Ver, marked};

/// `Parser.DEFAULT_TAGS`.
fn default_tags() -> Vec<(String, String)> {
    vec![
        ("!".to_owned(), "!".to_owned()),
        ("!!".to_owned(), "tag:yaml.org,2002:".to_owned()),
    ]
}

/// The parser's `state` / `states` entries (bound methods).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum PState {
    StreamStart,
    ImplicitDocumentStart,
    DocumentStart,
    DocumentEnd,
    DocumentContent,
    BlockNode,
    BlockSequenceFirstEntry,
    BlockSequenceEntry,
    IndentlessSequenceEntry,
    BlockMappingFirstKey,
    BlockMappingKey,
    BlockMappingValue,
    FlowSequenceFirstEntry,
    FlowSequenceEntry,
    FlowSequenceEntryMappingKey,
    FlowSequenceEntryMappingValue,
    FlowSequenceEntryMappingEnd,
    FlowMappingFirstKey,
    FlowMappingKey,
    FlowMappingValue,
    FlowMappingEmptyValue,
}

/// Event classes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum EvId {
    StreamStart,
    StreamEnd,
    DocStart,
    DocEnd,
    Alias,
    Scalar,
    SeqStart,
    SeqEnd,
    MapStart,
    MapEnd,
}

/// An event; `tag` is the `Tag`'s `trval` (computed where Python first calls `str(tag)`).
#[derive(Debug, Clone)]
pub(super) struct Event {
    pub(super) id: EvId,
    pub(super) start: Mark,
    pub(super) end: Mark,
    pub(super) anchor: Option<String>,
    pub(super) tag: Option<String>,
    pub(super) implicit: (bool, bool),
    pub(super) value: Vec<u8>,
}

impl Event {
    fn new(id: EvId, start: Mark, end: Mark) -> Self {
        Self {
            id,
            start,
            end,
            anchor: None,
            tag: None,
            implicit: (false, false),
            value: Vec::new(),
        }
    }
}

/// Parser fields of [`Loader`] (`reset_parser`).
#[derive(Debug)]
pub(super) struct ParserState {
    current: Option<Event>,
    tag_handles: Vec<(String, String)>,
    states: Vec<PState>,
    marks: Vec<Mark>,
    state: Option<PState>,
}

impl Default for ParserState {
    fn default() -> Self {
        Self {
            current: None,
            tag_handles: Vec::new(),
            states: Vec::new(),
            marks: Vec::new(),
            state: Some(PState::StreamStart),
        }
    }
}

/// `int(s, 16)` for the (at most two-character) slice `uri_decoded_suffix` hands it, then `chr()`.
fn uri_char(s: &str) -> R<u32> {
    let err = || {
        Fail::crash(
            "ValueError",
            format!("invalid literal for int() with base 16: {}", repr_str(s)),
        )
    };
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, s.get(1..).unwrap_or_default()),
        Some(b'+') => (false, s.get(1..).unwrap_or_default()),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(err());
    }
    let v = u32::from_str_radix(digits, 16).map_err(|_| err())?;
    if neg && v != 0 {
        return Err(Fail::crash(
            "ValueError",
            "chr() arg not in range(0x110000)",
        ));
    }
    Ok(v)
}

/// `Tag.uri_decoded_suffix`: `%XX` → `chr(int(XX, 16))` (the scanner already decoded escapes; a `%` left in the
/// suffix came from `%25`).
fn uri_decoded(suffix: &str) -> R<String> {
    let chars: Vec<char> = suffix.chars().collect();
    let mut out = String::new();
    let mut idx = 0usize;
    while let Some(&ch) = chars.get(idx) {
        idx = idx.saturating_add(1);
        if ch == '%' {
            let end = idx.saturating_add(2).min(chars.len());
            let hex: String = chars.get(idx..end).unwrap_or_default().iter().collect();
            let cp = uri_char(&hex)?;
            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
            idx = idx.saturating_add(2);
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the loader's methods live next to the ruamel module they port"
)]
impl Loader<'_, '_> {
    /// `self.scanner` (the parser property → `YAML.scanner`) then `check_token(*choices)`.
    fn ck(&mut self, choices: &[TokId]) -> R<bool> {
        self.touch(2)?;
        self.check_token(choices)
    }

    fn pk(&mut self) -> R<Token> {
        self.touch(2)?;
        self.peek_token()?.ok_or_else(|| {
            Fail::crash(
                "AttributeError",
                "'NoneType' object has no attribute 'start_mark'",
            )
        })
    }

    fn gt(&mut self) -> R<Token> {
        self.touch(2)?;
        self.get_token()?.ok_or_else(|| {
            Fail::crash(
                "AttributeError",
                "'NoneType' object has no attribute 'start_mark'",
            )
        })
    }

    /// `self.resolver.processing_version` from the parser.
    fn p_pv(&mut self) -> R<Ver> {
        self.frame(Self::resolver_prop)?;
        self.touch(1)?;
        Ok(self.processing_version())
    }

    fn parser_err<T>(
        &mut self,
        context: Option<&str>,
        cm: Option<&Mark>,
        problem: &str,
        pm: &Mark,
    ) -> R<T> {
        self.touch(1)?;
        Err(marked(
            context,
            cm,
            Some(problem.as_bytes()),
            Some(pm),
            None,
        ))
    }

    fn pop_state(&mut self) -> R<Option<PState>> {
        match self.p.states.pop() {
            Some(s) => Ok(Some(s)),
            None => Err(Fail::crash("IndexError", "pop from empty list")),
        }
    }

    /// `check_event()` with no choices: whether an event remains.
    pub(super) fn check_event_any(&mut self) -> R<bool> {
        self.check_event(&[])
    }

    /// `Parser.check_event(*choices)`.
    pub(super) fn check_event(&mut self, choices: &[EvId]) -> R<bool> {
        self.frame(|s| {
            if s.p.current.is_none()
                && let Some(st) = s.p.state
            {
                let ev = s.run_state(st)?;
                s.p.current = Some(ev);
            }
            Ok(match &s.p.current {
                Some(e) => choices.is_empty() || choices.contains(&e.id),
                None => false,
            })
        })
    }

    /// `Parser.peek_event()`.
    pub(super) fn peek_event(&mut self) -> R<Option<Event>> {
        self.frame(|s| {
            if s.p.current.is_none()
                && let Some(st) = s.p.state
            {
                let ev = s.run_state(st)?;
                s.p.current = Some(ev);
            }
            Ok(s.p.current.clone())
        })
    }

    /// `Parser.get_event()`.
    pub(super) fn get_event(&mut self) -> R<Option<Event>> {
        self.frame(|s| {
            if s.p.current.is_none()
                && let Some(st) = s.p.state
            {
                let ev = s.run_state(st)?;
                s.p.current = Some(ev);
            }
            Ok(s.p.current.take())
        })
    }

    /// Calls the bound method `st` (one frame).
    fn run_state(&mut self, st: PState) -> R<Event> {
        self.frame(|s| s.state_body(st))
    }

    fn state_body(&mut self, st: PState) -> R<Event> {
        match st {
            PState::StreamStart => {
                let t = self.gt()?;
                self.touch(1)?; // move_token_comment
                self.touch(2)?;
                self.p.state = Some(PState::ImplicitDocumentStart);
                Ok(Event::new(EvId::StreamStart, t.start, t.end))
            }
            PState::ImplicitDocumentStart => {
                if self.ck(&[TokId::Directive, TokId::DocStart, TokId::StreamEnd])? {
                    return self.run_state(PState::DocumentStart);
                }
                self.p.tag_handles = default_tags();
                let t = self.pk()?;
                self.touch(2)?;
                let ev = Event::new(EvId::DocStart, t.start.clone(), t.start);
                self.p.states.push(PState::DocumentEnd);
                self.p.state = Some(PState::BlockNode);
                Ok(ev)
            }
            PState::DocumentStart => {
                while self.ck(&[TokId::DocEnd])? {
                    self.gt()?;
                }
                if self.ck(&[TokId::StreamEnd])? {
                    let t = self.gt()?;
                    self.touch(1)?; // token.comment
                    self.touch(1)?;
                    if !self.p.states.is_empty() || !self.p.marks.is_empty() {
                        return Err(Fail::crash("AssertionError", ""));
                    }
                    self.p.state = None;
                    return Ok(Event::new(EvId::StreamEnd, t.start, t.end));
                }
                self.frame(Self::process_directives)?;
                if !self.ck(&[TokId::DocStart])? {
                    let t = self.pk()?;
                    let p = format!(
                        "expected '<document start>', but found ({},)",
                        repr_str(t.id.id())
                    );
                    let t2 = self.pk()?;
                    return self.parser_err(None, None, &p, &t2.start);
                }
                let t = self.gt()?;
                self.touch(1)?;
                self.touch(2)?;
                self.p.states.push(PState::DocumentEnd);
                self.p.state = Some(PState::DocumentContent);
                Ok(Event::new(EvId::DocStart, t.start, t.end))
            }
            PState::DocumentEnd => {
                let t = self.pk()?;
                let start = t.start.clone();
                let mut end = t.start;
                let explicit = self.ck(&[TokId::DocEnd])?;
                if explicit {
                    let tok = self.gt()?;
                    let pt = self.pk()?;
                    if pt.id != TokId::StreamEnd && tok.end.line == pt.start.line {
                        let pt1 = self.pk()?;
                        let p = format!(
                            "found non-comment content after document end marker, ({},)",
                            repr_str(pt1.id.id())
                        );
                        let pt2 = self.pk()?;
                        return self.parser_err(None, None, &p, &pt2.start);
                    }
                    end = tok.end;
                }
                self.touch(2)?;
                let ev = Event::new(EvId::DocEnd, start, end);
                if self.p_pv()?.is11() || !explicit {
                    self.p.state = Some(PState::DocumentStart);
                } else {
                    self.p.state = Some(PState::ImplicitDocumentStart);
                }
                Ok(ev)
            }
            PState::DocumentContent => {
                if self.ck(&[
                    TokId::Directive,
                    TokId::DocStart,
                    TokId::DocEnd,
                    TokId::StreamEnd,
                ])? {
                    let t = self.pk()?;
                    let ev = self.empty_scalar(t.start)?;
                    self.p.state = self.pop_state()?;
                    Ok(ev)
                } else {
                    self.run_state(PState::BlockNode)
                }
            }
            PState::BlockNode => self.frame(|s| s.parse_node(true, false)),
            PState::BlockSequenceFirstEntry => {
                let t = self.gt()?;
                self.p.marks.push(t.start);
                self.run_state(PState::BlockSequenceEntry)
            }
            PState::BlockSequenceEntry => {
                if self.ck(&[TokId::BlockEntry])? {
                    let t = self.gt()?;
                    self.touch(1)?;
                    if !self.ck(&[TokId::BlockEntry, TokId::BlockEnd])? {
                        self.p.states.push(PState::BlockSequenceEntry);
                        return self.run_state(PState::BlockNode);
                    }
                    self.p.state = Some(PState::BlockSequenceEntry);
                    return self.empty_scalar(t.end);
                }
                if !self.ck(&[TokId::BlockEnd])? {
                    let t = self.pk()?;
                    let p = format!("expected <block end>, but found {}", repr_str(t.id.id()));
                    let m = self.p.marks.last().cloned();
                    return self.parser_err(
                        Some("while parsing a block collection"),
                        m.as_ref(),
                        &p,
                        &t.start,
                    );
                }
                let t = self.gt()?;
                self.touch(1)?;
                self.touch(1)?;
                self.p.state = self.pop_state()?;
                self.p.marks.pop();
                Ok(Event::new(EvId::SeqEnd, t.start, t.end))
            }
            PState::IndentlessSequenceEntry => {
                if self.ck(&[TokId::BlockEntry])? {
                    let t = self.gt()?;
                    self.touch(1)?;
                    if !self.ck(&[TokId::BlockEntry, TokId::Key, TokId::Value, TokId::BlockEnd])? {
                        self.p.states.push(PState::IndentlessSequenceEntry);
                        return self.run_state(PState::BlockNode);
                    }
                    self.p.state = Some(PState::IndentlessSequenceEntry);
                    return self.empty_scalar(t.end);
                }
                let t = self.pk()?;
                self.touch(1)?;
                self.touch(1)?;
                self.p.state = self.pop_state()?;
                Ok(Event::new(EvId::SeqEnd, t.start.clone(), t.start))
            }
            PState::BlockMappingFirstKey => {
                let t = self.gt()?;
                self.p.marks.push(t.start);
                self.run_state(PState::BlockMappingKey)
            }
            PState::BlockMappingKey => {
                if self.ck(&[TokId::Key])? {
                    let t = self.gt()?;
                    self.touch(1)?;
                    if !self.ck(&[TokId::Key, TokId::Value, TokId::BlockEnd])? {
                        self.p.states.push(PState::BlockMappingValue);
                        return self.frame(|s| s.frame(|s| s.parse_node(true, true)));
                    }
                    self.p.state = Some(PState::BlockMappingValue);
                    return self.empty_scalar(t.end);
                }
                if self.p_pv()?.gt11() && self.ck(&[TokId::Value])? {
                    self.p.state = Some(PState::BlockMappingValue);
                    let t = self.pk()?;
                    return self.empty_scalar(t.start);
                }
                if !self.ck(&[TokId::BlockEnd])? {
                    let t = self.pk()?;
                    let p = format!("expected <block end>, but found {}", repr_str(t.id.id()));
                    let m = self.p.marks.last().cloned();
                    return self.parser_err(
                        Some("while parsing a block mapping"),
                        m.as_ref(),
                        &p,
                        &t.start,
                    );
                }
                let t = self.gt()?;
                self.touch(1)?;
                self.touch(1)?;
                self.touch(1)?;
                self.p.state = self.pop_state()?;
                self.p.marks.pop();
                Ok(Event::new(EvId::MapEnd, t.start, t.end))
            }
            PState::BlockMappingValue => {
                if self.ck(&[TokId::Value])? {
                    self.gt()?;
                    // move_token_comment(token) when a value follows, move_token_comment(token, empty=True)
                    // unless a key does
                    if self.ck(&[TokId::Value])? || !self.ck(&[TokId::Key])? {
                        self.touch(1)?;
                    }
                    if !self.ck(&[TokId::Key, TokId::Value, TokId::BlockEnd])? {
                        self.p.states.push(PState::BlockMappingKey);
                        return self.frame(|s| s.frame(|s| s.parse_node(true, true)));
                    }
                    self.p.state = Some(PState::BlockMappingKey);
                    self.touch(1)?; // token.comment (a ValueToken never has one)
                    let pt = self.pk()?;
                    self.touch(1)?;
                    return self.empty_scalar(pt.end);
                }
                self.p.state = Some(PState::BlockMappingKey);
                let t = self.pk()?;
                self.empty_scalar(t.start)
            }
            PState::FlowSequenceFirstEntry => {
                let t = self.gt()?;
                self.p.marks.push(t.start);
                self.frame(|s| s.flow_sequence_entry(true))
            }
            PState::FlowSequenceEntry => self.flow_sequence_entry(false),
            PState::FlowSequenceEntryMappingKey => {
                let t = self.gt()?;
                if !self.ck(&[TokId::Value, TokId::FlowEntry, TokId::FlowSeqEnd])? {
                    self.p.states.push(PState::FlowSequenceEntryMappingValue);
                    return self.frame(|s| s.frame(|s| s.parse_node(false, false)));
                }
                self.p.state = Some(PState::FlowSequenceEntryMappingValue);
                self.empty_scalar(t.end)
            }
            PState::FlowSequenceEntryMappingValue => {
                if self.ck(&[TokId::Value])? {
                    let t = self.gt()?;
                    if !self.ck(&[TokId::FlowEntry, TokId::FlowSeqEnd])? {
                        self.p.states.push(PState::FlowSequenceEntryMappingEnd);
                        return self.frame(|s| s.frame(|s| s.parse_node(false, false)));
                    }
                    self.p.state = Some(PState::FlowSequenceEntryMappingEnd);
                    return self.empty_scalar(t.end);
                }
                self.p.state = Some(PState::FlowSequenceEntryMappingEnd);
                let t = self.pk()?;
                self.empty_scalar(t.start)
            }
            PState::FlowSequenceEntryMappingEnd => {
                self.p.state = Some(PState::FlowSequenceEntry);
                let t = self.pk()?;
                self.touch(1)?;
                Ok(Event::new(EvId::MapEnd, t.start.clone(), t.start))
            }
            PState::FlowMappingFirstKey => {
                let t = self.gt()?;
                self.p.marks.push(t.start);
                self.frame(|s| s.flow_mapping_key(true))
            }
            PState::FlowMappingKey => self.flow_mapping_key(false),
            PState::FlowMappingValue => {
                if self.ck(&[TokId::Value])? {
                    let t = self.gt()?;
                    if !self.ck(&[TokId::FlowEntry, TokId::FlowMapEnd])? {
                        self.p.states.push(PState::FlowMappingKey);
                        return self.frame(|s| s.frame(|s| s.parse_node(false, false)));
                    }
                    self.p.state = Some(PState::FlowMappingKey);
                    return self.empty_scalar(t.end);
                }
                self.p.state = Some(PState::FlowMappingKey);
                let t = self.pk()?;
                self.empty_scalar(t.start)
            }
            PState::FlowMappingEmptyValue => {
                self.p.state = Some(PState::FlowMappingKey);
                let t = self.pk()?;
                self.empty_scalar(t.start)
            }
        }
    }

    fn flow_sequence_entry(&mut self, first: bool) -> R<Event> {
        if !self.ck(&[TokId::FlowSeqEnd])? {
            if !first {
                if self.ck(&[TokId::FlowEntry])? {
                    self.gt()?;
                } else {
                    let t = self.pk()?;
                    let p = format!("expected ',' or ']', but got {}", repr_str(t.id.id()));
                    let m = self.p.marks.last().cloned();
                    return self.parser_err(
                        Some("while parsing a flow sequence"),
                        m.as_ref(),
                        &p,
                        &t.start,
                    );
                }
            }
            if self.ck(&[TokId::Key])? {
                let t = self.pk()?;
                self.touch(3)?;
                let mut ev = Event::new(EvId::MapStart, t.start, t.end);
                ev.implicit = (true, false);
                self.p.state = Some(PState::FlowSequenceEntryMappingKey);
                return Ok(ev);
            } else if !self.ck(&[TokId::FlowSeqEnd])? {
                self.p.states.push(PState::FlowSequenceEntry);
                return self.frame(|s| s.frame(|s| s.parse_node(false, false)));
            }
        }
        let t = self.gt()?;
        self.touch(1)?;
        self.touch(1)?;
        self.p.state = self.pop_state()?;
        self.p.marks.pop();
        Ok(Event::new(EvId::SeqEnd, t.start, t.end))
    }

    fn flow_mapping_key(&mut self, first: bool) -> R<Event> {
        if !self.ck(&[TokId::FlowMapEnd])? {
            if !first {
                if self.ck(&[TokId::FlowEntry])? {
                    self.gt()?;
                } else {
                    let t = self.pk()?;
                    let p = format!("expected ',' or '}}', but got {}", repr_str(t.id.id()));
                    let m = self.p.marks.last().cloned();
                    return self.parser_err(
                        Some("while parsing a flow mapping"),
                        m.as_ref(),
                        &p,
                        &t.start,
                    );
                }
            }
            if self.ck(&[TokId::Key])? {
                let t = self.gt()?;
                if !self.ck(&[TokId::Value, TokId::FlowEntry, TokId::FlowMapEnd])? {
                    self.p.states.push(PState::FlowMappingValue);
                    return self.frame(|s| s.frame(|s| s.parse_node(false, false)));
                }
                self.p.state = Some(PState::FlowMappingValue);
                return self.empty_scalar(t.end);
            } else if self.p_pv()?.gt11() && self.ck(&[TokId::Value])? {
                self.p.state = Some(PState::FlowMappingValue);
                let t = self.pk()?;
                return self.empty_scalar(t.end);
            } else if !self.ck(&[TokId::FlowMapEnd])? {
                self.p.states.push(PState::FlowMappingEmptyValue);
                return self.frame(|s| s.frame(|s| s.parse_node(false, false)));
            }
        }
        let t = self.gt()?;
        self.touch(1)?;
        self.touch(1)?;
        self.p.state = self.pop_state()?;
        self.p.marks.pop();
        Ok(Event::new(EvId::MapEnd, t.start, t.end))
    }

    /// `process_empty_scalar(mark)`.
    fn empty_scalar(&mut self, mark: Mark) -> R<Event> {
        self.frame(|s| {
            s.touch(3)?;
            let mut ev = Event::new(EvId::Scalar, mark.clone(), mark);
            ev.implicit = (true, false);
            Ok(ev)
        })
    }

    fn process_directives(&mut self) -> R<()> {
        let mut yaml_version: Option<Ver> = None;
        self.p.tag_handles = Vec::new();
        while self.ck(&[TokId::Directive])? {
            let t = self.gt()?;
            if let TokData::Directive(name, value) = &t.data {
                if name == "YAML" {
                    if yaml_version.is_some() {
                        return self.parser_err(
                            None,
                            None,
                            "found duplicate YAML directive",
                            &t.start,
                        );
                    }
                    if let Some(DirValue::Yaml(v)) = value {
                        if v.0 != "1" {
                            return self.parser_err(
                                None,
                                None,
                                "found incompatible YAML document (version 1.* is required)",
                                &t.start,
                            );
                        }
                        yaml_version = Some(v.clone());
                    }
                } else if name == "TAG"
                    && let Some(DirValue::Tag(handle, prefix)) = value
                {
                    if self.p.tag_handles.iter().any(|(h, _)| h == handle) {
                        let p = format!("duplicate tag handle {}", repr_str(handle));
                        return self.parser_err(None, None, &p, &t.start);
                    }
                    self.p.tag_handles.push((handle.clone(), prefix.clone()));
                }
            }
        }
        // `self.loader.version = yaml_version` (the YAML.version setter validates it)
        self.frame(|s| {
            if let Some(v) = &yaml_version {
                s.touch(1)?; // the generator expression of `tuple(int(x) for x in val)`
                if v.1 != "1" && v.1 != "2" {
                    return Err(Fail::crash(
                        "AssertionError",
                        format!("version minor part can only be 2 or 1, got {}", v.repr()),
                    ));
                }
            }
            s.st.version.clone_from(&yaml_version);
            Ok(())
        })?;
        self.touch(1)?; // the tags property
        for (k, v) in default_tags() {
            if !self.p.tag_handles.iter().any(|(h, _)| *h == k) {
                self.p.tag_handles.push((k, v));
            }
        }
        Ok(())
    }

    /// `Parser.parse_node(block, indentless_sequence)` (the caller pushed the frame).
    fn parse_node(&mut self, block: bool, indentless: bool) -> R<Event> {
        if self.ck(&[TokId::Alias])? {
            let t = self.gt()?;
            self.touch(3)?;
            let mut ev = Event::new(EvId::Alias, t.start, t.end);
            if let TokData::Name(n) = t.data {
                ev.anchor = Some(n);
            }
            self.p.state = self.pop_state()?;
            return Ok(ev);
        }
        let mut anchor: Option<String> = None;
        let mut tag: Option<(Option<String>, String)> = None;
        let mut start: Option<Mark> = None;
        let mut end: Option<Mark> = None;
        let mut tag_mark: Option<Mark> = None;
        if self.ck(&[TokId::Anchor])? {
            let t = self.gt()?;
            self.touch(1)?;
            start = Some(t.start.clone());
            end = Some(t.end.clone());
            if let TokData::Name(n) = t.data {
                anchor = Some(n);
            }
            if self.ck(&[TokId::Tag])? {
                let t = self.gt()?;
                tag_mark = Some(t.start.clone());
                end = Some(t.end.clone());
                self.touch(1)?; // Tag.__init__
                if let TokData::Tag(h, sfx) = t.data {
                    tag = Some((h, sfx));
                }
            }
        } else if self.ck(&[TokId::Tag])? {
            let t = self.gt()?;
            self.touch(1)?;
            start = Some(t.start.clone());
            tag_mark = Some(t.start.clone());
            end = Some(t.end.clone());
            self.touch(1)?;
            if let TokData::Tag(h, sfx) = t.data {
                tag = Some((h, sfx));
            }
            if self.ck(&[TokId::Anchor])? {
                let t = self.gt()?;
                start = Some(t.start.clone());
                tag_mark = Some(t.start.clone());
                end = Some(t.end.clone());
                if let TokData::Name(n) = t.data {
                    anchor = Some(n);
                }
            }
        }
        let trval: Option<String> = if let Some((handle, suffix)) = &tag {
            self.touch(2)?; // select_tag_transform -> Tag.select_transform
            self.touch(1)?; // check_handle
            if let Some(h) = handle
                && !self.p.tag_handles.iter().any(|(k, _)| k == h)
            {
                let p = format!("found undefined tag handle {}", repr_str(h));
                let tm = tag_mark.unwrap_or_else(|| self.mark_here());
                return self.parser_err(Some("while parsing a node"), start.as_ref(), &p, &tm);
            }
            // `str(tag)`: Tag.__str__ -> trval -> uri_decoded_suffix
            self.touch(3)?;
            let decoded = uri_decoded(suffix)?;
            Some(match handle {
                None => decoded,
                Some(h) => {
                    let prefix = self
                        .p
                        .tag_handles
                        .iter()
                        .find(|(k, _)| k == h)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    prefix + &decoded
                }
            })
        } else {
            None
        };
        let start = if let Some(m) = start {
            m
        } else {
            let t = self.pk()?;
            end = Some(t.start.clone());
            t.start
        };
        let end = end.unwrap_or_else(|| start.clone());
        let implicit = trval.as_deref().is_none_or(|t| t == "!");
        let make =
            |id: EvId, start: Mark, end: Mark, anchor: Option<String>, trval: Option<String>| {
                let mut ev = Event::new(id, start, end);
                ev.anchor = anchor;
                ev.tag = trval;
                ev.implicit = (implicit, false);
                ev
            };
        if indentless && self.ck(&[TokId::BlockEntry])? {
            let _pt = self.pk()?;
            self.touch(1)?; // pt.comment
            self.touch(1)?;
            let t = self.pk()?;
            self.touch(3)?;
            self.p.state = Some(PState::IndentlessSequenceEntry);
            return Ok(make(EvId::SeqStart, start, t.end, anchor, trval));
        }
        if self.ck(&[TokId::Scalar])? {
            let t = self.gt()?;
            let (value, plain) = match t.data {
                TokData::Scalar { value, plain, .. } => (value, plain),
                _ => (Vec::new(), false),
            };
            if trval.is_some() {
                self.touch(2)?; // str(tag)
            }
            let dimplicit = if plain && trval.is_none() || trval.as_deref() == Some("!") {
                (true, false)
            } else if trval.is_none() {
                (false, true)
            } else {
                (false, false)
            };
            self.touch(1)?; // token.comment
            self.touch(3)?;
            let mut ev = make(EvId::Scalar, start, t.end, anchor, trval);
            ev.implicit = dimplicit;
            ev.value = value;
            self.p.state = self.pop_state()?;
            return Ok(ev);
        }
        if self.ck(&[TokId::FlowSeqStart])? {
            let pt = self.pk()?;
            self.touch(1)?;
            self.touch(3)?;
            self.p.state = Some(PState::FlowSequenceFirstEntry);
            return Ok(make(EvId::SeqStart, start, pt.end, anchor, trval));
        }
        if self.ck(&[TokId::FlowMapStart])? {
            let pt = self.pk()?;
            self.touch(1)?;
            self.touch(3)?;
            self.p.state = Some(PState::FlowMappingFirstKey);
            return Ok(make(EvId::MapStart, start, pt.end, anchor, trval));
        }
        if block && self.ck(&[TokId::BlockSeqStart])? {
            let t = self.pk()?;
            let _pt = self.pk()?;
            self.touch(1)?; // pt.comment
            self.touch(2)?; // pt.split_old_comment() -> comment
            self.touch(3)?;
            self.p.state = Some(PState::BlockSequenceFirstEntry);
            return Ok(make(EvId::SeqStart, start, t.start, anchor, trval));
        }
        if block && self.ck(&[TokId::BlockMapStart])? {
            let t = self.pk()?;
            self.pk()?;
            self.touch(1)?;
            self.touch(3)?;
            self.p.state = Some(PState::BlockMappingFirstKey);
            return Ok(make(EvId::MapStart, start, t.start, anchor, trval));
        }
        if anchor.is_some() || trval.is_some() {
            self.touch(3)?;
            let ev = make(EvId::Scalar, start, end, anchor, trval);
            self.p.state = self.pop_state()?;
            return Ok(ev);
        }
        let node = if block { "block" } else { "flow" };
        let t = self.pk()?;
        let p = format!(
            "expected the node content, but found {}",
            repr_str(t.id.id())
        );
        self.parser_err(
            Some(&format!("while parsing a {node} node")),
            Some(&start),
            &p,
            &t.start,
        )
    }
}
