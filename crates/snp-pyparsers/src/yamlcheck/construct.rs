//! `ruamel/yaml/constructor.py` (`BaseConstructor` + `SafeConstructor`): the node graph and the constructor's
//! state (generators, `deep_construct`, the object caches) with Python's evaluation order — collections are
//! generators whose bodies run breadth-first from `construct_document` unless `deep_construct` is set, which is
//! what decides the printed values (`"[]"` for a not yet filled list) and the order of errors.
//!
//! Deep construction recurses once per nesting level (at least four Python frames each, so at most ≈ 250
//! levels before Python's `RecursionError`); `flatten_mapping` and the node `repr` run on explicit stacks.
//!
//! Owner: WP-17.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::int::{BigInt, py_float, py_int};
use super::obj::{
    DateTime, Dict, Obj, ObjId, Objs, Ymd, days_in_month, repr_pystr, timedelta_repr,
};
use super::resolver::{STR, TsGroups, timestamp_groups};
use super::{Fail, Loader, Mark, R, Rx, marked};
use crate::json::PyMinor;
use crate::pystr;

pub(super) type NodeId = u32;

/// A node: `tag` is the resolved tag's `trval`.
#[derive(Debug, Clone)]
pub(super) struct Node {
    pub(super) tag: String,
    pub(super) kind: NodeKind,
    pub(super) start: Mark,
    pub(super) end: Mark,
    /// `MappingNode.merge`, set by `flatten_mapping`.
    pub(super) merge: Option<Vec<(NodeId, NodeId)>>,
    pub(super) anchor: Option<String>,
    /// Whether the node's `Tag` object has its `trval` cached (the first `node.tag` computes it one frame
    /// deeper, in `uri_decoded_suffix`).
    pub(super) tag_cache: TagCache,
}

/// Where a node's `Tag` object comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TagCache {
    /// `trval` already computed (the parser's tags: `str(tag)` ran in `parse_node`).
    Cached,
    /// A new `Tag(suffix=…)` (an implicit resolution, the `tag` setter).
    Fresh,
    /// `BaseResolver.DEFAULT_{SCALAR,SEQUENCE,MAPPING}_TAG`: one object per process.
    Default(u8),
}

#[derive(Debug, Clone)]
pub(super) enum NodeKind {
    Scalar(Vec<u8>),
    Seq(Vec<NodeId>),
    Map(Vec<(NodeId, NodeId)>),
}

impl NodeKind {
    /// `Node.id`.
    const fn id(&self) -> &'static str {
        match self {
            Self::Scalar(_) => "scalar",
            Self::Seq(_) => "sequence",
            Self::Map(_) => "mapping",
        }
    }
}

/// The collection constructors (generator functions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GenKind {
    Seq,
    Map,
    Set,
    Omap,
    Pairs,
}

/// A generator suspended after its first `yield`.
#[derive(Debug, Clone, Copy)]
pub(super) struct Gen {
    kind: GenKind,
    node: NodeId,
    obj: ObjId,
}

/// An activation of the constructor machine (a Python frame that calls `construct_object`).
enum Act {
    /// `construct_object(node, deep)`; `started` once `construct_non_recursive_object` was called.
    Object {
        node: NodeId,
        deep: bool,
        saved: u32,
        started: bool,
        old_deep: bool,
    },
    /// `construct_non_recursive_object(node)`; `obj` while a deep generator body runs.
    Cnro {
        node: NodeId,
        saved: u32,
        obj: Option<ObjId>,
    },
    /// A generator body after its `yield`.
    Body(Body),
}

impl Act {
    fn body(g: Gen, saved: u32) -> Self {
        Self::Body(match g.kind {
            GenKind::Seq => Body::Seq {
                g,
                saved,
                inner: None,
                idx: 0,
                items: Vec::new(),
            },
            GenKind::Map | GenKind::Set => Body::Map(Box::new(MapBody {
                g,
                saved,
                outer: saved,
                base: saved,
                phase: MapPhase::Start,
                todo: Vec::new(),
                todo_i: 0,
                pair_i: 0,
                key: 0,
                mapping: Dict::default(),
                total: Dict::default(),
            })),
            GenKind::Omap | GenKind::Pairs => Body::Omap {
                g,
                saved,
                idx: 0,
                key: None,
                started: false,
            },
        })
    }
}

enum Body {
    Seq {
        g: Gen,
        saved: u32,
        /// The `construct_sequence` frame, once entered.
        inner: Option<u32>,
        idx: usize,
        items: Vec<ObjId>,
    },
    Map(Box<MapBody>),
    Omap {
        g: Gen,
        saved: u32,
        idx: usize,
        key: Option<ObjId>,
        started: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MapPhase {
    Start,
    NextPair,
    GotKey,
    GotValue,
}

struct MapBody {
    g: Gen,
    saved: u32,
    outer: u32,
    base: u32,
    phase: MapPhase,
    todo: Vec<(Vec<(NodeId, NodeId)>, bool)>,
    todo_i: usize,
    pair_i: usize,
    key: ObjId,
    mapping: Dict,
    total: Dict,
}

/// What a body step asks the machine to do.
enum Step {
    /// `construct_object(node, deep)`.
    Call(NodeId, bool),
    /// The body returned; restore this depth.
    Done(u32),
}

/// The constructor's state, which survives a failed document (and file).
#[derive(Debug)]
pub(super) struct ConsState {
    nodes: Vec<Node>,
    objs: Objs,
    constructed: HashMap<NodeId, ObjId>,
    recursive: HashSet<NodeId>,
    generators: Vec<Gen>,
    deep: bool,
    dummy: Node,
}

impl Default for ConsState {
    fn default() -> Self {
        let m = Mark {
            name: Rc::from(""),
            line: 0,
            column: 0,
        };
        Self {
            nodes: Vec::new(),
            objs: Objs::default(),
            constructed: HashMap::new(),
            recursive: HashSet::new(),
            generators: Vec::new(),
            deep: false,
            dummy: Node {
                tag: String::new(),
                kind: NodeKind::Scalar(Vec::new()),
                start: m.clone(),
                end: m,
                merge: None,
                anchor: None,
                tag_cache: TagCache::Cached,
            },
        }
    }
}

impl ConsState {
    /// Drops the arenas when nothing refers to them any more.
    pub(super) fn gc(&mut self) {
        if self.generators.is_empty() && self.constructed.is_empty() && self.recursive.is_empty() {
            self.nodes.clear();
            self.objs.v.clear();
        }
    }

    pub(super) fn push_node(&mut self, n: Node) -> NodeId {
        self.nodes.push(n);
        NodeId::try_from(self.nodes.len().saturating_sub(1)).unwrap_or(NodeId::MAX)
    }

    pub(super) fn node(&self, id: NodeId) -> &Node {
        self.nodes
            .get(usize::try_from(id).unwrap_or(usize::MAX))
            .unwrap_or(&self.dummy)
    }

    pub(super) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        match self
            .nodes
            .get_mut(usize::try_from(id).unwrap_or(usize::MAX))
        {
            Some(n) => n,
            None => &mut self.dummy,
        }
    }
}

const NULL: &str = "tag:yaml.org,2002:null";
const BOOL: &str = "tag:yaml.org,2002:bool";
const INT: &str = "tag:yaml.org,2002:int";
const FLOAT: &str = "tag:yaml.org,2002:float";
const BINARY: &str = "tag:yaml.org,2002:binary";
const TIMESTAMP: &str = "tag:yaml.org,2002:timestamp";
const OMAP: &str = "tag:yaml.org,2002:omap";
const PAIRS: &str = "tag:yaml.org,2002:pairs";
const SET: &str = "tag:yaml.org,2002:set";
const SEQ: &str = "tag:yaml.org,2002:seq";
const MAP: &str = "tag:yaml.org,2002:map";
const MERGE: &str = "tag:yaml.org,2002:merge";
const VALUE: &str = "tag:yaml.org,2002:value";

const DUPKEY_NOTE: &str =
    "\nTo suppress this check see:\n    https://yaml.dev/doc/ruamel.yaml/api/#Duplicate_keys\n";
const MERGE_NOTE: &str = "Duplicate merge keys are never allowed, not even when\n`.allow_duplicate_keys` is set to True\n";

fn type_error(name: &str) -> Fail {
    Fail::crash("TypeError", format!("unhashable type: '{name}'"))
}

/// `str.lower()` on generalized UTF-8 (lone surrogates unchanged).
fn py_lower(s: &[u8]) -> Vec<u8> {
    if let Ok(t) = std::str::from_utf8(s) {
        return t.to_lowercase().into_bytes();
    }
    let mut out = Vec::new();
    let mut run = String::new();
    for cp in pystr::code_points(s) {
        if let Some(c) = char::from_u32(cp) {
            run.push(c);
        } else {
            out.extend_from_slice(run.to_lowercase().as_bytes());
            run.clear();
            pystr::push_cp(&mut out, cp);
        }
    }
    out.extend_from_slice(run.to_lowercase().as_bytes());
    out
}

/// `'ascii'` encode error text for the first run of non-ASCII code points.
fn ascii_encode_error(s: &[u8]) -> Option<String> {
    let cps: Vec<u32> = pystr::code_points(s).collect();
    let start = cps.iter().position(|&c| c >= 0x80)?;
    let end = cps.get(start..).map_or(start, |r| {
        start.saturating_add(r.iter().take_while(|&&c| c >= 0x80).count())
    });
    Some(if end.saturating_sub(start) == 1 {
        let c = cps.get(start).copied().unwrap_or(0);
        let esc = if c <= 0xff {
            format!("\\x{c:02x}")
        } else if c <= 0xffff {
            format!("\\u{c:04x}")
        } else {
            format!("\\U{c:08x}")
        };
        format!(
            "'ascii' codec can't encode character '{esc}' in position {start}: ordinal not in range(128)"
        )
    } else {
        format!(
            "'ascii' codec can't encode characters in position {start}-{}: ordinal not in range(128)",
            end.saturating_sub(1)
        )
    })
}

/// `binascii.a2b_base64(data)` (non-strict): `Err` is the `binascii.Error` text.
fn a2b_base64(data: &[u8], v: PyMinor) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut quad_pos = 0u8;
    let mut leftchar = 0u8;
    let mut pads = 0u8;
    for &ch in data {
        if ch == b'=' {
            if quad_pos >= 2 {
                pads = pads.saturating_add(1);
                if quad_pos.saturating_add(pads) >= 4 && matches!(v, PyMinor::P312) {
                    return Ok(out);
                }
            }
            continue;
        }
        let val = match ch {
            b'A'..=b'Z' => ch.wrapping_sub(b'A'),
            b'a'..=b'z' => ch.wrapping_sub(b'a').wrapping_add(26),
            b'0'..=b'9' => ch.wrapping_sub(b'0').wrapping_add(52),
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        pads = 0;
        match quad_pos {
            0 => {
                quad_pos = 1;
                leftchar = val;
            }
            1 => {
                quad_pos = 2;
                out.push((leftchar << 2) | (val >> 4));
                leftchar = val & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push((leftchar << 4) | (val >> 2));
                leftchar = val & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push((leftchar << 6) | val);
                leftchar = 0;
            }
        }
    }
    if matches!(v, PyMinor::P313) && quad_pos != 0 && quad_pos.saturating_add(pads) >= 4 {
        return Ok(out);
    }
    match quad_pos {
        0 => Ok(out),
        1 => Err(format!(
            "Invalid base64-encoded string: number of data characters ({}) cannot be 1 more than a multiple of 4",
            (out.len() / 3).saturating_mul(4).saturating_add(1)
        )),
        _ => Err("Incorrect padding".to_owned()),
    }
}

/// `datetime.date(y, m, d)` validation.
fn check_date(y: i64, m: i64, d: i64) -> Result<Ymd, Fail> {
    if !(1..=9999).contains(&y) {
        return Err(Fail::crash(
            "ValueError",
            format!("year {y} is out of range"),
        ));
    }
    if !(1..=12).contains(&m) {
        return Err(Fail::crash("ValueError", "month must be in 1..12"));
    }
    if d < 1 || d > days_in_month(y, m) {
        return Err(Fail::crash("ValueError", "day is out of range for month"));
    }
    Ok(Ymd { y, m, d })
}

fn ascii_int(s: &str) -> i64 {
    s.bytes().fold(0i64, |a, b| {
        a.saturating_mul(10)
            .saturating_add(i64::from(b.wrapping_sub(b'0')))
    })
}

/// `util.create_timestamp(**groups)`.
fn create_timestamp(g: &TsGroups) -> Result<Obj, Fail> {
    let (year, month, day) = (ascii_int(&g.year), ascii_int(&g.month), ascii_int(&g.day));
    let Some(hour) = &g.hour else {
        return Ok(Obj::Date(check_date(year, month, day)?));
    };
    let hour = ascii_int(hour);
    let minute = ascii_int(&g.minute);
    let second = ascii_int(&g.second);
    let mut frac = 0i64;
    let fraction = if let Some(f) = g.fraction.as_deref().filter(|f| !f.is_empty()) {
        let mut fs: String = f.chars().take(6).collect();
        while fs.len() < 6 {
            fs.push('0');
        }
        frac = ascii_int(&fs);
        if f.len() > 6 && f.as_bytes().get(6).is_some_and(|&b| b > b'4') {
            frac = frac.saturating_add(1);
        }
        if frac > 999_999 { 0 } else { frac }
    } else {
        0
    };
    let mut tz = None;
    if let Some(sign) = g.tz_sign {
        let th = ascii_int(&g.tz_hour);
        let tm = g.tz_minute.as_deref().map_or(0, ascii_int);
        let mut secs = th
            .saturating_mul(3600)
            .saturating_add(tm.saturating_mul(60));
        if sign == '-' {
            secs = secs.saturating_neg();
        }
        if secs.unsigned_abs() >= 86_400 {
            return Err(Fail::crash(
                "ValueError",
                format!(
                    "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), not {}.",
                    timedelta_repr(secs)
                ),
            ));
        }
        tz = Some((secs, g.tz.clone().unwrap_or_default()));
    } else if g.tz.as_deref() == Some("Z") {
        tz = Some((0, "Z".to_owned()));
    }
    let date = check_date(year, month, day)?;
    if !(0..=23).contains(&hour) {
        return Err(Fail::crash("ValueError", "hour must be in 0..23"));
    }
    if !(0..=59).contains(&minute) {
        return Err(Fail::crash("ValueError", "minute must be in 0..59"));
    }
    if !(0..=59).contains(&second) {
        return Err(Fail::crash("ValueError", "second must be in 0..59"));
    }
    let mut dt = DateTime {
        date,
        hour,
        minute,
        second,
        micro: fraction,
        tz,
    };
    if frac > 999_999 {
        // data -= -timedelta(seconds=1)
        dt.second = dt.second.saturating_add(1);
        if dt.second == 60 {
            dt.second = 0;
            dt.minute = dt.minute.saturating_add(1);
            if dt.minute == 60 {
                dt.minute = 0;
                dt.hour = dt.hour.saturating_add(1);
                if dt.hour == 24 {
                    dt.hour = 0;
                    let mut d = dt.date;
                    d.d = d.d.saturating_add(1);
                    if d.d > days_in_month(d.y, d.m) {
                        d.d = 1;
                        d.m = d.m.saturating_add(1);
                        if d.m > 12 {
                            d.m = 1;
                            d.y = d.y.saturating_add(1);
                            if d.y > 9999 {
                                return Err(Fail::crash(
                                    "OverflowError",
                                    "date value out of range",
                                ));
                            }
                        }
                    }
                    dt.date = d;
                }
            }
        }
    }
    Ok(Obj::DateTime(dt))
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the loader's methods live next to the ruamel module they port"
)]
impl Loader<'_, '_> {
    /// `node.tag` (the property, `Tag.__str__` and `trval`; `uri_decoded_suffix` the first time).
    fn tag_access(&mut self, id: NodeId) -> R<()> {
        let fresh = match self.st.cons.node(id).tag_cache {
            TagCache::Cached => false,
            TagCache::Fresh => true,
            TagCache::Default(i) => !self
                .st
                .default_tags_cached
                .get(usize::from(i))
                .copied()
                .unwrap_or(true),
        };
        self.touch(if fresh { 4 } else { 3 })?;
        match self.st.cons.node(id).tag_cache {
            TagCache::Default(i) => {
                if let Some(c) = self.st.default_tags_cached.get_mut(usize::from(i)) {
                    *c = true;
                }
            }
            _ => self.st.cons.node_mut(id).tag_cache = TagCache::Cached,
        }
        Ok(())
    }

    fn objs(&self) -> &Objs {
        &self.st.cons.objs
    }

    fn new_obj(&mut self, o: Obj) -> ObjId {
        self.st.cons.objs.push(o)
    }

    /// `ConstructorError(context, context_mark, problem, problem_mark)`.
    fn cons_err<T>(
        &mut self,
        context: Option<&str>,
        cm: Option<&Mark>,
        problem: &[u8],
        pm: &Mark,
    ) -> R<T> {
        self.touch(1)?;
        Err(marked(context, cm, Some(problem), Some(pm), None))
    }

    /// `self.resolver.processing_version` from the constructor.
    fn k_pv(&mut self) -> R<super::Ver> {
        self.frame(Self::resolver_prop)?;
        self.touch(1)?;
        Ok(self.processing_version())
    }

    /// `BaseConstructor.construct_document(node)` (the caller pushed its frame).
    pub(super) fn construct_document(&mut self, node: NodeId) -> R<()> {
        let saved = self.push_frame()?;
        self.run_machine(Act::Object {
            node,
            deep: false,
            saved,
            started: false,
            old_deep: false,
        })?;
        while !self.st.cons.generators.is_empty() {
            let sg = std::mem::take(&mut self.st.cons.generators);
            for g in sg {
                // `for _dummy in generator: pass` resumes the generator one frame deeper
                let saved = self.push_frame()?;
                self.run_machine(Act::body(g, saved))?;
            }
        }
        self.st.cons.constructed.clear();
        self.st.cons.recursive.clear();
        self.st.cons.deep = false;
        Ok(())
    }

    /// Runs `construct_object` / a generator body and everything it calls on an explicit stack; returns the
    /// value of the outermost activation (`None` for a generator body).
    fn run_machine(&mut self, start: Act) -> R<Option<ObjId>> {
        let mut stack = vec![start];
        let mut ret: Option<ObjId> = None;
        while let Some(top) = stack.last_mut() {
            match top {
                Act::Object {
                    node,
                    deep,
                    saved,
                    started,
                    old_deep,
                } => {
                    let (node, deep, saved) = (*node, *deep, *saved);
                    if *started {
                        // construct_non_recursive_object returned `ret`
                        let data = ret.unwrap_or(0);
                        self.st.cons.constructed.insert(node, data);
                        self.st.cons.recursive.remove(&node);
                        if deep {
                            self.st.cons.deep = *old_deep;
                        }
                        self.depth = saved;
                        stack.pop();
                        continue;
                    }
                    if let Some(&o) = self.st.cons.constructed.get(&node) {
                        ret = Some(o);
                        self.depth = saved;
                        stack.pop();
                        continue;
                    }
                    *old_deep = self.st.cons.deep;
                    if deep {
                        self.st.cons.deep = true;
                    }
                    if self.st.cons.recursive.contains(&node) {
                        ret = Some(self.new_obj(Obj::None));
                        self.depth = saved;
                        stack.pop();
                        continue;
                    }
                    self.st.cons.recursive.insert(node);
                    *started = true;
                    let cnro = self.push_frame()?;
                    stack.push(Act::Cnro {
                        node,
                        saved: cnro,
                        obj: None,
                    });
                }
                Act::Cnro { node, saved, obj } => {
                    let (node, saved) = (*node, *saved);
                    if let Some(o) = *obj {
                        // the deep generator body finished
                        ret = Some(o);
                        self.depth = saved;
                        stack.pop();
                        continue;
                    }
                    self.tag_access(node)?;
                    let tag = self.st.cons.node(node).tag.clone();
                    let gen_kind = match tag.as_str() {
                        OMAP => Some(GenKind::Omap),
                        PAIRS => Some(GenKind::Pairs),
                        SET => Some(GenKind::Set),
                        SEQ => Some(GenKind::Seq),
                        MAP => Some(GenKind::Map),
                        _ => None,
                    };
                    let Some(kind) = gen_kind else {
                        ret = Some(self.construct_leaf(node, &tag)?);
                        self.depth = saved;
                        stack.pop();
                        continue;
                    };
                    // next(generator): the prologue creates the empty container
                    self.touch(1)?;
                    let o = self.new_obj(match kind {
                        GenKind::Omap => Obj::ODict(Dict::default()),
                        GenKind::Pairs | GenKind::Seq => Obj::List(Vec::new()),
                        GenKind::Set => Obj::Set(Dict::default()),
                        GenKind::Map => Obj::Dict(Dict::default()),
                    });
                    let g = Gen { kind, node, obj: o };
                    if self.st.cons.deep {
                        *obj = Some(o);
                        let body = self.push_frame()?;
                        stack.push(Act::body(g, body));
                    } else {
                        self.st.cons.generators.push(g);
                        ret = Some(o);
                        self.depth = saved;
                        stack.pop();
                    }
                }
                Act::Body(b) => {
                    let step = self.body_step(b, ret.take())?;
                    match step {
                        Step::Call(node, deep) => {
                            let saved = self.push_frame()?;
                            stack.push(Act::Object {
                                node,
                                deep,
                                saved,
                                started: false,
                                old_deep: false,
                            });
                        }
                        Step::Done(saved) => {
                            ret = None;
                            self.depth = saved;
                            stack.pop();
                        }
                    }
                }
            }
        }
        Ok(ret)
    }

    /// One step of a generator body: either a `construct_object` call to make, or the end of the body.
    #[expect(
        clippy::many_single_char_names,
        reason = "the names of the Python and C code this ports"
    )]
    fn body_step(&mut self, b: &mut Body, ret: Option<ObjId>) -> R<Step> {
        match b {
            Body::Seq {
                g,
                saved,
                inner,
                idx,
                items,
            } => {
                let n = self.st.cons.node(g.node).clone();
                if inner.is_none() {
                    // data.extend(self.construct_sequence(node))
                    *inner = Some(self.push_frame()?);
                    if !matches!(n.kind, NodeKind::Seq(_)) {
                        let p = format!("expected a sequence node, but found {}", n.kind.id());
                        return self.cons_err(None, None, p.as_bytes(), &n.start);
                    }
                } else if let Some(r) = ret {
                    items.push(r);
                }
                let NodeKind::Seq(children) = &n.kind else {
                    return Ok(Step::Done(*saved));
                };
                if let Some(&c) = children.get(*idx) {
                    *idx = idx.saturating_add(1);
                    return Ok(Step::Call(c, false));
                }
                self.depth = inner.unwrap_or(*saved);
                let items = std::mem::take(items);
                if let Some(Obj::List(l)) = self.st.cons.objs.get_mut(g.obj) {
                    l.extend(items);
                }
                Ok(Step::Done(*saved))
            }
            Body::Map(m) => self.map_step(m, ret),
            Body::Omap {
                g,
                saved,
                idx,
                key,
                started,
            } => {
                let what = if g.kind == GenKind::Omap {
                    "while constructing an ordered map"
                } else {
                    "while constructing pairs"
                };
                let n = self.st.cons.node(g.node).clone();
                let NodeKind::Seq(subs) = &n.kind else {
                    let p = format!("expected a sequence, but found {}", n.kind.id());
                    return self.cons_err(Some(what), Some(&n.start), p.as_bytes(), &n.start);
                };
                if *started {
                    let r = ret.unwrap_or(0);
                    match key.take() {
                        None => {
                            // the key was constructed
                            if g.kind == GenKind::Omap {
                                let present = match self.objs().get(g.obj) {
                                    Obj::ODict(d) => {
                                        d.find(self.objs(), r).map_err(type_error)?.is_some()
                                    }
                                    _ => false,
                                };
                                if present {
                                    return Err(Fail::crash("AssertionError", ""));
                                }
                            }
                            *key = Some(r);
                            let value_node = match &self
                                .st
                                .cons
                                .node(subs.get(idx.saturating_sub(1)).copied().unwrap_or(0))
                                .kind
                            {
                                NodeKind::Map(p) => p.first().map_or(0, |e| e.1),
                                _ => 0,
                            };
                            return Ok(Step::Call(value_node, false));
                        }
                        Some(k) => {
                            if g.kind == GenKind::Omap {
                                let objs = &mut self.st.cons.objs;
                                let mut d = match objs.get_mut(g.obj) {
                                    Some(Obj::ODict(d)) => std::mem::take(d),
                                    _ => Dict::default(),
                                };
                                let res = d.set(objs, k, r);
                                if let Some(Obj::ODict(slot)) = objs.get_mut(g.obj) {
                                    *slot = d;
                                }
                                res.map_err(type_error)?;
                            } else {
                                let t = self.new_obj(Obj::Tuple(vec![k, r]));
                                if let Some(Obj::List(l)) = self.st.cons.objs.get_mut(g.obj) {
                                    l.push(t);
                                }
                            }
                        }
                    }
                }
                *started = true;
                let Some(&sub) = subs.get(*idx) else {
                    return Ok(Step::Done(*saved));
                };
                *idx = idx.saturating_add(1);
                let sn = self.st.cons.node(sub).clone();
                let NodeKind::Map(pairs) = &sn.kind else {
                    let p = format!("expected a mapping of length 1, but found {}", sn.kind.id());
                    return self.cons_err(Some(what), Some(&n.start), p.as_bytes(), &sn.start);
                };
                let [(key_node, _)] = pairs.as_slice() else {
                    let p = format!(
                        "expected a single mapping item, but found {} items",
                        pairs.len()
                    );
                    return self.cons_err(Some(what), Some(&n.start), p.as_bytes(), &sn.start);
                };
                Ok(Step::Call(*key_node, false))
            }
        }
    }

    /// The body of `construct_yaml_map` / `construct_yaml_set`: `SafeConstructor.construct_mapping` (flatten,
    /// then `BaseConstructor.construct_mapping`) and the update of the yielded object.
    fn map_step(&mut self, m: &mut MapBody, ret: Option<ObjId>) -> R<Step> {
        if m.phase == MapPhase::Start {
            m.outer = self.push_frame()?; // SafeConstructor.construct_mapping
            if matches!(self.st.cons.node(m.g.node).kind, NodeKind::Map(_)) {
                self.flatten_mapping(m.g.node)?;
            }
            m.base = self.push_frame()?; // BaseConstructor.construct_mapping
            let n = self.st.cons.node(m.g.node).clone();
            let NodeKind::Map(value) = &n.kind else {
                let p = format!("expected a mapping node, but found {}", n.kind.id());
                return self.cons_err(None, None, p.as_bytes(), &n.start);
            };
            m.todo = match &n.merge {
                Some(mg) => vec![(mg.clone(), false), (value.clone(), false)],
                None => vec![(value.clone(), true)],
            };
            m.phase = MapPhase::NextPair;
        }
        loop {
            match m.phase {
                MapPhase::Start => return Ok(Step::Done(m.saved)),
                MapPhase::NextPair => {
                    let Some((values, _)) = m.todo.get(m.todo_i) else {
                        // end of construct_mapping: data.update(value)
                        self.depth = m.outer;
                        let objs = &mut self.st.cons.objs;
                        let mut target = match objs.get_mut(m.g.obj) {
                            Some(Obj::Dict(d) | Obj::Set(d)) => std::mem::take(d),
                            _ => Dict::default(),
                        };
                        let mut res = Ok(());
                        for &(k, v) in &m.total.entries {
                            if let Err(e) = target.set(objs, k, v) {
                                res = Err(e);
                                break;
                            }
                        }
                        if let Some(Obj::Dict(d) | Obj::Set(d)) = objs.get_mut(m.g.obj) {
                            *d = target;
                        }
                        res.map_err(type_error)?;
                        return Ok(Step::Done(m.saved));
                    };
                    let Some(&(key_node, _)) = values.get(m.pair_i) else {
                        let mapping = std::mem::take(&mut m.mapping);
                        for (k, v) in mapping.entries {
                            m.total.set(self.objs(), k, v).map_err(type_error)?;
                        }
                        m.todo_i = m.todo_i.saturating_add(1);
                        m.pair_i = 0;
                        continue;
                    };
                    m.phase = MapPhase::GotKey;
                    return Ok(Step::Call(key_node, true));
                }
                MapPhase::GotKey => {
                    let (key_node, value_node) = m
                        .todo
                        .get(m.todo_i)
                        .and_then(|(v, _)| v.get(m.pair_i))
                        .copied()
                        .unwrap_or((0, 0));
                    let mut key = ret.unwrap_or(0);
                    let t = self.objs().get(key).py_type();
                    self.hashable_check(t)?;
                    if !self.objs().get(key).is_hashable_type()
                        && let Obj::List(items) = self.objs().get(key)
                    {
                        let items = items.clone();
                        key = self.new_obj(Obj::Tuple(items));
                    }
                    let t = self.objs().get(key).py_type();
                    self.hashable_check(t)?;
                    if !self.objs().get(key).is_hashable_type() {
                        let km = self.st.cons.node(key_node).start.clone();
                        let ms = self.st.cons.node(m.g.node).start.clone();
                        return self.cons_err(
                            Some("while constructing a mapping"),
                            Some(&ms),
                            b"found unhashable key",
                            &km,
                        );
                    }
                    m.key = key;
                    m.phase = MapPhase::GotValue;
                    return Ok(Step::Call(value_node, false));
                }
                MapPhase::GotValue => {
                    let (key_node, _) = m
                        .todo
                        .get(m.todo_i)
                        .and_then(|(v, _)| v.get(m.pair_i))
                        .copied()
                        .unwrap_or((0, 0));
                    let check = m.todo.get(m.todo_i).is_some_and(|t| t.1);
                    let (key, value) = (m.key, ret.unwrap_or(0));
                    if check {
                        let ms = self.st.cons.node(m.g.node).start.clone();
                        let mapping = std::mem::take(&mut m.mapping);
                        let unique = self
                            .frame(|s| s.check_mapping_key(&ms, key_node, &mapping, key, value));
                        m.mapping = mapping;
                        if unique? {
                            m.mapping.set(self.objs(), key, value).map_err(type_error)?;
                        }
                    } else {
                        m.mapping.set(self.objs(), key, value).map_err(type_error)?;
                    }
                    m.pair_i = m.pair_i.saturating_add(1);
                    m.phase = MapPhase::NextPair;
                }
            }
        }
    }

    /// The non-generator constructors (`construct_non_recursive_object` with its frame pushed).
    fn construct_leaf(&mut self, node: NodeId, tag: &str) -> R<ObjId> {
        let o = match tag {
            NULL => self.frame(|s| {
                s.frame(|s| s.construct_scalar(node))?;
                Ok(Obj::None)
            })?,
            BOOL => self.frame(|s| s.construct_yaml_bool(node))?,
            INT => self.frame(|s| s.construct_yaml_int(node))?,
            FLOAT => self.frame(|s| s.construct_yaml_float(node))?,
            BINARY => self.frame(|s| s.construct_yaml_binary(node))?,
            TIMESTAMP => self.frame(|s| s.construct_yaml_timestamp(node))?,
            t if t == STR => {
                self.frame(|s| Ok(Obj::Str(s.frame(|s| s.construct_scalar(node))?)))?
            }
            _ => {
                return self.frame(|s| {
                    s.tag_access(node)?; // node.tag in the message
                    let p = format!(
                        "could not determine a constructor for the tag {}",
                        snp_pycompat::repr::repr_str(tag)
                    );
                    let m = s.st.cons.node(node).start.clone();
                    s.cons_err(None, None, p.as_bytes(), &m)
                });
            }
        };
        Ok(self.new_obj(o))
    }

    /// `SafeConstructor.construct_scalar(node)` (the caller pushed its frame): the `=` key of a mapping, then
    /// `BaseConstructor.construct_scalar`.
    fn construct_scalar(&mut self, node: NodeId) -> R<Vec<u8>> {
        let saved = self.depth;
        let mut node = node;
        loop {
            let next = match &self.st.cons.node(node).kind {
                NodeKind::Map(pairs) => {
                    let pairs = pairs.clone();
                    let mut found = None;
                    for (k, v) in pairs {
                        self.tag_access(k)?;
                        if self.st.cons.node(k).tag == VALUE {
                            found = Some(v);
                            break;
                        }
                    }
                    found
                }
                _ => None,
            };
            match next {
                Some(v) => {
                    // return self.construct_scalar(value_node)
                    self.push_frame()?;
                    node = v;
                }
                None => break,
            }
        }
        let r = self.frame(|s| {
            let n = s.st.cons.node(node);
            match &n.kind {
                NodeKind::Scalar(v) => Ok(v.clone()),
                k => {
                    let p = format!("expected a scalar node, but found {}", k.id());
                    let m = n.start.clone();
                    s.cons_err(None, None, p.as_bytes(), &m)
                }
            }
        });
        self.depth = saved;
        r
    }

    fn construct_yaml_bool(&mut self, node: NodeId) -> R<Obj> {
        let value = self.frame(|s| s.construct_scalar(node))?;
        let lower = py_lower(&value);
        match lower.as_slice() {
            b"yes" | b"y" | b"true" | b"on" => Ok(Obj::Bool(true)),
            b"no" | b"n" | b"false" | b"off" => Ok(Obj::Bool(false)),
            _ => Err(Fail::crash("KeyError", repr_pystr(&lower))),
        }
    }

    fn construct_yaml_int(&mut self, node: NodeId) -> R<Obj> {
        let value_s = self.frame(|s| s.construct_scalar(node))?;
        let mut value_s: Vec<u8> = value_s.into_iter().filter(|&b| b != b'_').collect();
        let index_error = || Fail::crash("IndexError", "string index out of range");
        let first = *value_s.first().ok_or_else(index_error)?;
        let neg = first == b'-';
        if first == b'-' || first == b'+' {
            value_s = value_s.get(1..).unwrap_or_default().to_vec();
        }
        let apply = |v: BigInt| if neg { v.negated() } else { v };
        let int_err = |e: String| Fail::crash("ValueError", e);
        if value_s == b"0" {
            return Ok(Obj::Int(BigInt::default()));
        }
        for (prefix, base) in [(&b"0b"[..], 2u32), (b"0x", 16), (b"0o", 8)] {
            if let Some(rest) = value_s.strip_prefix(prefix) {
                return Ok(Obj::Int(apply(py_int(rest, base).map_err(int_err)?)));
            }
        }
        if self.k_pv()?.is11() && *value_s.first().ok_or_else(index_error)? == b'0' {
            return Ok(Obj::Int(apply(py_int(&value_s, 8).map_err(int_err)?)));
        }
        if self.k_pv()?.is11() && value_s.contains(&b':') {
            let mut digits = Vec::new();
            for part in value_s.split(|&b| b == b':') {
                digits.push(py_int(part, 10).map_err(int_err)?);
            }
            digits.reverse();
            let mut base = BigInt::from_u64(1);
            let mut value = BigInt::default();
            let sixty = BigInt::from_u64(60);
            for d in digits {
                value = value.add(&d.mul(&base));
                base = base.mul(&sixty);
            }
            return Ok(Obj::Int(apply(value)));
        }
        Ok(Obj::Int(apply(py_int(&value_s, 10).map_err(int_err)?)))
    }

    fn construct_yaml_float(&mut self, node: NodeId) -> R<Obj> {
        let value_so = self.frame(|s| s.construct_scalar(node))?;
        let replaced: Vec<u8> = value_so.iter().copied().filter(|&b| b != b'_').collect();
        let mut value_s = py_lower(&replaced);
        let first = *value_s
            .first()
            .ok_or_else(|| Fail::crash("IndexError", "string index out of range"))?;
        let sign = if first == b'-' { -1.0f64 } else { 1.0 };
        if first == b'-' || first == b'+' {
            value_s = value_s.get(1..).unwrap_or_default().to_vec();
        }
        if value_s == b".inf" {
            return Ok(Obj::Float(sign * f64::INFINITY, false));
        }
        if value_s == b".nan" {
            return Ok(Obj::Float(f64::NAN, true));
        }
        let float_err = |e: String| Fail::crash("ValueError", e);
        if !self.k_pv()?.is("1", "2") && value_s.contains(&b':') {
            let mut digits = Vec::new();
            for part in value_s.split(|&b| b == b':') {
                digits.push(py_float(part).map_err(float_err)?);
            }
            digits.reverse();
            let mut base = BigInt::from_u64(1);
            let sixty = BigInt::from_u64(60);
            let mut value = 0.0f64;
            for d in digits {
                let b: f64 = base
                    .to_decimal()
                    .ok()
                    .and_then(|t| t.parse::<f64>().ok())
                    .filter(|f| f.is_finite())
                    .ok_or_else(|| {
                        Fail::crash("OverflowError", "int too large to convert to float")
                    })?;
                value = d.mul_add(b, value);
                base = base.mul(&sixty);
            }
            return Ok(Obj::Float(sign * value, false));
        }
        if !self.k_pv()?.is("1", "2") && value_s.contains(&b'e') {
            let parts = value_s.split(|&b| b == b'e').count();
            if parts > 2 {
                return Err(Fail::crash(
                    "ValueError",
                    "too many values to unpack (expected 2)",
                ));
            }
            let mantissa = value_s.split(|&b| b == b'e').next().unwrap_or_default();
            if !mantissa.contains(&b'.') {
                // MantissaNoDotYAML1_1Warning(node, value_so) and its display
                self.touch(1)?;
                self.touch(if self.st.warned { 6 } else { 23 })?;
                self.st.warned = true;
            }
        }
        Ok(Obj::Float(
            sign * py_float(&value_s).map_err(float_err)?,
            false,
        ))
    }

    fn construct_yaml_binary(&mut self, node: NodeId) -> R<Obj> {
        let value = self.frame(|s| s.construct_scalar(node))?;
        let start = self.st.cons.node(node).start.clone();
        if let Some(e) = ascii_encode_error(&value) {
            let p = format!("failed to convert base64 data into ascii: {e}");
            return self.cons_err(None, None, p.as_bytes(), &start);
        }
        self.touch(2)?; // base64.decodebytes -> _input_type_check
        match a2b_base64(&value, self.st.minor) {
            Ok(b) => Ok(Obj::Bytes(b)),
            Err(e) => {
                let p = format!("failed to decode base64 data: {e}");
                self.cons_err(None, None, p.as_bytes(), &start)
            }
        }
    }

    fn construct_yaml_timestamp(&mut self, node: NodeId) -> R<Obj> {
        self.rx_call(Rx::TimestampCons)?;
        let n = self.st.cons.node(node).clone();
        let groups = match &n.kind {
            NodeKind::Scalar(v) => {
                timestamp_groups(v).or_else(|| v.strip_suffix(b"\n").and_then(timestamp_groups))
            }
            _ => None,
        };
        let Some(g) = groups else {
            let shown = match &n.kind {
                NodeKind::Scalar(v) => v.clone(),
                _ => self.node_value_repr(node)?,
            };
            let mut p = b"failed to construct timestamp from \"".to_vec();
            p.extend_from_slice(&shown);
            p.push(b'"');
            return self.cons_err(None, None, &p, &n.start);
        };
        self.frame(|_| create_timestamp(&g))
    }

    /// `repr(node.value)` of a collection node (`Node.__repr__` for every node inside).
    fn node_value_repr(&mut self, node: NodeId) -> R<Vec<u8>> {
        enum W {
            Node(NodeId),
            Text(&'static str),
            Value(NodeId),
        }
        let mut out = Vec::new();
        let mut active: Vec<NodeId> = vec![node];
        let mut stack = vec![W::Value(node)];
        let base = self.depth;
        while let Some(w) = stack.pop() {
            match w {
                W::Text(t) => {
                    if t == "\u{0}pop" {
                        active.pop();
                        self.depth = self.depth.saturating_sub(1);
                    } else {
                        out.extend_from_slice(t.as_bytes());
                    }
                }
                W::Node(id) => {
                    // Node.__repr__ (one frame) with the tag property
                    self.push_frame()?;
                    self.tag_access(id)?;
                    let n = self.st.cons.node(id);
                    let cls = match n.kind {
                        NodeKind::Scalar(_) => "ScalarNode",
                        NodeKind::Seq(_) => "SequenceNode",
                        NodeKind::Map(_) => "MappingNode",
                    };
                    out.extend_from_slice(
                        format!("{cls}(tag={}, ", snp_pycompat::repr::repr_str(&n.tag)).as_bytes(),
                    );
                    if let Some(a) = &n.anchor {
                        out.extend_from_slice(
                            format!("anchor={}, ", snp_pycompat::repr::repr_str(a)).as_bytes(),
                        );
                    }
                    out.extend_from_slice(b"value=");
                    if let NodeKind::Scalar(v) = &n.kind {
                        out.extend_from_slice(repr_pystr(v).as_bytes());
                        out.push(b')');
                        self.depth = self.depth.saturating_sub(1);
                    } else {
                        stack.push(W::Text("\u{0}pop"));
                        stack.push(W::Text(")"));
                        stack.push(W::Value(id));
                        active.push(id);
                    }
                }
                W::Value(id) => {
                    if active.iter().filter(|&&a| a == id).count() > 1 {
                        out.extend_from_slice(b"[...]");
                        continue;
                    }
                    let n = self.st.cons.node(id).clone();
                    match n.kind {
                        NodeKind::Seq(items) => {
                            stack.push(W::Text("]"));
                            for (k, it) in items.iter().enumerate().rev() {
                                stack.push(W::Node(*it));
                                if k > 0 {
                                    stack.push(W::Text(", "));
                                }
                            }
                            stack.push(W::Text("["));
                        }
                        NodeKind::Map(pairs) => {
                            stack.push(W::Text("]"));
                            for (k, (a, b)) in pairs.iter().enumerate().rev() {
                                stack.push(W::Text(")"));
                                stack.push(W::Node(*b));
                                stack.push(W::Text(", "));
                                stack.push(W::Node(*a));
                                stack.push(W::Text("("));
                                if k > 0 {
                                    stack.push(W::Text(", "));
                                }
                            }
                            stack.push(W::Text("["));
                        }
                        NodeKind::Scalar(v) => out.extend_from_slice(repr_pystr(&v).as_bytes()),
                    }
                }
            }
        }
        self.depth = base;
        Ok(out)
    }

    /// `check_mapping_key` (the caller pushed its frame): `true` when the key is new.
    fn check_mapping_key(
        &mut self,
        map_start: &Mark,
        key_node: NodeId,
        mapping: &Dict,
        key: ObjId,
        value: ObjId,
    ) -> R<bool> {
        let found = mapping.find(self.objs(), key).map_err(type_error)?;
        let Some(i) = found else {
            return Ok(true);
        };
        let mk = mapping.entries.get(i).map_or(key, |e| e.1);
        let text_err = |e: String| Fail::crash("ValueError", e);
        let ks = self.objs().str_of(key).map_err(text_err)?;
        let vs = self.objs().str_of(value).map_err(text_err)?;
        let ms = self.objs().str_of(mk).map_err(text_err)?;
        let mut p = b"found duplicate key \"".to_vec();
        p.extend_from_slice(&ks);
        p.extend_from_slice(b"\" with value \"");
        p.extend_from_slice(&vs);
        p.extend_from_slice(b"\" (original value: \"");
        p.extend_from_slice(&ms);
        p.extend_from_slice(b"\")");
        self.touch(1)?;
        let km = self.st.cons.node(key_node).start.clone();
        Err(marked(
            Some("while constructing a mapping"),
            Some(map_start),
            Some(&p),
            Some(&km),
            Some(DUPKEY_NOTE),
        ))
    }

    /// `SafeConstructor.flatten_mapping(node)` and its recursion on merged mappings, on an explicit stack.
    fn flatten_mapping(&mut self, root: NodeId) -> R<()> {
        enum Pending {
            None,
            Map(NodeId),
            Seq {
                value_node: NodeId,
                next: usize,
                waiting: Option<NodeId>,
                sub: Vec<Vec<(NodeId, NodeId)>>,
            },
        }
        struct F {
            node: NodeId,
            index: usize,
            merge: Vec<(NodeId, NodeId)>,
            saved: u32,
            pending: Pending,
        }
        let call = |s: &mut Self, stack: &mut Vec<F>, node: NodeId| -> R<()> {
            let saved = s.push_frame()?;
            stack.push(F {
                node,
                index: 0,
                merge: Vec::new(),
                saved,
                pending: Pending::None,
            });
            Ok(())
        };
        let mut stack: Vec<F> = Vec::new();
        call(self, &mut stack, root)?;
        while let Some(f) = stack.last_mut() {
            match &mut f.pending {
                Pending::Map(v) => {
                    let v = *v;
                    if let NodeKind::Map(vals) = &self.st.cons.node(v).kind {
                        f.merge.extend(vals.iter().copied());
                    }
                    f.pending = Pending::None;
                    continue;
                }
                Pending::Seq {
                    value_node,
                    next,
                    waiting,
                    sub,
                } => {
                    if let Some(w) = waiting.take()
                        && let NodeKind::Map(vals) = &self.st.cons.node(w).kind
                    {
                        sub.push(vals.clone());
                    }
                    let subs = match &self.st.cons.node(*value_node).kind {
                        NodeKind::Seq(items) => items.clone(),
                        _ => Vec::new(),
                    };
                    if let Some(&subnode) = subs.get(*next) {
                        *next = next.saturating_add(1);
                        let sn = self.st.cons.node(subnode).clone();
                        if !matches!(sn.kind, NodeKind::Map(_)) {
                            let p = format!(
                                "expected a mapping for merging, but found {}",
                                sn.kind.id()
                            );
                            let ms = self.st.cons.node(f.node).start.clone();
                            return self.cons_err(
                                Some("while constructing a mapping"),
                                Some(&ms),
                                p.as_bytes(),
                                &sn.start,
                            );
                        }
                        *waiting = Some(subnode);
                        call(self, &mut stack, subnode)?;
                        continue;
                    }
                    let mut sub = std::mem::take(sub);
                    sub.reverse();
                    for v in sub {
                        f.merge.extend(v);
                    }
                    f.pending = Pending::None;
                    continue;
                }
                Pending::None => {}
            }
            let node = f.node;
            let pairs = match &self.st.cons.node(node).kind {
                NodeKind::Map(p) => p.clone(),
                _ => Vec::new(),
            };
            let Some(&(key_node, value_node)) = pairs.get(f.index) else {
                // end of the while loop
                if !f.merge.is_empty() {
                    let merge = std::mem::take(&mut f.merge);
                    let n = self.st.cons.node_mut(node);
                    if let NodeKind::Map(vals) = &mut n.kind {
                        let mut all = merge.clone();
                        all.extend(vals.iter().copied());
                        *vals = all;
                    }
                    n.merge = Some(merge);
                }
                self.depth = f.saved;
                stack.pop();
                continue;
            };
            self.tag_access(key_node)?;
            let key_tag = self.st.cons.node(key_node).tag.clone();
            if key_tag == MERGE {
                if !f.merge.is_empty() {
                    let ms = self.st.cons.node(node).start.clone();
                    let km = self.st.cons.node(key_node).start.clone();
                    self.touch(1)?;
                    return Err(marked(
                        Some("while constructing a mapping"),
                        Some(&ms),
                        Some(b"found duplicate merge key \"<<\""),
                        Some(&km),
                        Some(MERGE_NOTE),
                    ));
                }
                let index = f.index;
                if let NodeKind::Map(vals) = &mut self.st.cons.node_mut(node).kind {
                    // del node.value[index]
                    *vals = vals
                        .iter()
                        .enumerate()
                        .filter(|&(i, _)| i != index)
                        .map(|(_, &p)| p)
                        .collect();
                }
                let vn = self.st.cons.node(value_node).clone();
                let Some(f) = stack.last_mut() else {
                    break;
                };
                match vn.kind {
                    NodeKind::Map(_) => {
                        f.pending = Pending::Map(value_node);
                        call(self, &mut stack, value_node)?;
                    }
                    NodeKind::Seq(_) => {
                        f.pending = Pending::Seq {
                            value_node,
                            next: 0,
                            waiting: None,
                            sub: Vec::new(),
                        };
                    }
                    NodeKind::Scalar(_) => {
                        let p = format!(
                            "expected a mapping or list of mappings for merging, but found {}",
                            vn.kind.id()
                        );
                        let ms = self.st.cons.node(node).start.clone();
                        return self.cons_err(
                            Some("while constructing a mapping"),
                            Some(&ms),
                            p.as_bytes(),
                            &vn.start,
                        );
                    }
                }
            } else {
                self.tag_access(key_node)?;
                if key_tag == VALUE {
                    self.touch(2)?; // the tag setter -> Tag.__init__
                    let kn = self.st.cons.node_mut(key_node);
                    STR.clone_into(&mut kn.tag);
                    kn.tag_cache = TagCache::Fresh;
                }
                if let Some(f) = stack.last_mut() {
                    f.index = f.index.saturating_add(1);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_versions() {
        assert_eq!(a2b_base64(b"aGk=\naGk=", PyMinor::P312), Ok(b"hi".to_vec()));
        assert_eq!(
            a2b_base64(b"aGk=\naGk=", PyMinor::P313),
            Err("Incorrect padding".to_owned())
        );
        assert_eq!(
            a2b_base64(b"aG==aG==", PyMinor::P313),
            Ok(b"hf\x86".to_vec())
        );
        assert_eq!(a2b_base64(b"aGk=x", PyMinor::P313), Ok(b"hi1".to_vec()));
        assert_eq!(a2b_base64(b"!!!", PyMinor::P312), Ok(Vec::new()));
        assert!(
            a2b_base64(b"a===", PyMinor::P312)
                .unwrap_err()
                .contains("(1) cannot be 1 more")
        );
    }

    #[test]
    fn ascii_errors() {
        assert_eq!(
            ascii_encode_error("a\u{e9}\u{20ac}x".as_bytes()).as_deref(),
            Some(
                "'ascii' codec can't encode characters in position 1-2: ordinal not in range(128)"
            )
        );
        assert_eq!(
            ascii_encode_error("\u{e9}".as_bytes()).as_deref(),
            Some(
                "'ascii' codec can't encode character '\\xe9' in position 0: ordinal not in range(128)"
            )
        );
    }
}
