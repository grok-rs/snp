//! PyYAML `CSafeDumper` emitter (`yaml.dump` byte parity, 07 §3.1, 03 §11.3).
//!
//! `pre_commit/yaml.py:14-19` dumps with `yaml.dump(o, Dumper=CSafeDumper, default_flow_style=False, indent=4,
//! sort_keys=False)`. That is PyYAML's `SafeRepresenter` (Python) building nodes, `CEmitter.serialize` turning them
//! into libyaml events with `plain_implicit`/`quoted_implicit` computed by the YAML 1.1 resolver and `idNNN`
//! anchors for nodes that occur more than once, and libyaml 0.2.5's emitter writing them. This module is the
//! representer and serializer; the emitter is a port of libyaml's (`emitter.rs`).
//!
//! Owner: WP-14.

use cfgv::{MaxStrDigitsError, Value};

use crate::base64::encodebytes;
use crate::compose::NodeId;
use crate::construct::{Built, Graph, construct_graph};
use crate::emitter::{Emitter, Event, Style};
use crate::resolve::{
    BINARY_TAG, BOOL_TAG, FLOAT_TAG, INT_TAG, MAP_TAG, NULL_TAG, SEQ_TAG, SET_TAG, STR_TAG,
    TIMESTAMP_TAG,
};
use crate::{Limits, Schema, YamlError, compose, resolve_plain};

/// `yaml.dump(o, Dumper=CSafeDumper, default_flow_style=False, indent=4, sort_keys=False)` — byte exact.
///
/// `SafeRepresenter`: `None` → `null`, `bool` → `true`/`false`, `int` → `str(n)`, `float` → `repr(x).lower()` with
/// `.0` inserted before a bare exponent (`1e+16` → `1.0e+16`) and `.nan`/`.inf`/`-.inf`, `str` → itself, `bytes` →
/// `!!binary |` with `base64.encodebytes` text, `date`/`datetime` → `isoformat()` / `isoformat(' ')`, `list` and
/// `tuple` → a block sequence, `dict` → a block mapping in insertion order, `set` → `!!set` mapping to `null`. A
/// scalar is written plain unless re-reading it would give another type or libyaml's analysis forbids it (then
/// single, else double quotes, non-ASCII escaped as `\xE9`/`\u2028`/`\U0001F600`); long plain and quoted scalars
/// fold at spaces past column 80; sequences inside mappings are not indented; mapping items in a sequence start
/// with `-   ` (indent 4); an empty list or dict is `[]`/`{}`; the output ends with `\n`.
///
/// A `cfgv::Value` is a tree, so no object occurs twice and no `&id001` anchors are written: to re-dump a loaded
/// document with PyYAML's anchors for shared objects use [`redump`]. An integer with more than 4300 digits (Python
/// raises `ValueError`) is written in hexadecimal.
pub fn dump(v: &Value) -> String {
    let mut em = Emitter::new();
    represent_value(v, None, &mut em);
    em.finish()
}

/// `yaml_dump(yaml_load(text))` — autoupdate's whole-file reformat (`autoupdate.py:134-137`, 03 §11.3) — byte
/// exact, including the `&id001` / `*id001` anchors PyYAML writes for objects that occur more than once (a
/// collection or a date reached through an alias or a merge key). Load errors are those of [`crate::load`]; an
/// integer with more than 4300 decimal digits is PyYAML's `ValueError` from `str(int)` (unlike [`dump`], which
/// cannot fail and writes it in hexadecimal).
pub fn redump(text: &str, lim: &Limits) -> Result<String, YamlError> {
    let doc = compose(text, Schema::PyYaml11Safe, lim)?;
    let graph = construct_graph(&doc, Schema::PyYaml11Safe, lim)?;
    let mut em = Emitter::new();
    match graph.root {
        None => represent_value(&Value::Null, None, &mut em),
        Some(root) => represent_graph(&graph, root, &mut em)?,
    }
    Ok(em.finish())
}

/// `default_style` of the autoupdate rev re-dump.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Quote {
    /// `None` (plain when possible).
    Plain,
    /// `"'"`.
    Single,
    /// `'"'`.
    Double,
}

/// autoupdate.py:146-149: `yaml_dump({'rev': rev}, default_style=q).split(':', 1)[1].strip()`.
///
/// With [`Quote::Single`] or [`Quote::Double`] every scalar of the dump is requested in that style (libyaml still
/// switches from single to double quotes when single quotes cannot hold the value); [`Quote::Plain`] is the
/// representer's default. The result is the text after the first `:` of the dump, stripped of Python whitespace —
/// for a long rev with spaces it can span several lines, exactly as pre-commit writes it.
pub fn scalar_for_rev(rev: &str, q: Quote) -> String {
    let style = match q {
        Quote::Plain => None,
        Quote::Single => Some(Style::Single),
        Quote::Double => Some(Style::Double),
    };
    let mut em = Emitter::new();
    em.emit(&Event::MappingStart {
        anchor: None,
        tag: MAP_TAG,
        implicit: true,
    });
    em.emit(&scalar_event(None, STR_TAG, "rev".to_owned(), style));
    em.emit(&scalar_event(None, STR_TAG, rev.to_owned(), style));
    em.emit(&Event::MappingEnd);
    let out = em.finish();
    let after = out.split_once(':').map_or("", |(_, rest)| rest);
    snp_pycompat::text::strip(after).to_owned()
}

/// A scalar node of `SafeRepresenter.represent_scalar(tag, value, style)` serialized by `CEmitter`:
/// `plain_implicit` iff the YAML 1.1 resolver gives the node's tag back for the plain text, `quoted_implicit` iff
/// the tag is `str`; `style` `None` means plain.
fn scalar_event(
    anchor: Option<String>,
    tag: &'static str,
    value: String,
    style: Option<Style>,
) -> Event {
    let plain_implicit = resolve_plain(&value, Schema::PyYaml11Safe).as_tag() == tag;
    Event::Scalar {
        anchor,
        tag,
        plain_implicit,
        quoted_implicit: tag == STR_TAG,
        style: style.unwrap_or(Style::Plain),
        value,
    }
}

/// `SafeRepresenter.represent_float`.
fn float_text(x: f64) -> String {
    if x.is_nan() {
        ".nan".to_owned()
    } else if x == f64::INFINITY {
        ".inf".to_owned()
    } else if x == f64::NEG_INFINITY {
        "-.inf".to_owned()
    } else {
        let r = snp_pycompat::float::repr(x).to_lowercase();
        if !r.contains('.') && r.contains('e') {
            r.replacen('e', ".0e", 1)
        } else {
            r
        }
    }
}

/// The scalar node of a scalar value (`SafeRepresenter.represent_*`), or `None` for a collection.
fn scalar_of(v: &Value, anchor: Option<&str>, default_style: Option<Style>) -> Option<Event> {
    let anchor = anchor.map(str::to_owned);
    let scalar = |tag, text| Some(scalar_event(anchor.clone(), tag, text, default_style));
    match v {
        Value::Null => scalar(NULL_TAG, "null".to_owned()),
        Value::Bool(b) => scalar(BOOL_TAG, if *b { "true" } else { "false" }.to_owned()),
        Value::Int(i) => scalar(INT_TAG, i.to_string()),
        Value::BigInt(b) => scalar(INT_TAG, b.to_string()),
        Value::Float(x) => scalar(FLOAT_TAG, float_text(*x)),
        Value::String(s) => scalar(STR_TAG, s.clone()),
        Value::Bytes(b) => Some(scalar_event(
            anchor.clone(),
            BINARY_TAG,
            encodebytes(b),
            Some(Style::Literal),
        )),
        Value::Date(d) => scalar(TIMESTAMP_TAG, d.to_string()),
        Value::DateTime(d) => scalar(TIMESTAMP_TAG, d.to_string()),
        Value::List(_) | Value::Tuple(_) | Value::Map(_) | Value::Set(_) => None,
        // A future value kind: its Python `str()` as a string.
        other => scalar(STR_TAG, other.to_string()),
    }
}

/// The start event of a collection (`empty` collections are one event).
fn collection_start(
    tag: &'static str,
    implicit: bool,
    mapping: bool,
    empty: bool,
    anchor: Option<String>,
) -> Event {
    match (mapping, empty) {
        (false, false) => Event::SequenceStart {
            anchor,
            tag,
            implicit,
        },
        (false, true) => Event::EmptySequence {
            anchor,
            tag,
            implicit,
        },
        (true, false) => Event::MappingStart {
            anchor,
            tag,
            implicit,
        },
        (true, true) => Event::EmptyMapping {
            anchor,
            tag,
            implicit,
        },
    }
}

/// One step of the iterative tree representer.
enum Work<'v> {
    Node(&'v Value),
    /// A `!!set` member: the key, then `null`.
    SetMember(&'v Value),
    End(Event),
}

/// `represent_data` + `serialize` for a tree value, feeding `em` (an explicit stack: deep values cost heap, not
/// stack).
fn represent_value(v: &Value, default_style: Option<Style>, em: &mut Emitter) {
    let mut work = vec![Work::Node(v)];
    while let Some(w) = work.pop() {
        let v = match w {
            Work::End(ev) => {
                em.emit(&ev);
                continue;
            }
            Work::SetMember(k) => {
                work.push(Work::Node(&Value::Null));
                work.push(Work::Node(k));
                continue;
            }
            Work::Node(v) => v,
        };
        if let Some(ev) = scalar_of(v, None, default_style) {
            em.emit(&ev);
            continue;
        }
        match v {
            Value::List(items) | Value::Tuple(items) => {
                em.emit(&collection_start(
                    SEQ_TAG,
                    true,
                    false,
                    items.is_empty(),
                    None,
                ));
                if !items.is_empty() {
                    work.push(Work::End(Event::SequenceEnd));
                    work.extend(items.iter().rev().map(Work::Node));
                }
            }
            Value::Map(m) => {
                em.emit(&collection_start(MAP_TAG, true, true, m.is_empty(), None));
                if !m.is_empty() {
                    work.push(Work::End(Event::MappingEnd));
                    for (k, v) in m.iter().rev() {
                        work.push(Work::Node(v));
                        work.push(Work::Node(k));
                    }
                }
            }
            Value::Set(s) => {
                em.emit(&collection_start(SET_TAG, false, true, s.is_empty(), None));
                if !s.is_empty() {
                    work.push(Work::End(Event::MappingEnd));
                    work.extend(s.iter().rev().map(Work::SetMember));
                }
            }
            _ => {}
        }
    }
}

/// A node of PyYAML's representation graph: an object of the constructed graph, or a node the representer
/// creates afresh at each use (a `!!pairs` tuple, the `null` of a `!!set` member).
#[derive(Copy, Clone, Debug)]
enum Rep {
    Object(NodeId),
    Tuple(NodeId, NodeId),
    Null,
}

/// The constructed graph as PyYAML's representer sees it.
#[derive(Debug, Clone, Copy)]
struct View<'g> {
    graph: &'g Graph,
}

impl<'g> View<'g> {
    fn get(self, id: NodeId) -> Option<&'g Built> {
        self.graph.built.get(id.index()).and_then(Option::as_ref)
    }

    /// Whether PyYAML memoises the object's node (`SafeRepresenter.ignore_aliases` is false): collections and
    /// dates; `None`, `str`, `bytes`, `bool`, `int` and `float` get a fresh node at every use.
    fn anchorable(self, id: NodeId) -> bool {
        match self.get(id) {
            Some(Built::Scalar(Value::Date(_) | Value::DateTime(_))) => true,
            Some(Built::Scalar(_)) | None => false,
            Some(_) => true,
        }
    }

    /// The children of a representation node, in serialization order.
    fn rep_children(self, r: Rep) -> Vec<Rep> {
        match r {
            Rep::Null => Vec::new(),
            Rep::Tuple(k, v) => vec![Rep::Object(k), Rep::Object(v)],
            Rep::Object(id) => match self.get(id) {
                Some(Built::Seq(items)) => items.iter().map(|c| Rep::Object(*c)).collect(),
                Some(Built::Map(pairs)) => pairs
                    .iter()
                    .flat_map(|(k, v)| [Rep::Object(*k), Rep::Object(*v)])
                    .collect(),
                Some(Built::Set(keys)) => keys
                    .iter()
                    .flat_map(|k| [Rep::Object(*k), Rep::Null])
                    .collect(),
                Some(Built::Pairs(pairs)) => {
                    pairs.iter().map(|(k, v)| Rep::Tuple(*k, *v)).collect()
                }
                _ => Vec::new(),
            },
        }
    }

    /// `CEmitter._anchor_node` over the representation graph: an anchorable object reached a second time gets
    /// the next `idNNN` name, in pre-order.
    fn anchors(self, root: NodeId) -> Vec<Option<String>> {
        let n = self.graph.built.len();
        let mut seen = vec![false; n];
        let mut names: Vec<Option<String>> = vec![None; n];
        let mut last: u32 = 0;
        let mut stack = vec![Rep::Object(root)];
        while let Some(r) = stack.pop() {
            if let Rep::Object(id) = r
                && self.anchorable(id)
            {
                let i = id.index();
                if seen.get(i).copied().unwrap_or(true) {
                    if let Some(slot @ None) = names.get_mut(i) {
                        last = last.saturating_add(1);
                        *slot = Some(format!("id{last:03}"));
                    }
                    continue;
                }
                if let Some(s) = seen.get_mut(i) {
                    *s = true;
                }
            }
            stack.extend(self.rep_children(r).into_iter().rev());
        }
        names
    }
}

/// One step of the iterative graph serializer.
enum GraphWork {
    Node(Rep),
    End(Event),
}

/// `represent_data` + `CEmitter.serialize` over the constructed graph: the first occurrence of an anchored object
/// carries `&idNNN`, later ones are `*idNNN` aliases.
///
/// # Errors
///
/// `ValueError` (CPython's `str(int)` text) for an integer with more than 4300 decimal digits: PyYAML's
/// representer calls `str(data)`, so the whole re-dump fails before anything is written.
fn represent_graph(graph: &Graph, root: NodeId, em: &mut Emitter) -> Result<(), YamlError> {
    let g = View { graph };
    let names = g.anchors(root);
    let mut serialized = vec![false; graph.built.len()];
    let mut work = vec![GraphWork::Node(Rep::Object(root))];
    while let Some(w) = work.pop() {
        let r = match w {
            GraphWork::End(ev) => {
                em.emit(&ev);
                continue;
            }
            GraphWork::Node(r) => r,
        };
        let id = match r {
            Rep::Null => {
                em.emit(&scalar_event(None, NULL_TAG, "null".to_owned(), None));
                continue;
            }
            Rep::Tuple(..) => {
                em.emit(&collection_start(SEQ_TAG, true, false, false, None));
                work.push(GraphWork::End(Event::SequenceEnd));
                work.extend(g.rep_children(r).into_iter().rev().map(GraphWork::Node));
                continue;
            }
            Rep::Object(id) => id,
        };
        let anchor = names.get(id.index()).cloned().flatten();
        if g.anchorable(id) {
            let i = id.index();
            if serialized.get(i).copied().unwrap_or(false) {
                if let Some(name) = anchor {
                    em.emit(&Event::Alias(name));
                }
                continue;
            }
            if let Some(s) = serialized.get_mut(i) {
                *s = true;
            }
        }
        let start = match g.get(id) {
            Some(Built::Scalar(v)) => {
                if let Value::BigInt(b) = v
                    && b.exceeds_max_str_digits()
                {
                    return Err(YamlError::python(
                        "ValueError",
                        MaxStrDigitsError.to_string(),
                    ));
                }
                if let Some(ev) = scalar_of(v, anchor.as_deref(), None) {
                    em.emit(&ev);
                }
                continue;
            }
            Some(Built::Seq(items)) => {
                collection_start(SEQ_TAG, true, false, items.is_empty(), anchor)
            }
            Some(Built::Pairs(pairs)) => {
                collection_start(SEQ_TAG, true, false, pairs.is_empty(), anchor)
            }
            Some(Built::Map(pairs)) => {
                collection_start(MAP_TAG, true, true, pairs.is_empty(), anchor)
            }
            Some(Built::Set(keys)) => {
                collection_start(SET_TAG, false, true, keys.is_empty(), anchor)
            }
            // A pending generator cannot remain after construction.
            Some(Built::Pending(_)) | None => continue,
        };
        let end = match start {
            Event::SequenceStart { .. } => Event::SequenceEnd,
            Event::MappingStart { .. } => Event::MappingEnd,
            _ => {
                em.emit(&start);
                continue;
            }
        };
        em.emit(&start);
        work.push(GraphWork::End(end));
        work.extend(g.rep_children(r).into_iter().rev().map(GraphWork::Node));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats() {
        assert_eq!(float_text(1e16), "1.0e+16");
        assert_eq!(float_text(1e-5), "1.0e-05");
        assert_eq!(float_text(1.5), "1.5");
        assert_eq!(float_text(-0.0), "-0.0");
        assert_eq!(float_text(f64::NAN), ".nan");
        assert_eq!(float_text(f64::NEG_INFINITY), "-.inf");
    }

    #[test]
    fn revs() {
        assert_eq!(scalar_for_rev("v1.2.3", Quote::Plain), "v1.2.3");
        assert_eq!(scalar_for_rev("1.10", Quote::Plain), "'1.10'");
        assert_eq!(scalar_for_rev("v1", Quote::Double), "\"v1\"");
        assert_eq!(scalar_for_rev("'q", Quote::Single), "'''q'");
        assert_eq!(scalar_for_rev("\"dq", Quote::Double), "\"\\\"dq\"");
        assert_eq!(scalar_for_rev("é", Quote::Plain), "\"\\xE9\"");
        assert_eq!(scalar_for_rev("", Quote::Plain), "''");
    }

    #[test]
    fn redump_anchors_shared_objects() {
        let lim = Limits::default();
        let text = "x: &x {repo: local, hooks: []}\nrepos: [*x, *x]\n";
        assert_eq!(
            redump(text, &lim).expect("redump"),
            "x: &id001\n    repo: local\n    hooks: []\nrepos:\n- *id001\n- *id001\n"
        );
    }
}
