//! libyaml 0.2.5's parser (`parser.c`): tokens to events, as a state machine with an explicit state stack (no
//! recursion, whatever the nesting depth).
//!
//! Owner: WP-14.

use super::scanner::{Scanner, Token, TokenKind};
use super::{Error, Mark, Style};

/// The event payloads (`yaml_event_type_t` + data). Tags are bytes (`%XX` escapes); the composer decodes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EventKind {
    StreamStart,
    StreamEnd,
    DocumentStart,
    DocumentEnd,
    Alias(String),
    Scalar {
        anchor: Option<String>,
        tag: Option<Vec<u8>>,
        value: String,
        /// libyaml's `plain_implicit`: a plain scalar without a tag, or any scalar tagged `!`.
        plain_implicit: bool,
        style: Style,
    },
    SequenceStart {
        anchor: Option<String>,
        tag: Option<Vec<u8>>,
    },
    SequenceEnd,
    MappingStart {
        anchor: Option<String>,
        tag: Option<Vec<u8>>,
    },
    MappingEnd,
}

/// One event with its marks.
#[derive(Debug, Clone)]
pub(crate) struct Event {
    pub(crate) kind: EventKind,
    pub(crate) start: Mark,
    pub(crate) end: Mark,
}

/// `yaml_parser_state_t`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum State {
    StreamStart,
    ImplicitDocumentStart,
    DocumentStart,
    DocumentContent,
    DocumentEnd,
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
    End,
}

/// A `%TAG` directive.
#[derive(Debug, Clone)]
struct TagDirective {
    handle: Vec<u8>,
    prefix: Vec<u8>,
}

type R<T> = Result<T, Error>;

/// The parser (`yaml_parser_t`, parser part).
#[derive(Debug)]
pub(crate) struct Parser<'a> {
    scanner: Scanner<'a>,
    state: State,
    states: Vec<State>,
    marks: Vec<Mark>,
    tag_directives: Vec<TagDirective>,
}

/// The kind of the next token, without its payload.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Kind {
    StreamStart,
    StreamEnd,
    VersionDirective,
    TagDirective,
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
    Alias,
    Anchor,
    Tag,
    Scalar,
}

const fn kind_of(t: &TokenKind) -> Kind {
    match t {
        TokenKind::StreamStart => Kind::StreamStart,
        TokenKind::StreamEnd => Kind::StreamEnd,
        TokenKind::VersionDirective { .. } => Kind::VersionDirective,
        TokenKind::TagDirective { .. } => Kind::TagDirective,
        TokenKind::DocumentStart => Kind::DocumentStart,
        TokenKind::DocumentEnd => Kind::DocumentEnd,
        TokenKind::BlockSequenceStart => Kind::BlockSequenceStart,
        TokenKind::BlockMappingStart => Kind::BlockMappingStart,
        TokenKind::BlockEnd => Kind::BlockEnd,
        TokenKind::FlowSequenceStart => Kind::FlowSequenceStart,
        TokenKind::FlowSequenceEnd => Kind::FlowSequenceEnd,
        TokenKind::FlowMappingStart => Kind::FlowMappingStart,
        TokenKind::FlowMappingEnd => Kind::FlowMappingEnd,
        TokenKind::BlockEntry => Kind::BlockEntry,
        TokenKind::FlowEntry => Kind::FlowEntry,
        TokenKind::Key => Kind::Key,
        TokenKind::Value => Kind::Value,
        TokenKind::Alias(_) => Kind::Alias,
        TokenKind::Anchor(_) => Kind::Anchor,
        TokenKind::Tag { .. } => Kind::Tag,
        TokenKind::Scalar { .. } => Kind::Scalar,
    }
}

impl<'a> Parser<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self {
            scanner: Scanner::new(text),
            state: State::StreamStart,
            states: Vec::new(),
            marks: Vec::new(),
            tag_directives: Vec::new(),
        }
    }

    /// `PEEK_TOKEN`: the kind and marks of the next token.
    fn peek(&mut self) -> R<(Kind, Mark, Mark)> {
        let t = self.scanner.peek()?;
        Ok((kind_of(&t.kind), t.start, t.end))
    }

    /// `SKIP_TOKEN`.
    fn skip(&mut self) -> R<Token> {
        self.scanner.take()
    }

    fn pop_state(&mut self) -> State {
        self.states.pop().unwrap_or(State::End)
    }

    fn pop_mark(&mut self) -> Mark {
        self.marks.pop().unwrap_or_default()
    }

    /// `yaml_parser_parse`: the next event; after STREAM-END, STREAM-END again.
    pub(crate) fn next_event(&mut self) -> R<Event> {
        if self.scanner.stream_end_produced() || self.state == State::End {
            let m = Mark::default();
            return Ok(Event {
                kind: EventKind::StreamEnd,
                start: m,
                end: m,
            });
        }
        match self.state {
            State::StreamStart => self.parse_stream_start(),
            State::ImplicitDocumentStart => self.parse_document_start(true),
            State::DocumentStart => self.parse_document_start(false),
            State::DocumentContent => self.parse_document_content(),
            State::DocumentEnd => self.parse_document_end(),
            State::BlockNode => self.parse_node(true, false),
            State::BlockSequenceFirstEntry => self.parse_block_sequence_entry(true),
            State::BlockSequenceEntry => self.parse_block_sequence_entry(false),
            State::IndentlessSequenceEntry => self.parse_indentless_sequence_entry(),
            State::BlockMappingFirstKey => self.parse_block_mapping_key(true),
            State::BlockMappingKey => self.parse_block_mapping_key(false),
            State::BlockMappingValue => self.parse_block_mapping_value(),
            State::FlowSequenceFirstEntry => self.parse_flow_sequence_entry(true),
            State::FlowSequenceEntry => self.parse_flow_sequence_entry(false),
            State::FlowSequenceEntryMappingKey => self.parse_flow_sequence_entry_mapping_key(),
            State::FlowSequenceEntryMappingValue => self.parse_flow_sequence_entry_mapping_value(),
            State::FlowSequenceEntryMappingEnd => self.parse_flow_sequence_entry_mapping_end(),
            State::FlowMappingFirstKey => self.parse_flow_mapping_key(true),
            State::FlowMappingKey => self.parse_flow_mapping_key(false),
            State::FlowMappingValue => self.parse_flow_mapping_value(false),
            State::FlowMappingEmptyValue => self.parse_flow_mapping_value(true),
            State::End => {
                let m = Mark::default();
                Ok(Event {
                    kind: EventKind::StreamEnd,
                    start: m,
                    end: m,
                })
            }
        }
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "every state function returns `R<Event>`; this keeps their tails uniform"
    )]
    fn event(kind: EventKind, start: Mark, end: Mark) -> R<Event> {
        Ok(Event { kind, start, end })
    }

    fn parse_stream_start(&mut self) -> R<Event> {
        let (kind, start, end) = self.peek()?;
        if kind != Kind::StreamStart {
            return Err(Error::parser(
                None,
                "did not find expected <stream-start>",
                start,
            ));
        }
        self.state = State::ImplicitDocumentStart;
        self.skip()?;
        Self::event(EventKind::StreamStart, start, end)
    }

    fn parse_document_start(&mut self, implicit: bool) -> R<Event> {
        let (mut kind, mut start, mut end) = self.peek()?;
        if !implicit {
            while kind == Kind::DocumentEnd {
                self.skip()?;
                (kind, start, end) = self.peek()?;
            }
        }
        if implicit
            && !matches!(
                kind,
                Kind::VersionDirective | Kind::TagDirective | Kind::DocumentStart | Kind::StreamEnd
            )
        {
            self.process_directives()?;
            self.states.push(State::DocumentEnd);
            self.state = State::BlockNode;
            return Self::event(EventKind::DocumentStart, start, end);
        }
        if kind == Kind::StreamEnd {
            self.state = State::End;
            self.skip()?;
            return Self::event(EventKind::StreamEnd, start, end);
        }
        let doc_start = start;
        self.process_directives()?;
        let (kind, start, end) = self.peek()?;
        if kind != Kind::DocumentStart {
            return Err(Error::parser(
                None,
                "did not find expected <document start>",
                start,
            ));
        }
        self.states.push(State::DocumentEnd);
        self.state = State::DocumentContent;
        self.skip()?;
        Self::event(EventKind::DocumentStart, doc_start, end)
    }

    fn parse_document_content(&mut self) -> R<Event> {
        let (kind, start, _) = self.peek()?;
        if matches!(
            kind,
            Kind::VersionDirective
                | Kind::TagDirective
                | Kind::DocumentStart
                | Kind::DocumentEnd
                | Kind::StreamEnd
        ) {
            self.state = self.pop_state();
            return Self::empty_scalar(start);
        }
        self.parse_node(true, false)
    }

    fn parse_document_end(&mut self) -> R<Event> {
        let (kind, start, token_end) = self.peek()?;
        let mut end = start;
        if kind == Kind::DocumentEnd {
            end = token_end;
            self.skip()?;
        }
        self.tag_directives.clear();
        self.state = State::DocumentStart;
        Self::event(EventKind::DocumentEnd, start, end)
    }

    /// `yaml_parser_parse_node`: properties (anchor, tag) and the node's first event.
    fn parse_node(&mut self, block: bool, indentless_sequence: bool) -> R<Event> {
        let (kind, start, end) = self.peek()?;
        if kind == Kind::Alias {
            let token = self.skip()?;
            self.state = self.pop_state();
            let TokenKind::Alias(name) = token.kind else {
                return Err(Error::Overflow);
            };
            return Self::event(EventKind::Alias(name), start, end);
        }
        let mut start_mark = start;
        let mut end_mark = start;
        let mut tag_mark = Mark::default();
        let mut anchor: Option<String> = None;
        let mut tag_parts: Option<(Vec<u8>, Vec<u8>)> = None;
        if kind == Kind::Anchor {
            let token = self.skip()?;
            start_mark = token.start;
            end_mark = token.end;
            if let TokenKind::Anchor(name) = token.kind {
                anchor = Some(name);
            }
            let (k, s, e) = self.peek()?;
            if k == Kind::Tag {
                let token = self.skip()?;
                tag_mark = s;
                end_mark = e;
                if let TokenKind::Tag { handle, suffix } = token.kind {
                    tag_parts = Some((handle, suffix));
                }
            }
        } else if kind == Kind::Tag {
            let token = self.skip()?;
            tag_mark = start;
            start_mark = start;
            end_mark = end;
            if let TokenKind::Tag { handle, suffix } = token.kind {
                tag_parts = Some((handle, suffix));
            }
            let (k, _, e) = self.peek()?;
            if k == Kind::Anchor {
                let token = self.skip()?;
                end_mark = e;
                if let TokenKind::Anchor(name) = token.kind {
                    anchor = Some(name);
                }
            }
        }
        let mut tag: Option<Vec<u8>> = None;
        if let Some((handle, suffix)) = tag_parts {
            if handle.is_empty() {
                tag = Some(suffix);
            } else {
                let found = self.tag_directives.iter().find(|d| d.handle == handle);
                match found {
                    Some(d) => {
                        let mut full = d.prefix.clone();
                        full.extend_from_slice(&suffix);
                        tag = Some(full);
                    }
                    None => {
                        return Err(Error::parser(
                            Some(("while parsing a node", start_mark)),
                            "found undefined tag handle",
                            tag_mark,
                        ));
                    }
                }
            }
        }
        let (kind, token_start, token_end) = self.peek()?;
        if indentless_sequence && kind == Kind::BlockEntry {
            self.state = State::IndentlessSequenceEntry;
            return Self::event(
                EventKind::SequenceStart { anchor, tag },
                start_mark,
                token_end,
            );
        }
        match kind {
            Kind::Scalar => {
                let token = self.skip()?;
                self.state = self.pop_state();
                let TokenKind::Scalar { value, style } = token.kind else {
                    return Err(Error::Overflow);
                };
                let plain_implicit =
                    (style == Style::Plain && tag.is_none()) || tag.as_deref() == Some(b"!");
                Self::event(
                    EventKind::Scalar {
                        anchor,
                        tag,
                        value,
                        plain_implicit,
                        style,
                    },
                    start_mark,
                    token_end,
                )
            }
            Kind::FlowSequenceStart => {
                self.state = State::FlowSequenceFirstEntry;
                Self::event(
                    EventKind::SequenceStart { anchor, tag },
                    start_mark,
                    token_end,
                )
            }
            Kind::FlowMappingStart => {
                self.state = State::FlowMappingFirstKey;
                Self::event(
                    EventKind::MappingStart { anchor, tag },
                    start_mark,
                    token_end,
                )
            }
            Kind::BlockSequenceStart if block => {
                self.state = State::BlockSequenceFirstEntry;
                Self::event(
                    EventKind::SequenceStart { anchor, tag },
                    start_mark,
                    token_end,
                )
            }
            Kind::BlockMappingStart if block => {
                self.state = State::BlockMappingFirstKey;
                Self::event(
                    EventKind::MappingStart { anchor, tag },
                    start_mark,
                    token_end,
                )
            }
            _ if anchor.is_some() || tag.is_some() => {
                // An empty scalar with properties; `plain_implicit` iff untagged (or tagged with '').
                self.state = self.pop_state();
                let plain_implicit = tag.as_deref().is_none_or(<[u8]>::is_empty);
                Self::event(
                    EventKind::Scalar {
                        anchor,
                        tag,
                        value: String::new(),
                        plain_implicit,
                        style: Style::Plain,
                    },
                    start_mark,
                    end_mark,
                )
            }
            _ => Err(Error::parser(
                Some((
                    if block {
                        "while parsing a block node"
                    } else {
                        "while parsing a flow node"
                    },
                    start_mark,
                )),
                "did not find expected node content",
                token_start,
            )),
        }
    }

    fn parse_block_sequence_entry(&mut self, first: bool) -> R<Event> {
        if first {
            let (_, start, _) = self.peek()?;
            self.marks.push(start);
            self.skip()?;
        }
        let (kind, start, end) = self.peek()?;
        match kind {
            Kind::BlockEntry => {
                self.skip()?;
                let (next, _, _) = self.peek()?;
                if matches!(next, Kind::BlockEntry | Kind::BlockEnd) {
                    self.state = State::BlockSequenceEntry;
                    Self::empty_scalar(end)
                } else {
                    self.states.push(State::BlockSequenceEntry);
                    self.parse_node(true, false)
                }
            }
            Kind::BlockEnd => {
                self.state = self.pop_state();
                self.pop_mark();
                self.skip()?;
                Self::event(EventKind::SequenceEnd, start, end)
            }
            _ => {
                let mark = self.pop_mark();
                Err(Error::parser(
                    Some(("while parsing a block collection", mark)),
                    "did not find expected '-' indicator",
                    start,
                ))
            }
        }
    }

    fn parse_indentless_sequence_entry(&mut self) -> R<Event> {
        let (kind, start, end) = self.peek()?;
        if kind == Kind::BlockEntry {
            self.skip()?;
            let (next, _, _) = self.peek()?;
            if matches!(
                next,
                Kind::BlockEntry | Kind::Key | Kind::Value | Kind::BlockEnd
            ) {
                self.state = State::IndentlessSequenceEntry;
                Self::empty_scalar(end)
            } else {
                self.states.push(State::IndentlessSequenceEntry);
                self.parse_node(true, false)
            }
        } else {
            self.state = self.pop_state();
            Self::event(EventKind::SequenceEnd, start, start)
        }
    }

    fn parse_block_mapping_key(&mut self, first: bool) -> R<Event> {
        if first {
            let (_, start, _) = self.peek()?;
            self.marks.push(start);
            self.skip()?;
        }
        let (kind, start, end) = self.peek()?;
        match kind {
            Kind::Key => {
                self.skip()?;
                let (next, _, _) = self.peek()?;
                if matches!(next, Kind::Key | Kind::Value | Kind::BlockEnd) {
                    self.state = State::BlockMappingValue;
                    Self::empty_scalar(end)
                } else {
                    self.states.push(State::BlockMappingValue);
                    self.parse_node(true, true)
                }
            }
            Kind::BlockEnd => {
                self.state = self.pop_state();
                self.pop_mark();
                self.skip()?;
                Self::event(EventKind::MappingEnd, start, end)
            }
            _ => {
                let mark = self.pop_mark();
                Err(Error::parser(
                    Some(("while parsing a block mapping", mark)),
                    "did not find expected key",
                    start,
                ))
            }
        }
    }

    fn parse_block_mapping_value(&mut self) -> R<Event> {
        let (kind, start, end) = self.peek()?;
        if kind == Kind::Value {
            self.skip()?;
            let (next, _, _) = self.peek()?;
            if matches!(next, Kind::Key | Kind::Value | Kind::BlockEnd) {
                self.state = State::BlockMappingKey;
                Self::empty_scalar(end)
            } else {
                self.states.push(State::BlockMappingKey);
                self.parse_node(true, true)
            }
        } else {
            self.state = State::BlockMappingKey;
            Self::empty_scalar(start)
        }
    }

    fn parse_flow_sequence_entry(&mut self, first: bool) -> R<Event> {
        if first {
            let (_, start, _) = self.peek()?;
            self.marks.push(start);
            self.skip()?;
        }
        let (mut kind, mut start, mut end) = self.peek()?;
        if kind != Kind::FlowSequenceEnd {
            if !first {
                if kind == Kind::FlowEntry {
                    self.skip()?;
                    (kind, start, end) = self.peek()?;
                } else {
                    let mark = self.pop_mark();
                    return Err(Error::parser(
                        Some(("while parsing a flow sequence", mark)),
                        "did not find expected ',' or ']'",
                        start,
                    ));
                }
            }
            if kind == Kind::Key {
                self.state = State::FlowSequenceEntryMappingKey;
                self.skip()?;
                return Self::event(
                    EventKind::MappingStart {
                        anchor: None,
                        tag: None,
                    },
                    start,
                    end,
                );
            } else if kind != Kind::FlowSequenceEnd {
                self.states.push(State::FlowSequenceEntry);
                return self.parse_node(false, false);
            }
        }
        self.state = self.pop_state();
        self.pop_mark();
        self.skip()?;
        Self::event(EventKind::SequenceEnd, start, end)
    }

    fn parse_flow_sequence_entry_mapping_key(&mut self) -> R<Event> {
        let (kind, _, end) = self.peek()?;
        if matches!(kind, Kind::Value | Kind::FlowEntry | Kind::FlowSequenceEnd) {
            self.skip()?;
            self.state = State::FlowSequenceEntryMappingValue;
            Self::empty_scalar(end)
        } else {
            self.states.push(State::FlowSequenceEntryMappingValue);
            self.parse_node(false, false)
        }
    }

    fn parse_flow_sequence_entry_mapping_value(&mut self) -> R<Event> {
        let (mut kind, mut start, _) = self.peek()?;
        if kind == Kind::Value {
            self.skip()?;
            (kind, start, _) = self.peek()?;
            if !matches!(kind, Kind::FlowEntry | Kind::FlowSequenceEnd) {
                self.states.push(State::FlowSequenceEntryMappingEnd);
                return self.parse_node(false, false);
            }
        }
        self.state = State::FlowSequenceEntryMappingEnd;
        Self::empty_scalar(start)
    }

    fn parse_flow_sequence_entry_mapping_end(&mut self) -> R<Event> {
        // The token is not consumed: both marks are its start (libyaml-safer uses its end).
        let (_, start, _) = self.peek()?;
        self.state = State::FlowSequenceEntry;
        Self::event(EventKind::MappingEnd, start, start)
    }

    fn parse_flow_mapping_key(&mut self, first: bool) -> R<Event> {
        if first {
            let (_, start, _) = self.peek()?;
            self.marks.push(start);
            self.skip()?;
        }
        let (mut kind, mut start, mut end) = self.peek()?;
        if kind != Kind::FlowMappingEnd {
            if !first {
                if kind == Kind::FlowEntry {
                    self.skip()?;
                    (kind, start, end) = self.peek()?;
                } else {
                    let mark = self.pop_mark();
                    return Err(Error::parser(
                        Some(("while parsing a flow mapping", mark)),
                        "did not find expected ',' or '}'",
                        start,
                    ));
                }
            }
            if kind == Kind::Key {
                self.skip()?;
                let (next, next_start, _) = self.peek()?;
                if !matches!(next, Kind::Value | Kind::FlowEntry | Kind::FlowMappingEnd) {
                    self.states.push(State::FlowMappingValue);
                    return self.parse_node(false, false);
                }
                self.state = State::FlowMappingValue;
                return Self::empty_scalar(next_start);
            } else if kind != Kind::FlowMappingEnd {
                self.states.push(State::FlowMappingEmptyValue);
                return self.parse_node(false, false);
            }
        }
        self.state = self.pop_state();
        self.pop_mark();
        self.skip()?;
        Self::event(EventKind::MappingEnd, start, end)
    }

    fn parse_flow_mapping_value(&mut self, empty: bool) -> R<Event> {
        let (mut kind, mut start, _) = self.peek()?;
        if empty {
            self.state = State::FlowMappingKey;
            return Self::empty_scalar(start);
        }
        if kind == Kind::Value {
            self.skip()?;
            (kind, start, _) = self.peek()?;
            if !matches!(kind, Kind::FlowEntry | Kind::FlowMappingEnd) {
                self.states.push(State::FlowMappingKey);
                return self.parse_node(false, false);
            }
        }
        self.state = State::FlowMappingKey;
        Self::empty_scalar(start)
    }

    /// `yaml_parser_process_empty_scalar`.
    fn empty_scalar(mark: Mark) -> R<Event> {
        Self::event(
            EventKind::Scalar {
                anchor: None,
                tag: None,
                value: String::new(),
                plain_implicit: true,
                style: Style::Plain,
            },
            mark,
            mark,
        )
    }

    /// `yaml_parser_process_directives`: `%YAML` and `%TAG`, then the default handles.
    fn process_directives(&mut self) -> R<()> {
        let mut version_seen = false;
        loop {
            let (kind, start, _) = self.peek()?;
            match kind {
                Kind::VersionDirective => {
                    let token = self.skip()?;
                    if version_seen {
                        return Err(Error::parser(
                            None,
                            "found duplicate %YAML directive",
                            start,
                        ));
                    }
                    if let TokenKind::VersionDirective { major, minor } = token.kind
                        && (major != 1 || (minor != 1 && minor != 2))
                    {
                        return Err(Error::parser(
                            None,
                            "found incompatible YAML document",
                            start,
                        ));
                    }
                    version_seen = true;
                }
                Kind::TagDirective => {
                    let token = self.skip()?;
                    if let TokenKind::TagDirective { handle, prefix } = token.kind {
                        if self.tag_directives.iter().any(|d| d.handle == handle) {
                            return Err(Error::parser(
                                None,
                                "found duplicate %TAG directive",
                                start,
                            ));
                        }
                        self.tag_directives.push(TagDirective { handle, prefix });
                    }
                }
                _ => break,
            }
        }
        for (handle, prefix) in [(&b"!"[..], &b"!"[..]), (b"!!", b"tag:yaml.org,2002:")] {
            if !self.tag_directives.iter().any(|d| d.handle == handle) {
                self.tag_directives.push(TagDirective {
                    handle: handle.to_vec(),
                    prefix: prefix.to_vec(),
                });
            }
        }
        Ok(())
    }
}
