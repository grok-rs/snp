//! The arena composer (`yaml.compose` / `compose_all`).
//!
//! A port of `_yaml.pyx` `CParser._compose_*` (PyYAML 6.0.3) over the parser events: the anchor of a collection is
//! registered before its children (so `&a [*a]` composes), a redefined anchor and an undefined alias are composer
//! errors with the C texts, and scalar tags are resolved here (PyYAML resolves while composing). The composer is
//! iterative — nesting depth costs heap, never stack — and enforces [`Limits`] (D-SEC-11).
//!
//! Owner: WP-14.

use std::collections::HashMap;
use std::ops::Range;

use snp_core as _;
use snp_pycompat::codec::decode_utf8;

use crate::events::{Event, EventKind, Events, ParseError, RawMark, Style};
use crate::messages::{
    DUPLICATE_ANCHOR_CONTEXT, DUPLICATE_ANCHOR_PROBLEM, INVALID_DOCUMENT, SINGLE_DOCUMENT_CONTEXT,
    SINGLE_DOCUMENT_PROBLEM, UNDEFINED_ALIAS, limit_bytes, limit_depth, limit_nodes,
};
use crate::resolve::{MAP_TAG, SEQ_TAG, STR_TAG};
use crate::{Limits, Mark, Schema, YamlError, resolve_plain};

/// Index of a node in [`Document::nodes`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(pub u32);

impl NodeId {
    /// The index into [`Document::nodes`].
    pub(crate) fn index(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }
}

/// Node payload (PyYAML `ScalarNode.value` / `SequenceNode.value` / `MappingNode.value`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum NodeKind {
    /// Scalar value (unresolved text).
    Scalar(Box<str>),
    /// Sequence items.
    Seq(Vec<NodeId>),
    /// Mapping pairs in document order.
    Map(Vec<(NodeId, NodeId)>),
}

impl NodeKind {
    /// PyYAML's `Node.id`: `scalar`, `sequence` or `mapping` (used in constructor messages).
    pub(crate) const fn py_id(&self) -> &'static str {
        match self {
            Self::Scalar(_) => "scalar",
            Self::Seq(_) => "sequence",
            Self::Map(_) => "mapping",
        }
    }
}

/// Scalar presentation style (PyYAML `Node.style`: `''`, `"'"`, `'"'`, `'|'`, `'>'`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScalarStyle {
    /// Plain.
    Plain,
    /// `'single'`.
    Single,
    /// `"double"`.
    Double,
    /// `|`.
    Literal,
    /// `>`.
    Folded,
    /// Not a scalar.
    NotScalar,
}

/// One composed node.
#[derive(Debug, Clone)]
pub struct Node {
    /// Payload.
    pub kind: NodeKind,
    /// Resolved tag: the explicit tag, or for an untagged node (or one tagged with the non-specific `!`) the tag
    /// PyYAML's resolver assigns (`Resolver.resolve`).
    pub tag: Box<str>,
    /// Scalar style.
    pub style: ScalarStyle,
    /// Byte span in the text: from the start of the node's first token (its anchor or tag when it has one) to the
    /// end of its last event (`node.start_mark.index` / `node.end_mark.index`, in bytes).
    pub span: Range<usize>,
    /// Start mark (character index, 0-based line and column, as `node.start_mark` of the C loader).
    pub start: Mark,
    /// `&anchor`, if any.
    pub anchor: Option<Box<str>>,
}

/// Arena composer: an alias is the *same* `NodeId` as its anchor (never a copy) — migrate-config dedupes by id.
#[derive(Debug, Clone)]
pub struct Document {
    /// The arena.
    pub nodes: Vec<Node>,
    /// The root node; `None` for an empty stream.
    pub root: Option<NodeId>,
}

/// `yaml.compose(text)` (one document, pre-commit semantics) under `schema` and `lim`.
///
/// Mirrors `CParser.get_single_node`: an empty stream (or one with only comments) gives `root: None`; a second
/// document is the composer error `expected a single document in the stream` whose context mark is the start of
/// the first document's root node and whose problem mark is the start of the second document. Errors of the
/// second document's body are never reached. Scanner, parser and reader errors carry libyaml's texts and marks.
pub fn compose(text: &str, schema: Schema, lim: &Limits) -> Result<Document, YamlError> {
    let mut c = Composer::new(text, schema, lim)?;
    c.expect_stream_start()?;
    let ev = c.next_event()?;
    match ev.kind {
        EventKind::StreamEnd => Ok(Document {
            nodes: Vec::new(),
            root: None,
        }),
        EventKind::DocumentStart => {
            let doc = c.compose_document()?;
            let next = c.next_event()?;
            if matches!(next.kind, EventKind::StreamEnd) {
                Ok(doc)
            } else {
                let first = doc
                    .root
                    .and_then(|r| doc.nodes.get(r.index()))
                    .map(|n| n.start);
                Err(YamlError::composer(
                    Some(SINGLE_DOCUMENT_CONTEXT),
                    first,
                    SINGLE_DOCUMENT_PROBLEM,
                    mark(next.start),
                ))
            }
        }
        _ => Err(invalid(ev.start)),
    }
}

/// `yaml.compose_all(text)`: every document of the stream (anchors are per document, as in PyYAML). The node
/// and size limits apply to the stream as a whole, and each document counts as one more node (a stream of
/// one-node documents stays far below the 512 MB fuzzing RSS cap).
pub fn compose_all(text: &str, schema: Schema, lim: &Limits) -> Result<Vec<Document>, YamlError> {
    let mut c = Composer::new(text, schema, lim)?;
    c.expect_stream_start()?;
    let mut docs = Vec::new();
    let max = usize::try_from(lim.max_constructed_nodes).unwrap_or(usize::MAX);
    loop {
        let ev = c.next_event()?;
        match ev.kind {
            EventKind::StreamEnd => return Ok(docs),
            EventKind::DocumentStart => {
                if c.total_nodes >= max {
                    return Err(YamlError::limit(
                        limit_nodes(lim.max_constructed_nodes),
                        Some(mark(ev.start)),
                    ));
                }
                c.total_nodes = c.total_nodes.saturating_add(1);
                let mut doc = c.compose_document()?;
                doc.nodes.shrink_to_fit();
                docs.push(doc);
            }
            _ => return Err(invalid(ev.start)),
        }
    }
}

/// A PyYAML mark for a parser mark (both count characters after the BOM).
const fn mark(m: RawMark) -> Mark {
    Mark {
        index: m.index,
        line: m.line,
        column: m.column,
    }
}

/// An internal error for an event sequence the parser never produces.
fn invalid(at: RawMark) -> YamlError {
    YamlError::constructor(INVALID_DOCUMENT, mark(at))
}

/// `CParser._parser_error`.
fn convert(e: ParseError) -> YamlError {
    match e {
        ParseError::Marked {
            scanner,
            context,
            problem,
            problem_mark,
        } => YamlError::marked(
            if scanner {
                "ScannerError"
            } else {
                "ParserError"
            },
            context.map(|(c, _)| c.into()),
            context.map(|(_, m)| mark(m)),
            problem,
            Some(mark(problem_mark)),
        ),
        ParseError::Reader { value, offset } => {
            YamlError::reader(value, offset, "control characters are not allowed")
        }
        ParseError::Overflow => YamlError::marked("YAMLError", None, None, INVALID_DOCUMENT, None),
    }
}

/// `PyUnicode_FromYamlString(tag)`: a tag may hold `%XX` octets that are not UTF-8 (`UnicodeDecodeError`).
fn decode_tag(tag: Vec<u8>) -> Result<Box<str>, YamlError> {
    if let Err(e) = decode_utf8(&tag) {
        return Err(YamlError::python("UnicodeDecodeError", e.to_string()));
    }
    String::from_utf8(tag)
        .map(String::into_boxed_str)
        .map_err(|_| YamlError::marked("YAMLError", None, None, INVALID_DOCUMENT, None))
}

/// The explicit tag of a node; `None` when it is absent or the non-specific `!` (which PyYAML resolves).
fn explicit_tag(tag: Option<Vec<u8>>) -> Result<Option<Box<str>>, YamlError> {
    match tag {
        None => Ok(None),
        Some(t) if t == b"!" => Ok(None),
        Some(t) => decode_tag(t).map(Some),
    }
}

/// One open collection of the iterative composer.
#[derive(Debug)]
enum Frame {
    Seq {
        id: NodeId,
        items: Vec<NodeId>,
    },
    Map {
        id: NodeId,
        pairs: Vec<(NodeId, NodeId)>,
        key: Option<NodeId>,
    },
}

/// The composer state over one stream.
#[derive(Debug)]
struct Composer<'a> {
    events: Events<'a>,
    schema: Schema,
    lim: Limits,
    /// Length of a leading UTF-8 BOM (skipped by the parser; byte spans include it).
    bom: usize,
    /// Nodes composed so far in the whole stream (the node limit is per stream).
    total_nodes: usize,
}

impl<'a> Composer<'a> {
    fn new(text: &'a str, schema: Schema, lim: &Limits) -> Result<Self, YamlError> {
        if text.len() > lim.max_bytes {
            return Err(YamlError::limit(
                limit_bytes(text.len(), lim.max_bytes),
                None,
            ));
        }
        let body = text.strip_prefix('\u{feff}').unwrap_or(text);
        Ok(Self {
            events: Events::new(text),
            schema,
            lim: *lim,
            bom: text.len().saturating_sub(body.len()),
            total_nodes: 0,
        })
    }

    /// The byte offset into the text of a parser mark.
    const fn byte(&self, m: RawMark) -> usize {
        m.byte.saturating_add(self.bom)
    }

    fn next_event(&mut self) -> Result<Event, YamlError> {
        self.events.next_event().map_err(convert)
    }

    fn expect_stream_start(&mut self) -> Result<(), YamlError> {
        let ev = self.next_event()?;
        if matches!(ev.kind, EventKind::StreamStart) {
            Ok(())
        } else {
            Err(invalid(ev.start))
        }
    }

    /// Adds a node to the arena, enforcing the node limit.
    fn push(&mut self, nodes: &mut Vec<Node>, node: Node) -> Result<NodeId, YamlError> {
        let max = usize::try_from(self.lim.max_constructed_nodes).unwrap_or(usize::MAX);
        let limit =
            |start| YamlError::limit(limit_nodes(self.lim.max_constructed_nodes), Some(start));
        if self.total_nodes >= max {
            return Err(limit(node.start));
        }
        let id = u32::try_from(nodes.len()).map_err(|_| limit(node.start))?;
        nodes.try_reserve(1).map_err(|_| limit(node.start))?;
        nodes.push(node);
        self.total_nodes = self.total_nodes.saturating_add(1);
        Ok(NodeId(id))
    }

    /// `_compose_document` after the DOCUMENT-START event: the root node, then DOCUMENT-END.
    fn compose_document(&mut self) -> Result<Document, YamlError> {
        let mut nodes: Vec<Node> = Vec::new();
        let mut anchors: HashMap<String, NodeId> = HashMap::new();
        let mut stack: Vec<Frame> = Vec::new();
        let root = loop {
            let ev = self.next_event()?;
            let start = mark(ev.start);
            let is_seq = matches!(ev.kind, EventKind::SequenceStart { .. });
            let done = match ev.kind {
                EventKind::Alias(name) => match anchors.get(&name) {
                    Some(id) => *id,
                    None => return Err(YamlError::composer(None, None, UNDEFINED_ALIAS, start)),
                },
                EventKind::Scalar {
                    anchor,
                    tag,
                    value,
                    plain_implicit,
                    style,
                } => {
                    check_anchor(&anchors, &nodes, anchor.as_deref(), start)?;
                    let tag = match explicit_tag(tag)? {
                        Some(t) => t,
                        None => self.implicit_scalar_tag(&value, plain_implicit),
                    };
                    let node = Node {
                        kind: NodeKind::Scalar(value.into_boxed_str()),
                        tag,
                        style: match style {
                            Style::Plain => ScalarStyle::Plain,
                            Style::Single => ScalarStyle::Single,
                            Style::Double => ScalarStyle::Double,
                            Style::Literal => ScalarStyle::Literal,
                            Style::Folded => ScalarStyle::Folded,
                        },
                        span: self.byte(ev.start)..self.byte(ev.end),
                        start,
                        anchor: anchor.as_deref().map(Into::into),
                    };
                    let id = self.push(&mut nodes, node)?;
                    if let Some(a) = anchor {
                        anchors.insert(a, id);
                    }
                    id
                }
                EventKind::SequenceStart { anchor, tag }
                | EventKind::MappingStart { anchor, tag } => {
                    check_anchor(&anchors, &nodes, anchor.as_deref(), start)?;
                    let max_depth = usize::try_from(self.lim.max_depth).unwrap_or(usize::MAX);
                    if stack.len() >= max_depth {
                        return Err(YamlError::limit(
                            limit_depth(self.lim.max_depth),
                            Some(start),
                        ));
                    }
                    let default = if is_seq { SEQ_TAG } else { MAP_TAG };
                    let tag = explicit_tag(tag)?.unwrap_or_else(|| default.into());
                    let node = Node {
                        kind: if is_seq {
                            NodeKind::Seq(Vec::new())
                        } else {
                            NodeKind::Map(Vec::new())
                        },
                        tag,
                        style: ScalarStyle::NotScalar,
                        span: self.byte(ev.start)..self.byte(ev.end),
                        start,
                        anchor: anchor.as_deref().map(Into::into),
                    };
                    let id = self.push(&mut nodes, node)?;
                    if let Some(a) = anchor {
                        anchors.insert(a, id);
                    }
                    stack.try_reserve(1).map_err(|_| {
                        YamlError::limit(limit_depth(self.lim.max_depth), Some(start))
                    })?;
                    stack.push(if is_seq {
                        Frame::Seq {
                            id,
                            items: Vec::new(),
                        }
                    } else {
                        Frame::Map {
                            id,
                            pairs: Vec::new(),
                            key: None,
                        }
                    });
                    continue;
                }
                EventKind::SequenceEnd | EventKind::MappingEnd => {
                    let end = self.byte(ev.end);
                    let (id, kind) = match stack.pop() {
                        Some(Frame::Seq { id, items }) => (id, NodeKind::Seq(items)),
                        Some(Frame::Map { id, pairs, .. }) => (id, NodeKind::Map(pairs)),
                        None => return Err(invalid(ev.start)),
                    };
                    let Some(node) = nodes.get_mut(id.index()) else {
                        return Err(invalid(ev.start));
                    };
                    node.kind = kind;
                    node.span.end = end;
                    id
                }
                EventKind::StreamStart
                | EventKind::StreamEnd
                | EventKind::DocumentStart
                | EventKind::DocumentEnd => return Err(invalid(ev.start)),
            };
            match stack.last_mut() {
                None => break done,
                Some(Frame::Seq { items, .. }) => items.push(done),
                Some(Frame::Map { pairs, key, .. }) => match key.take() {
                    None => *key = Some(done),
                    Some(k) => pairs.push((k, done)),
                },
            }
        };
        let end = self.next_event()?;
        if !matches!(end.kind, EventKind::DocumentEnd) {
            return Err(invalid(end.start));
        }
        Ok(Document {
            nodes,
            root: Some(root),
        })
    }

    /// `Resolver.resolve(ScalarNode, value, (plain_implicit, quoted_implicit))` for an untagged (or `!`) scalar.
    fn implicit_scalar_tag(&self, value: &str, plain_implicit: bool) -> Box<str> {
        if plain_implicit {
            resolve_plain(value, self.schema).as_tag().into()
        } else {
            STR_TAG.into()
        }
    }
}

/// `_compose_node`: a redefined anchor is an error naming both occurrences.
fn check_anchor(
    anchors: &HashMap<String, NodeId>,
    nodes: &[Node],
    anchor: Option<&str>,
    at: Mark,
) -> Result<(), YamlError> {
    let Some(anchor) = anchor else {
        return Ok(());
    };
    match anchors.get(anchor) {
        None => Ok(()),
        Some(first) => Err(YamlError::composer(
            Some(DUPLICATE_ANCHOR_CONTEXT),
            nodes.get(first.index()).map(|n| n.start),
            DUPLICATE_ANCHOR_PROBLEM,
            at,
        )),
    }
}
