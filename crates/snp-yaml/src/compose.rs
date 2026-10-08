//! The arena composer over libyaml-safer events (`yaml.compose` / `compose_all`).
//!
//! Owner: WP-14.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-14")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-14")]

use std::ops::Range;

use snp_core as _;

use crate::{Limits, Mark, Schema, YamlError};

/// Index of a node in [`Document::nodes`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(pub u32);

/// Node payload.
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

/// Scalar presentation style.
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
    /// Resolved tag.
    pub tag: Box<str>,
    /// Scalar style.
    pub style: ScalarStyle,
    /// Byte span in the text.
    pub span: Range<usize>,
    /// Start mark.
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
pub fn compose(text: &str, schema: Schema, lim: &Limits) -> Result<Document, YamlError> {
    todo!()
}
