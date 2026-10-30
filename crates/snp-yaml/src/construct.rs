//! `SafeConstructor` + implicit resolver → `cfgv::Value`.
//!
//! A port of PyYAML 6.0.3 `yaml/constructor.py` (`BaseConstructor` + `SafeConstructor`) that runs in three phases:
//!
//! 1. **Python order.** `construct_document` constructs the root, then runs the queued collection generators in
//!    rounds (breadth first: a round runs every generator queued by the previous one). Scalars are constructed at
//!    once, collections are queued, every node is constructed once (`constructed_objects`), and `flatten_mapping`
//!    mutates the mapping's pair list and the tags of `=` keys exactly as PyYAML does. This phase reports the first
//!    error PyYAML would raise, with its text.
//! 2. **Limits.** The constructed graph is a DAG (aliases share nodes) or has cycles (`&a [*a]`, which Python
//!    represents with self-referencing objects). A `cfgv::Value` is a tree, so a cycle is reported as PyYAML's
//!    `found unconstructable recursive node`, and the expanded tree must stay within `max_constructed_nodes` and
//!    `max_depth` (D-SEC-11) — checked on the DAG in linear time, before anything is copied.
//! 3. **Materialisation** into a `cfgv::Value` with an explicit stack (no recursion).
//!
//! Owner: WP-14.

use std::borrow::Cow;

use cfgv::{BigInt, Date, DateError, DateTime, Map, Set, TzOffset, Value};
use indexmap::IndexMap;

use crate::base64;
use crate::bignum::Big;
use crate::compose::{Document, Node, NodeId, NodeKind};
use crate::messages::{
    INT_TOO_LARGE_FOR_FLOAT, INVALID_DOCUMENT, STRING_INDEX_OUT_OF_RANGE, TIMESTAMP_NO_MATCH,
    TIMESTAMP_NOT_STRING, UNCONSTRUCTABLE_RECURSIVE, UNHASHABLE_KEY, WHILE_CONSTRUCTING_MAPPING,
    WHILE_CONSTRUCTING_OMAP, WHILE_CONSTRUCTING_PAIRS, ascii_encode_error, binary_bad_base64,
    binary_not_ascii, expected_mapping_node, expected_scalar, expected_sequence,
    expected_sequence_node, expected_single_item, expected_single_mapping, limit_depth,
    limit_nodes, merge_expected_mapping, merge_expected_mapping_or_list, undefined_tag,
};
use crate::pynum::{py_float, py_int, py_lower};
use crate::resolve::{
    BINARY_TAG, BOOL_TAG, FLOAT_TAG, GoScalar, INT_TAG, MAP_TAG, MERGE_TAG, NULL_TAG, OMAP_TAG,
    PAIRS_TAG, SEQ_TAG, SET_TAG, STR_TAG, TIMESTAMP_TAG, Tz, VALUE_TAG, go_resolve,
    match_timestamp,
};
use crate::{Limits, Mark, Schema, YamlError, compose};

/// `SafeConstructor` + implicit resolver → `cfgv::Value` (date, datetime, big ints, bytes, set, omap/pairs as tuples,
/// merge keys, duplicate keys last-wins at the first position with Python key equality 1 == 1.0 == True).
///
/// `yaml.constructor.SafeConstructor.construct_document(node)` with PyYAML's exact error texts and error order:
/// constructor errors are `MarkedYAMLError`s; the Python exceptions that escape (`ValueError` from `int()`,
/// `float()` and `datetime`, `KeyError` from `!!bool`, `IndexError` from an empty `!!int`, `AttributeError` from
/// a non-matching `!!timestamp`) render as `str(exc)`, which is what cfgv reports. `schema` selects the int
/// constructor for implicit ints (YAML 1.2 core ints are decimal with leading zeros, `0o`, `0x`). An empty
/// document (`root: None`) is `None`. Python values that a tree cannot hold — a collection that contains itself —
/// are the error `found unconstructable recursive node` (D-SEC-11).
pub fn construct(doc: &Document, schema: Schema, lim: &Limits) -> Result<Value, YamlError> {
    let graph = construct_graph(doc, schema, lim)?;
    match graph.root {
        None => Ok(Value::Null),
        Some(root) => graph.materialize(root),
    }
}

/// The constructed Python object graph: one object per composed node (`constructed_objects`), shared by every
/// alias, as PyYAML builds it. `yaml.dump` of a loaded value anchors objects that occur more than once
/// ([`crate::emit::redump`]).
#[derive(Debug)]
pub(crate) struct Graph {
    /// The object built for each node of the document (`None` for nodes outside the value).
    pub(crate) built: Vec<Option<Built>>,
    /// The root object (`None` for an empty document).
    pub(crate) root: Option<NodeId>,
}

/// Phases 1 and 2 of [`construct`]: PyYAML's construction order and errors, then the cycle and limit checks.
pub(crate) fn construct_graph(
    doc: &Document,
    schema: Schema,
    lim: &Limits,
) -> Result<Graph, YamlError> {
    let Some(root) = doc.root else {
        return Ok(Graph {
            built: Vec::new(),
            root: None,
        });
    };
    let mut st = State::new(doc, schema, lim);
    st.construct_document(root)?;
    st.check_limits(root)?;
    Ok(Graph {
        built: st.built,
        root: Some(root),
    })
}

/// `yaml.load(text, Loader=CSafeLoader)`: `PyYaml11Safe`, one document.
///
/// [`compose`] then [`construct`]: syntax, reader and composer errors come first (`get_single_node`), then
/// constructor errors, as in PyYAML.
pub fn load(text: &str, lim: &Limits) -> Result<Value, YamlError> {
    let doc = compose(text, Schema::PyYaml11Safe, lim)?;
    construct(&doc, Schema::PyYaml11Safe, lim)
}

/// What phase 1 built for a node (`constructed_objects[node]`).
#[derive(Debug, Clone)]
pub(crate) enum Built {
    /// A scalar constructor's value (always hashable).
    Scalar(Value),
    /// A collection whose generator is queued (`yield data` happened, the body has not run).
    Pending(Gen),
    /// `list` from `!!seq`.
    Seq(Vec<NodeId>),
    /// `dict` from `!!map`: the entries in insertion order, keys unique under Python equality (the first key
    /// object with the last value, as `dict.__setitem__` keeps them).
    Map(Vec<(NodeId, NodeId)>),
    /// `set` from `!!set`: the members in insertion order (the first of equal keys).
    Set(Vec<NodeId>),
    /// `list` of 2-tuples from `!!omap` / `!!pairs`.
    Pairs(Vec<(NodeId, NodeId)>),
}

/// The generator kinds of `SafeConstructor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gen {
    Seq,
    Map,
    Set,
    Omap,
    Pairs,
}

/// The constructor dispatch of one tag.
#[derive(Debug, Clone, Copy)]
enum Ctor {
    Null,
    Bool,
    Int,
    Float,
    Binary,
    Timestamp,
    Str,
    Gen(Gen),
    Undefined,
}

/// `SafeConstructor.yaml_constructors[tag]`.
fn ctor_for(tag: &str) -> Ctor {
    match tag {
        NULL_TAG => Ctor::Null,
        BOOL_TAG => Ctor::Bool,
        INT_TAG => Ctor::Int,
        FLOAT_TAG => Ctor::Float,
        BINARY_TAG => Ctor::Binary,
        TIMESTAMP_TAG => Ctor::Timestamp,
        STR_TAG => Ctor::Str,
        OMAP_TAG => Ctor::Gen(Gen::Omap),
        PAIRS_TAG => Ctor::Gen(Gen::Pairs),
        SET_TAG => Ctor::Gen(Gen::Set),
        SEQ_TAG => Ctor::Gen(Gen::Seq),
        MAP_TAG => Ctor::Gen(Gen::Map),
        _ => Ctor::Undefined,
    }
}

/// One frame of the iterative `flatten_mapping`.
#[derive(Debug)]
struct FlattenFrame {
    node: NodeId,
    /// PyYAML's `index` into the live `node.value`.
    index: usize,
    /// The position of `index` in the stored list (which holds [`TOMBSTONE`]s for deleted `<<` pairs), valid
    /// while the list's version is `ver`.
    raw: usize,
    ver: u32,
    merge: Vec<(NodeId, NodeId)>,
    resume: Resume,
}

/// Where a `flatten_mapping` frame resumes after a nested `flatten_mapping` returns.
#[derive(Debug)]
enum Resume {
    /// Continue the scan of `node.value`.
    Scan,
    /// `merge.extend(value_node.value)` for a mapping merged with `<<: *m`.
    MergedMap(NodeId),
    /// `submerge.append(subnode.value)` for item `pos` of the merged sequence `seq`, then the next item.
    MergedSeqItem {
        seq: NodeId,
        pos: usize,
        submerge: Vec<Vec<(NodeId, NodeId)>>,
    },
}

/// A `<<` pair deleted from a stored `node.value` (`del node.value[index]` in O(1)); a frame skips it, and the
/// list is compacted when the frame that flattens it returns.
const TOMBSTONE: (NodeId, NodeId) = (NodeId(u32::MAX), NodeId(u32::MAX));

/// Phase-1 state: PyYAML's constructor state over the arena.
#[derive(Debug)]
struct State<'d> {
    doc: &'d Document,
    schema: Schema,
    lim: Limits,
    /// `constructed_objects`.
    built: Vec<Option<Built>>,
    /// `node.value` of mapping nodes that `flatten_mapping` changed.
    pairs: Vec<Option<Vec<(NodeId, NodeId)>>>,
    /// Keys whose tag `flatten_mapping` changed from `value` to `str`.
    str_tag: Vec<bool>,
    /// `state_generators` of the next round.
    queue: Vec<NodeId>,
    /// Bumped whenever a stored `node.value` is compacted or replaced (frames re-find their position).
    version: Vec<u32>,
    /// `flatten_mapping`'s merge work — nested flattens, `<<` keys, pairs copied — counted against
    /// `max_constructed_nodes` (merge bombs, D-SEC-11).
    work: u64,
}

impl<'d> State<'d> {
    fn new(doc: &'d Document, schema: Schema, lim: &Limits) -> Self {
        let n = doc.nodes.len();
        Self {
            doc,
            schema,
            lim: *lim,
            built: vec![None; n],
            pairs: vec![None; n],
            str_tag: vec![false; n],
            queue: Vec::new(),
            version: vec![0; n],
            work: 0,
        }
    }

    fn node(&self, id: NodeId) -> Result<&'d Node, YamlError> {
        self.doc
            .nodes
            .get(id.index())
            .ok_or_else(|| YamlError::marked("YAMLError", None, None, INVALID_DOCUMENT, None))
    }

    /// `node.tag` after `flatten_mapping`'s `key_node.tag = 'tag:yaml.org,2002:str'`.
    fn tag(&self, id: NodeId) -> Result<&'d str, YamlError> {
        let node = self.node(id)?;
        if self.str_tag.get(id.index()).copied().unwrap_or(false) {
            Ok(STR_TAG)
        } else {
            Ok(&node.tag)
        }
    }

    /// `node.value` of a mapping node (the flattened list once `flatten_mapping` changed it).
    fn pairs_of(&self, id: NodeId) -> Result<&[(NodeId, NodeId)], YamlError> {
        if let Some(Some(p)) = self.pairs.get(id.index()) {
            return Ok(p);
        }
        match &self.node(id)?.kind {
            NodeKind::Map(p) => Ok(p),
            _ => Ok(&[]),
        }
    }

    fn set_pairs(&mut self, id: NodeId, value: Vec<(NodeId, NodeId)>) {
        if let Some(slot) = self.pairs.get_mut(id.index()) {
            *slot = Some(value);
        }
        self.bump(id);
    }

    /// The stored `node.value` of a mapping, copied from the node the first time it changes.
    fn pairs_mut(&mut self, id: NodeId) -> Result<Option<&mut Vec<(NodeId, NodeId)>>, YamlError> {
        let node = self.node(id)?;
        let Some(slot) = self.pairs.get_mut(id.index()) else {
            return Ok(None);
        };
        if slot.is_none() {
            let NodeKind::Map(p) = &node.kind else {
                return Ok(None);
            };
            *slot = Some(p.clone());
        }
        Ok(slot.as_mut())
    }

    /// The live pairs of `node.value` (without [`TOMBSTONE`]s).
    fn live_pairs(&self, id: NodeId) -> Result<Vec<(NodeId, NodeId)>, YamlError> {
        Ok(self
            .pairs_of(id)?
            .iter()
            .copied()
            .filter(|p| *p != TOMBSTONE)
            .collect())
    }

    fn version_of(&self, id: NodeId) -> u32 {
        self.version.get(id.index()).copied().unwrap_or(0)
    }

    fn bump(&mut self, id: NodeId) {
        if let Some(v) = self.version.get_mut(id.index()) {
            *v = v.wrapping_add(1);
        }
    }

    fn set_built(&mut self, id: NodeId, b: Built) {
        if let Some(slot) = self.built.get_mut(id.index()) {
            *slot = Some(b);
        }
    }

    fn limit_nodes_error(&self, mark: Mark) -> YamlError {
        YamlError::limit(limit_nodes(self.lim.max_constructed_nodes), Some(mark))
    }

    /// Counts `n` units of merge work against the node budget.
    fn spend(&mut self, n: usize, at: Mark) -> Result<(), YamlError> {
        self.work = self
            .work
            .saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        if self.work > u64::from(self.lim.max_constructed_nodes) {
            Err(self.limit_nodes_error(at))
        } else {
            Ok(())
        }
    }

    // --- phase 1: PyYAML's construction order ------------------------------------------------------------------

    /// `construct_document`: the root, then the generators in rounds.
    fn construct_document(&mut self, root: NodeId) -> Result<(), YamlError> {
        self.construct_object(root)?;
        while !self.queue.is_empty() {
            let round = std::mem::take(&mut self.queue);
            for id in round {
                self.run_generator(id)?;
            }
        }
        Ok(())
    }

    /// `construct_object(node)`.
    fn construct_object(&mut self, id: NodeId) -> Result<(), YamlError> {
        if self.built.get(id.index()).is_some_and(Option::is_some) {
            return Ok(());
        }
        let node = self.node(id)?;
        let tag = self.tag(id)?;
        // go-yaml (lefthook) reads `<<` outside a key as the string `<<`; PyYAML has no constructor for it.
        let ctor = if self.schema == Schema::Yaml12Core && tag == MERGE_TAG {
            Ctor::Str
        } else {
            ctor_for(tag)
        };
        let built = match ctor {
            Ctor::Null | Ctor::Bool | Ctor::Int | Ctor::Float
                if self.schema == Schema::Yaml12Core =>
            {
                Built::Scalar(construct_core12(
                    self.construct_scalar(id)?,
                    ctor,
                    node.start,
                )?)
            }
            Ctor::Gen(g) => {
                self.queue.push(id);
                Built::Pending(g)
            }
            Ctor::Undefined => {
                return Err(YamlError::constructor(undefined_tag(&node.tag), node.start));
            }
            Ctor::Null => {
                self.construct_scalar(id)?;
                Built::Scalar(Value::Null)
            }
            Ctor::Str => Built::Scalar(Value::String(self.construct_scalar(id)?.to_owned())),
            Ctor::Bool => Built::Scalar(construct_bool(self.construct_scalar(id)?)?),
            Ctor::Int => {
                let text = self.construct_scalar(id)?;
                let at = node.start;
                Built::Scalar(construct_int(text, &mut |n| self.spend(n, at))?)
            }
            Ctor::Float => Built::Scalar(construct_float(self.construct_scalar(id)?)?),
            Ctor::Binary => {
                let text = self.construct_scalar(id)?;
                Built::Scalar(
                    construct_binary(text).map_err(|p| YamlError::constructor(p, node.start))?,
                )
            }
            Ctor::Timestamp => {
                self.construct_scalar(id)?;
                match &node.kind {
                    NodeKind::Scalar(text) => Built::Scalar(construct_timestamp(text)?),
                    // `re.match(pattern, node.value)` with a list (a mapping reached through a `=` key).
                    _ => return Err(YamlError::python("TypeError", TIMESTAMP_NOT_STRING)),
                }
            }
        };
        self.set_built(id, built);
        Ok(())
    }

    /// `SafeConstructor.construct_scalar`: a mapping with a `=` key stands for that key's value.
    fn construct_scalar(&self, id: NodeId) -> Result<&'d str, YamlError> {
        let mut cur = id;
        // Each step follows a `=` key one mapping deeper (PyYAML recurses; a long chain or a cycle is its
        // RecursionError): at most `max_depth` steps, so that many nodes sharing one chain stay linear.
        for _ in 0..=self.lim.max_depth {
            let node = self.node(cur)?;
            match &node.kind {
                NodeKind::Scalar(s) => return Ok(s),
                NodeKind::Map(_) => {
                    let mut next = None;
                    for (k, v) in self.pairs_of(cur)? {
                        if self.tag(*k)? == VALUE_TAG {
                            next = Some(*v);
                            break;
                        }
                    }
                    match next {
                        Some(v) => cur = v,
                        None => {
                            return Err(YamlError::constructor(
                                expected_scalar(node.kind.py_id()),
                                node.start,
                            ));
                        }
                    }
                }
                NodeKind::Seq(_) => {
                    return Err(YamlError::constructor(
                        expected_scalar(node.kind.py_id()),
                        node.start,
                    ));
                }
            }
        }
        Err(YamlError::limit(
            limit_depth(self.lim.max_depth),
            Some(self.node(id)?.start),
        ))
    }

    /// Runs the body of a queued generator (`for dummy in generator: pass`).
    fn run_generator(&mut self, id: NodeId) -> Result<(), YamlError> {
        let Some(Some(Built::Pending(g))) = self.built.get(id.index()).cloned() else {
            return Ok(());
        };
        let node = self.node(id)?;
        let built = match g {
            Gen::Seq => {
                let NodeKind::Seq(items) = &node.kind else {
                    return Err(YamlError::constructor(
                        expected_sequence_node(node.kind.py_id()),
                        node.start,
                    ));
                };
                for item in items {
                    self.construct_object(*item)?;
                }
                Built::Seq(items.clone())
            }
            Gen::Map => {
                let pairs = self.construct_mapping(id)?;
                Built::Map(self.dict_entries(&pairs))
            }
            Gen::Set => {
                let pairs = self.construct_mapping(id)?;
                Built::Set(
                    self.dict_entries(&pairs)
                        .into_iter()
                        .map(|(k, _)| k)
                        .collect(),
                )
            }
            Gen::Omap | Gen::Pairs => Built::Pairs(self.construct_pairs(id, g)?),
        };
        self.set_built(id, built);
        Ok(())
    }

    /// `SafeConstructor.construct_mapping` (flatten, then `BaseConstructor.construct_mapping`).
    fn construct_mapping(&mut self, id: NodeId) -> Result<Vec<(NodeId, NodeId)>, YamlError> {
        let node = self.node(id)?;
        if !matches!(node.kind, NodeKind::Map(_)) {
            return Err(YamlError::constructor(
                expected_mapping_node(node.kind.py_id()),
                node.start,
            ));
        }
        self.flatten_mapping(id)?;
        let pairs = self.pairs_of(id)?.to_vec();
        for (k, v) in &pairs {
            self.construct_object(*k)?;
            if !matches!(self.built.get(k.index()), Some(Some(Built::Scalar(_)))) {
                return Err(YamlError::constructor_in(
                    WHILE_CONSTRUCTING_MAPPING,
                    node.start,
                    UNHASHABLE_KEY,
                    self.node(*k)?.start,
                ));
            }
            self.construct_object(*v)?;
        }
        Ok(pairs)
    }

    /// `construct_yaml_omap` / `construct_yaml_pairs` after `yield`.
    fn construct_pairs(&mut self, id: NodeId, g: Gen) -> Result<Vec<(NodeId, NodeId)>, YamlError> {
        let context = if g == Gen::Omap {
            WHILE_CONSTRUCTING_OMAP
        } else {
            WHILE_CONSTRUCTING_PAIRS
        };
        let node = self.node(id)?;
        let NodeKind::Seq(items) = &node.kind else {
            return Err(YamlError::constructor_in(
                context,
                node.start,
                expected_sequence(node.kind.py_id()),
                node.start,
            ));
        };
        let mut out = Vec::new();
        for sub in items {
            let subnode = self.node(*sub)?;
            if !matches!(subnode.kind, NodeKind::Map(_)) {
                return Err(YamlError::constructor_in(
                    context,
                    node.start,
                    expected_single_mapping(subnode.kind.py_id()),
                    subnode.start,
                ));
            }
            let pairs = self.pairs_of(*sub)?;
            let [(k, v)] = pairs else {
                return Err(YamlError::constructor_in(
                    context,
                    node.start,
                    expected_single_item(pairs.len()),
                    subnode.start,
                ));
            };
            let (k, v) = (*k, *v);
            self.construct_object(k)?;
            self.construct_object(v)?;
            out.push((k, v));
        }
        Ok(out)
    }

    /// `SafeConstructor.flatten_mapping(node)`, iteratively: `<<` pairs are deleted from `node.value` before the
    /// merged mapping is flattened (which is why `&a {<<: *a}` is `{}`), merged pairs go first, and earlier
    /// mappings of a merged sequence win over later ones; `=` keys become `str`.
    ///
    /// The work is linear in what it charges to the node budget: each nested flatten (a merged mapping or a
    /// sequence item), each `<<` key and each pair copied costs one unit; a deletion is a [`TOMBSTONE`], and the
    /// pairs a nested frame scans are the pairs it then copies.
    fn flatten_mapping(&mut self, root: NodeId) -> Result<(), YamlError> {
        let max_depth = usize::try_from(self.lim.max_depth).unwrap_or(usize::MAX);
        let mut stack = vec![FlattenFrame {
            node: root,
            index: 0,
            raw: 0,
            ver: self.version_of(root),
            merge: Vec::new(),
            resume: Resume::Scan,
        }];
        while let Some(frame) = stack.last_mut() {
            let node_id = frame.node;
            let node_start = self.node(node_id)?.start;
            // A nested flatten_mapping has returned: finish the step that called it.
            match std::mem::replace(&mut frame.resume, Resume::Scan) {
                Resume::Scan => {}
                Resume::MergedMap(v) => {
                    let p = self.live_pairs(v)?;
                    self.spend(p.len(), node_start)?;
                    let Some(frame) = stack.last_mut() else {
                        break;
                    };
                    frame.merge.extend(p);
                }
                Resume::MergedSeqItem {
                    seq,
                    pos,
                    mut submerge,
                } => {
                    let NodeKind::Seq(items) = &self.node(seq)?.kind else {
                        break;
                    };
                    if let Some(done) = items.get(pos).copied() {
                        let p = self.live_pairs(done)?;
                        self.spend(p.len(), node_start)?;
                        submerge.push(p);
                    }
                    let next = pos.saturating_add(1);
                    if let Some(sub) = items.get(next).copied() {
                        let subnode = self.node(sub)?;
                        if !matches!(subnode.kind, NodeKind::Map(_)) {
                            return Err(YamlError::constructor_in(
                                WHILE_CONSTRUCTING_MAPPING,
                                node_start,
                                merge_expected_mapping(subnode.kind.py_id()),
                                subnode.start,
                            ));
                        }
                        let Some(frame) = stack.last_mut() else {
                            break;
                        };
                        frame.resume = Resume::MergedSeqItem {
                            seq,
                            pos: next,
                            submerge,
                        };
                        self.push_flatten(&mut stack, sub, max_depth, subnode.start)?;
                        continue;
                    }
                    let Some(frame) = stack.last_mut() else {
                        break;
                    };
                    for value in submerge.into_iter().rev() {
                        frame.merge.extend(value);
                    }
                }
            }
            // The scan step: one pair of `node.value`, or the end of the loop.
            let Some((key, value)) = self.scan_next(&mut stack, node_id, node_start)? else {
                {
                    let merge = stack.pop().map(|f| f.merge).unwrap_or_default();
                    if !merge.is_empty() {
                        let own = self.live_pairs(node_id)?;
                        self.spend(own.len(), node_start)?;
                        let mut all = merge;
                        all.extend(own);
                        self.set_pairs(node_id, all);
                    } else if self
                        .pairs
                        .get(node_id.index())
                        .is_some_and(|p| p.as_ref().is_some_and(|p| p.contains(&TOMBSTONE)))
                    {
                        // Compact the deleted `<<` pairs away.
                        let own = self.live_pairs(node_id)?;
                        self.set_pairs(node_id, own);
                    }
                    continue;
                }
            };
            let key_tag = self.tag(key)?;
            if key_tag == MERGE_TAG {
                self.spend(1, node_start)?;
                // `del node.value[index]`: the stored pair at `raw` becomes a tombstone; `index` stays.
                let raw = stack.last().map_or(0, |f| f.raw);
                if let Some(list) = self.pairs_mut(node_id)?
                    && let Some(slot) = list.get_mut(raw)
                {
                    *slot = TOMBSTONE;
                }
                if let Some(frame) = stack.last_mut() {
                    frame.raw = raw.saturating_add(1);
                }
                let value_node = self.node(value)?;
                match &value_node.kind {
                    NodeKind::Map(_) => {
                        if let Some(frame) = stack.last_mut() {
                            frame.resume = Resume::MergedMap(value);
                        }
                        self.push_flatten(&mut stack, value, max_depth, value_node.start)?;
                    }
                    NodeKind::Seq(items) => {
                        let Some(first) = items.first().copied() else {
                            continue;
                        };
                        let first_node = self.node(first)?;
                        if !matches!(first_node.kind, NodeKind::Map(_)) {
                            return Err(YamlError::constructor_in(
                                WHILE_CONSTRUCTING_MAPPING,
                                node_start,
                                merge_expected_mapping(first_node.kind.py_id()),
                                first_node.start,
                            ));
                        }
                        if let Some(frame) = stack.last_mut() {
                            frame.resume = Resume::MergedSeqItem {
                                seq: value,
                                pos: 0,
                                submerge: Vec::new(),
                            };
                        }
                        self.push_flatten(&mut stack, first, max_depth, first_node.start)?;
                    }
                    NodeKind::Scalar(_) => {
                        return Err(YamlError::constructor_in(
                            WHILE_CONSTRUCTING_MAPPING,
                            node_start,
                            merge_expected_mapping_or_list(value_node.kind.py_id()),
                            value_node.start,
                        ));
                    }
                }
            } else {
                if key_tag == VALUE_TAG
                    && let Some(slot) = self.str_tag.get_mut(key.index())
                {
                    *slot = true;
                }
                if let Some(frame) = stack.last_mut() {
                    frame.index = frame.index.saturating_add(1);
                    frame.raw = frame.raw.saturating_add(1);
                }
            }
        }
        Ok(())
    }

    /// `node.value[index]` of the top frame (`None` past the end), skipping [`TOMBSTONE`]s; leaves the frame's
    /// `raw` at the returned pair. When the list was compacted or replaced (a cycle flattened the same mapping
    /// in a nested frame), the position is found again from `index`.
    fn scan_next(
        &mut self,
        stack: &mut [FlattenFrame],
        node_id: NodeId,
        at: Mark,
    ) -> Result<Option<(NodeId, NodeId)>, YamlError> {
        let ver = self.version_of(node_id);
        let nested = stack.len() > 1;
        let Some(frame) = stack.last() else {
            return Ok(None);
        };
        let (index, mut raw, stale) = (frame.index, frame.raw, frame.ver != ver);
        let list = self.pairs_of(node_id)?;
        let mut steps = if stale {
            // The live index → stored position (O(len), charged).
            let mut live = 0usize;
            raw = 0;
            while let Some(p) = list.get(raw) {
                if *p != TOMBSTONE {
                    if live == index {
                        break;
                    }
                    live = live.saturating_add(1);
                }
                raw = raw.saturating_add(1);
            }
            raw
        } else {
            0
        };
        while list.get(raw) == Some(&TOMBSTONE) {
            raw = raw.saturating_add(1);
            steps = steps.saturating_add(1);
        }
        let pair = list.get(raw).copied();
        if let Some(frame) = stack.last_mut() {
            frame.raw = raw;
            frame.ver = ver;
        }
        if nested || stale {
            self.spend(steps, at)?;
        }
        Ok(pair)
    }

    /// Starts a nested `flatten_mapping(node)` (one unit of merge work).
    fn push_flatten(
        &mut self,
        stack: &mut Vec<FlattenFrame>,
        node: NodeId,
        max_depth: usize,
        at: Mark,
    ) -> Result<(), YamlError> {
        if stack.len() >= max_depth {
            return Err(YamlError::limit(limit_depth(self.lim.max_depth), Some(at)));
        }
        self.spend(1, at)?;
        stack.push(FlattenFrame {
            node,
            index: 0,
            raw: 0,
            ver: self.version_of(node),
            merge: Vec::new(),
            resume: Resume::Scan,
        });
        Ok(())
    }

    // --- phase 2: cycles and limits ----------------------------------------------------------------------------

    /// The children of a built node, each with the extra node count it adds besides its own subtree (a tuple
    /// of `!!omap`/`!!pairs` is one more node above its key and value).
    fn children(&self, id: NodeId) -> Vec<NodeId> {
        match self.built.get(id.index()) {
            Some(Some(Built::Seq(items) | Built::Set(items))) => items.clone(),
            Some(Some(Built::Map(pairs) | Built::Pairs(pairs))) => {
                pairs.iter().flat_map(|(k, v)| [*k, *v]).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The extra nesting levels and nodes a collection adds above its children (`list` → 1; a list of tuples →
    /// 2 levels and one node per tuple).
    fn shape(&self, id: NodeId) -> (u64, u64) {
        match self.built.get(id.index()) {
            Some(Some(Built::Pairs(pairs))) => (2, u64::try_from(pairs.len()).unwrap_or(u64::MAX)),
            Some(Some(Built::Seq(_) | Built::Map(_) | Built::Set(_))) => (1, 0),
            _ => (0, 0),
        }
    }

    /// Child `i` of a built node (a mapping's children are key, value, key, value, …).
    fn child(&self, id: NodeId, i: usize) -> Option<NodeId> {
        match self.built.get(id.index()) {
            Some(Some(Built::Seq(items) | Built::Set(items))) => items.get(i).copied(),
            Some(Some(Built::Map(pairs) | Built::Pairs(pairs))) => pairs
                .get(i.wrapping_div(2))
                .map(|(k, v)| if i.wrapping_rem(2) == 0 { *k } else { *v }),
            _ => None,
        }
    }

    /// Checks the expanded size (nodes, aliases counted at every use) and depth of the root, rejecting cycles.
    fn check_limits(&self, root: NodeId) -> Result<(), YamlError> {
        /// A node being measured: the next child to visit and the totals so far.
        struct Frame {
            id: NodeId,
            next: usize,
            size: u64,
            depth: u64,
        }
        let n = self.doc.nodes.len();
        let mut size = vec![0u64; n];
        let mut depth = vec![0u64; n];
        // 0 = new, 1 = on the stack, 2 = done.
        let mut state = vec![0u8; n];
        let open = |id: NodeId, state: &mut Vec<u8>| {
            if let Some(s) = state.get_mut(id.index()) {
                *s = 1;
            }
            Frame {
                id,
                next: 0,
                size: 1u64.saturating_add(self.shape(id).1),
                depth: 0,
            }
        };
        let mut stack = vec![open(root, &mut state)];
        while let Some(top) = stack.last_mut() {
            if let Some(child) = self.child(top.id, top.next) {
                top.next = top.next.saturating_add(1);
                match state.get(child.index()).copied() {
                    Some(0) => stack.push(open(child, &mut state)),
                    Some(1) => {
                        return Err(YamlError::constructor(
                            UNCONSTRUCTABLE_RECURSIVE,
                            self.node(child)?.start,
                        ));
                    }
                    _ => {
                        top.size = top
                            .size
                            .saturating_add(size.get(child.index()).copied().unwrap_or(0));
                        top.depth = top
                            .depth
                            .max(depth.get(child.index()).copied().unwrap_or(0));
                    }
                }
                continue;
            }
            let Some(done) = stack.pop() else {
                break;
            };
            let total_depth = done.depth.saturating_add(self.shape(done.id).0);
            if let Some(slot) = size.get_mut(done.id.index()) {
                *slot = done.size;
            }
            if let Some(slot) = depth.get_mut(done.id.index()) {
                *slot = total_depth;
            }
            if let Some(slot) = state.get_mut(done.id.index()) {
                *slot = 2;
            }
            if let Some(parent) = stack.last_mut() {
                parent.size = parent.size.saturating_add(done.size);
                parent.depth = parent.depth.max(total_depth);
            }
        }
        let total = size.get(root.index()).copied().unwrap_or(0);
        if total > u64::from(self.lim.max_constructed_nodes) {
            let at = self.find_budget_crossing(root, &size)?;
            return Err(self.limit_nodes_error(at));
        }
        if depth.get(root.index()).copied().unwrap_or(0) > u64::from(self.lim.max_depth) {
            let at = self.find_too_deep(root, &depth)?;
            return Err(YamlError::limit(limit_depth(self.lim.max_depth), Some(at)));
        }
        Ok(())
    }

    /// The first node (in construction order) that no longer fits the node budget.
    fn find_budget_crossing(&self, root: NodeId, size: &[u64]) -> Result<Mark, YamlError> {
        let limit = u64::from(self.lim.max_constructed_nodes);
        let mut cur = root;
        let (_, extra) = self.shape(root);
        let mut used = 1u64.saturating_add(extra);
        'descend: loop {
            for c in self.children(cur) {
                let s = size.get(c.index()).copied().unwrap_or(0);
                if used.saturating_add(s) > limit {
                    if s > 1 && used < limit {
                        let (_, extra) = self.shape(c);
                        used = used.saturating_add(1).saturating_add(extra);
                        cur = c;
                        continue 'descend;
                    }
                    return Ok(self.node(c)?.start);
                }
                used = used.saturating_add(s);
            }
            return Ok(self.node(cur)?.start);
        }
    }

    /// The first collection nested deeper than `max_depth`.
    fn find_too_deep(&self, root: NodeId, depth: &[u64]) -> Result<Mark, YamlError> {
        let max = u64::from(self.lim.max_depth);
        let mut cur = root;
        let mut level: u64 = self.shape(root).0;
        while level <= max {
            let deepest = self
                .children(cur)
                .into_iter()
                .max_by_key(|c| depth.get(c.index()).copied().unwrap_or(0));
            match deepest {
                Some(c) if depth.get(c.index()).copied().unwrap_or(0) > 0 => {
                    level = level.saturating_add(self.shape(c).0);
                    cur = c;
                }
                _ => break,
            }
        }
        Ok(self.node(cur)?.start)
    }

    /// `dict.__setitem__` over constructed pairs: equal keys (Python equality, `1 == 1.0 == True`) keep the first
    /// key and its position and take the last value.
    fn dict_entries(&self, pairs: &[(NodeId, NodeId)]) -> Vec<(NodeId, NodeId)> {
        let mut index: IndexMap<&Value, usize> = IndexMap::new();
        let mut out: Vec<(NodeId, NodeId)> = Vec::new();
        for (k, v) in pairs {
            let Some(Some(Built::Scalar(key))) = self.built.get(k.index()) else {
                continue;
            };
            if let Some(i) = index.get(key).copied() {
                if let Some(slot) = out.get_mut(i) {
                    slot.1 = *v;
                }
            } else {
                index.insert(key, out.len());
                out.push((*k, *v));
            }
        }
        out
    }
}

impl Graph {
    // --- phase 3: materialisation ------------------------------------------------------------------------------

    /// Builds the `cfgv::Value` tree (an explicit stack; at most `max_constructed_nodes` nodes, checked in
    /// phase 2).
    fn materialize(&self, root: NodeId) -> Result<Value, YamlError> {
        enum Task {
            Enter(NodeId),
            Seq(usize),
            Map(usize),
            Set(usize),
            Pairs(usize),
        }
        let mut tasks = vec![Task::Enter(root)];
        let mut values: Vec<Value> = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Enter(id) => match self.built.get(id.index()) {
                    Some(Some(Built::Scalar(v))) => values.push(v.clone()),
                    Some(Some(Built::Seq(items))) => {
                        tasks.push(Task::Seq(items.len()));
                        tasks.extend(items.iter().rev().map(|c| Task::Enter(*c)));
                    }
                    Some(Some(Built::Set(items))) => {
                        tasks.push(Task::Set(items.len()));
                        tasks.extend(items.iter().rev().map(|c| Task::Enter(*c)));
                    }
                    Some(Some(Built::Map(pairs))) => {
                        tasks.push(Task::Map(pairs.len()));
                        for (k, v) in pairs.iter().rev() {
                            tasks.push(Task::Enter(*v));
                            tasks.push(Task::Enter(*k));
                        }
                    }
                    Some(Some(Built::Pairs(pairs))) => {
                        tasks.push(Task::Pairs(pairs.len()));
                        for (k, v) in pairs.iter().rev() {
                            tasks.push(Task::Enter(*v));
                            tasks.push(Task::Enter(*k));
                        }
                    }
                    _ => {
                        return Err(YamlError::marked(
                            "YAMLError",
                            None,
                            None,
                            INVALID_DOCUMENT,
                            None,
                        ));
                    }
                },
                Task::Seq(n) => {
                    let items = pop_n(&mut values, n);
                    values.push(Value::List(items));
                }
                Task::Set(n) => {
                    let mut set = Set::new();
                    for k in pop_n(&mut values, n) {
                        set.insert(k);
                    }
                    values.push(Value::Set(set));
                }
                Task::Map(n) => {
                    let flat = pop_n(&mut values, n.saturating_mul(2));
                    let mut map = Map::new();
                    let mut it = flat.into_iter();
                    while let (Some(k), Some(v)) = (it.next(), it.next()) {
                        // Python dict semantics: an equal key keeps its first position and object, the value
                        // is replaced (`IndexMap::insert`).
                        map.insert(k, v);
                    }
                    values.push(Value::Map(map));
                }
                Task::Pairs(n) => {
                    let flat = pop_n(&mut values, n.saturating_mul(2));
                    let mut list = Vec::new();
                    let mut it = flat.into_iter();
                    while let (Some(k), Some(v)) = (it.next(), it.next()) {
                        list.push(Value::Tuple(vec![k, v]));
                    }
                    values.push(Value::List(list));
                }
            }
        }
        values
            .pop()
            .ok_or_else(|| YamlError::marked("YAMLError", None, None, INVALID_DOCUMENT, None))
    }
}

/// The last `n` values, in order.
fn pop_n(values: &mut Vec<Value>, n: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for _ in 0..n {
        match values.pop() {
            Some(v) => out.push(v),
            None => break,
        }
    }
    out.reverse();
    out
}

// --- scalar constructors --------------------------------------------------------------------------------------

/// `construct_yaml_bool`: `bool_values[value.lower()]`, else `KeyError` (whose `str` is the key's repr).
fn construct_bool(value: &str) -> Result<Value, YamlError> {
    let lower = py_lower(value);
    match lower.as_str() {
        "yes" | "true" | "on" => Ok(Value::Bool(true)),
        "no" | "false" | "off" => Ok(Value::Bool(false)),
        _ => Err(YamlError::python(
            "KeyError",
            snp_pycompat::repr::repr_str(&lower),
        )),
    }
}

/// A `ValueError` raised by `int()` / `float()` / `datetime`.
fn value_error(text: impl Into<String>) -> YamlError {
    YamlError::python("ValueError", text)
}

/// `value[0]` on an empty string.
fn index_error() -> YamlError {
    YamlError::python("IndexError", STRING_INDEX_OUT_OF_RANGE)
}

/// go-yaml v3's `resolve(tag, text)` for an explicit or resolved `!!null`/`!!bool`/`!!int`/`!!float` in
/// [`Schema::Yaml12Core`]: the text must resolve to that tag (an int is accepted as a float), else go-yaml's
/// ``cannot decode !!str `yes` as a !!bool``.
fn construct_core12(text: &str, ctor: Ctor, at: Mark) -> Result<Value, YamlError> {
    let r = go_resolve(text);
    let (value, want) = match (ctor, r) {
        (Ctor::Null, GoScalar::Null) => (Some(Value::Null), "!!null"),
        (Ctor::Bool, GoScalar::Bool(b)) => (Some(Value::Bool(b)), "!!bool"),
        (Ctor::Int, GoScalar::Int(i)) => (Some(Value::Int(i)), "!!int"),
        (Ctor::Int, GoScalar::Uint(u)) => (Some(Value::BigInt(BigInt::from(u))), "!!int"),
        (Ctor::Float, GoScalar::Float(x)) => (Some(Value::Float(x)), "!!float"),
        (Ctor::Float, GoScalar::Int(i)) => (Some(Value::Float(go_float64(i))), "!!float"),
        (Ctor::Null, _) => (None, "!!null"),
        (Ctor::Bool, _) => (None, "!!bool"),
        (Ctor::Int, _) => (None, "!!int"),
        _ => (None, "!!float"),
    };
    value.ok_or_else(|| {
        YamlError::constructor(
            format!("cannot decode {} `{text}` as a {want}", r.short_tag()),
            at,
        )
    })
}

/// Go's `float64(i)`: round to nearest, ties to even — what Rust's `as` does.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "the rounding conversion Go performs; no other API rounds an i64"
)]
const fn go_float64(i: i64) -> f64 {
    i as f64
}

/// `construct_yaml_int` (YAML 1.1). A sexagesimal int charges its work (parts × limbs) to the node budget:
/// CPython is quadratic there too, and nothing else bounds a 16 MiB `!!int "1:1:…"`.
fn construct_int(
    value: &str,
    spend: &mut dyn FnMut(usize) -> Result<(), YamlError>,
) -> Result<Value, YamlError> {
    let cleaned = value.replace('_', "");
    let first = cleaned.chars().next().ok_or_else(index_error)?;
    let negative = first == '-';
    let v = cleaned.strip_prefix(['+', '-']).unwrap_or(&cleaned);
    let mut big = if v == "0" {
        Big::zero()
    } else if let Some(bits) = v.strip_prefix("0b") {
        py_int(bits, 2).map_err(value_error)?
    } else if let Some(hex) = v.strip_prefix("0x") {
        py_int(hex, 16).map_err(value_error)?
    } else if v.chars().next().ok_or_else(index_error)? == '0' {
        py_int(v, 8).map_err(value_error)?
    } else if v.contains(':') {
        // digits = [int(part) for part in value.split(':')]; then Σ digit * 60**i from the right.
        let mut parts = Vec::new();
        for part in v.split(':') {
            parts.push(py_int(part, 10).map_err(value_error)?);
        }
        let mut acc = Big::zero();
        for part in &parts {
            spend(acc.limbs().saturating_add(1))?;
            acc.mul_add_small(60, 0);
            acc.add(part);
        }
        acc
    } else {
        py_int(v, 10).map_err(value_error)?
    };
    if negative {
        big.negate();
    }
    Ok(big.to_value())
}

/// `construct_yaml_float`.
fn construct_float(value: &str) -> Result<Value, YamlError> {
    let cleaned = py_lower(&value.replace('_', ""));
    let first = cleaned.chars().next().ok_or_else(index_error)?;
    let sign = if first == '-' { -1.0 } else { 1.0 };
    let v = cleaned.strip_prefix(['+', '-']).unwrap_or(&cleaned);
    let x = if v == ".inf" {
        sign * f64::INFINITY
    } else if v == ".nan" {
        f64::NAN
    } else if v.contains(':') {
        let mut digits = Vec::new();
        for part in v.split(':') {
            digits.push(py_float(part).map_err(value_error)?);
        }
        // value = 0.0; base = 1; for digit in reversed(digits): value += digit*base; base *= 60
        let mut acc = 0.0f64;
        let mut base = Big::from_ascii_digits(false, b"1", 10);
        for digit in digits.iter().rev() {
            let b = base
                .to_f64()
                .ok_or_else(|| YamlError::python("OverflowError", INT_TOO_LARGE_FOR_FLOAT))?;
            acc += digit * b;
            base.mul_add_small(60, 0);
        }
        sign * acc
    } else {
        sign * py_float(v).map_err(value_error)?
    };
    Ok(Value::Float(x))
}

/// `construct_yaml_binary`: `.encode('ascii')` then `base64.decodebytes`; the error is the `ConstructorError`
/// problem.
fn construct_binary(value: &str) -> Result<Value, Cow<'static, str>> {
    if let Some((start, bad)) = value.chars().enumerate().find(|(_, c)| !c.is_ascii()) {
        let run = value
            .chars()
            .skip(start)
            .take_while(|c| !c.is_ascii())
            .count();
        return Err(
            binary_not_ascii(&ascii_encode_error(bad, start, start.saturating_add(run))).into(),
        );
    }
    base64::decode(value.as_bytes())
        .map(Value::Bytes)
        .map_err(|e| binary_bad_base64(&e).into())
}

/// `construct_yaml_timestamp` on `node.value`.
fn construct_timestamp(value: &str) -> Result<Value, YamlError> {
    let parts = match_timestamp(value)
        .ok_or_else(|| YamlError::python("AttributeError", TIMESTAMP_NO_MATCH))?;
    let num = |s: &str| s.parse::<u32>().unwrap_or(0);
    let year = i64::from(num(parts.year));
    let month = num(parts.month);
    let day = num(parts.day);
    let date_err = |e: DateError| value_error(e.to_string());
    let Some(time) = parts.time else {
        return Date::new(year, month, day)
            .map(Value::Date)
            .map_err(date_err);
    };
    // fraction = values['fraction'][:6] padded with '0' to 6 digits.
    let micro = match time.fraction {
        Some(f) if !f.is_empty() => {
            let mut digits: String = f.chars().take(6).collect();
            while digits.len() < 6 {
                digits.push('0');
            }
            num(&digits)
        }
        _ => 0,
    };
    // The tzinfo is built before datetime() validates the date and the time.
    let tz = match time.tz {
        None => None,
        Some(Tz::Utc) => Some(TzOffset::UTC),
        Some(Tz::Offset {
            negative,
            hour,
            minute,
        }) => Some(
            TzOffset::from_hours_minutes(negative, num(hour), minute.map_or(0, num))
                .map_err(date_err)?,
        ),
    };
    let date = Date::new(year, month, day).map_err(date_err)?;
    DateTime::new(
        date,
        num(time.hour),
        num(time.minute),
        num(time.second),
        micro,
        tz,
    )
    .map(Value::DateTime)
    .map_err(date_err)
}
