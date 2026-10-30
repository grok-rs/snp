//! `ruamel/yaml/composer.py` (`Composer`): events to the node graph, anchors and aliases (aliases are never
//! expanded: they share the node). Python's `compose_node` recursion runs here on an explicit stack with the
//! same frame accounting (`compose_node` + `compose_*_node` per level).
//!
//! Owner: WP-17.

use snp_pycompat::repr::repr_str;

use super::construct::{Node, NodeId, NodeKind, TagCache};
use super::parser::{EvId, Event};
use super::{Fail, Loader, R, marked};

/// A collection under construction on the explicit stack.
enum Work {
    Seq(NodeId),
    /// A mapping and the key composed for the value being composed now.
    Map(NodeId, Option<NodeId>),
}

/// The step the composer takes next.
enum Step {
    /// Call `compose_node` for the next child (or the root).
    Enter,
    /// Back in `compose_*_node`'s loop: `while not check_event(...End)`.
    CheckEnd,
    /// A `compose_node` call returned this node.
    Deliver(NodeId),
}

impl Loader<'_, '_> {
    /// `self.parser.<method>` from the composer: the `parser` property is two frames deep.
    fn c_check(&mut self, choices: &[EvId]) -> R<bool> {
        self.touch(2)?;
        self.check_event(choices)
    }

    fn c_get(&mut self) -> R<Event> {
        self.touch(2)?;
        self.get_event()?.ok_or_else(|| {
            Fail::crash(
                "AttributeError",
                "'NoneType' object has no attribute 'start_mark'",
            )
        })
    }

    fn c_peek(&mut self) -> R<Event> {
        self.touch(2)?;
        self.peek_event()?.ok_or_else(|| {
            Fail::crash(
                "AttributeError",
                "'NoneType' object has no attribute 'anchor'",
            )
        })
    }

    /// `self.resolver` from the composer (`Composer.resolver` → `YAML.resolver`).
    fn c_resolver(&mut self) -> R<()> {
        self.frame(Self::resolver_prop)
    }

    /// `Composer.check_node()`.
    pub(super) fn check_node(&mut self) -> R<bool> {
        if self.c_check(&[EvId::StreamStart])? {
            self.c_get()?;
        }
        Ok(!self.c_check(&[EvId::StreamEnd])?)
    }

    /// `Composer.get_node()`.
    pub(super) fn get_node(&mut self) -> R<Option<NodeId>> {
        if !self.c_check(&[EvId::StreamEnd])? {
            return Ok(Some(self.frame(Self::compose_document)?));
        }
        Ok(None)
    }

    /// `Composer.get_single_node()`.
    pub(super) fn get_single_node(&mut self) -> R<Option<NodeId>> {
        self.c_get()?;
        let document = if self.c_check(&[EvId::StreamEnd])? {
            None
        } else {
            Some(self.frame(Self::compose_document)?)
        };
        if !self.c_check(&[EvId::StreamEnd])? {
            let ev = self.c_get()?;
            let Some(doc) = document else {
                return Err(Fail::crash(
                    "AttributeError",
                    "'NoneType' object has no attribute 'start_mark'",
                ));
            };
            self.touch(1)?;
            let dm = self.st.cons.node(doc).start.clone();
            return Err(marked(
                Some("expected a single document in the stream"),
                Some(&dm),
                Some(b"but found another document"),
                Some(&ev.start),
                None,
            ));
        }
        self.c_get()?;
        Ok(document)
    }

    /// `Composer.compose_document()` (the caller pushed its frame).
    fn compose_document(&mut self) -> R<NodeId> {
        self.anchors.clear();
        self.c_get()?;
        let node = self.compose_tree()?;
        self.c_get()?;
        Ok(node)
    }

    /// `compose_node(None, None)` and everything below it.
    fn compose_tree(&mut self) -> R<NodeId> {
        let mut stack: Vec<(Work, u32)> = Vec::new();
        let mut step = Step::Enter;
        loop {
            step = match step {
                Step::Enter => self.compose_node_enter(&mut stack)?,
                Step::CheckEnd => self.compose_check_end(&mut stack)?,
                Step::Deliver(node) => {
                    let Some((top, _)) = stack.last_mut() else {
                        return Ok(node);
                    };
                    match top {
                        Work::Seq(parent) => {
                            let parent = *parent;
                            if let NodeKind::Seq(items) = &mut self.st.cons.node_mut(parent).kind {
                                items.push(node);
                            }
                            Step::CheckEnd
                        }
                        Work::Map(parent, key) => match key.take() {
                            None => {
                                *key = Some(node);
                                Step::Enter
                            }
                            Some(k) => {
                                let parent = *parent;
                                if let NodeKind::Map(items) =
                                    &mut self.st.cons.node_mut(parent).kind
                                {
                                    items.push((k, node));
                                }
                                Step::CheckEnd
                            }
                        },
                    }
                }
            };
        }
    }

    /// The body of `compose_node` up to the collection loop (or the whole call for scalars and aliases).
    fn compose_node_enter(&mut self, stack: &mut Vec<(Work, u32)>) -> R<Step> {
        let saved = self.push_frame()?;
        if self.c_check(&[EvId::Alias])? {
            let ev = self.c_get()?;
            let alias = ev.anchor.clone().unwrap_or_default();
            let Some(&node) = self.anchors.get(&alias) else {
                self.touch(1)?;
                let p = format!("found undefined alias {}", repr_str(&alias));
                return Err(marked(
                    None,
                    None,
                    Some(p.as_bytes()),
                    Some(&ev.start),
                    None,
                ));
            };
            self.touch(1)?; // return_alias
            self.depth = saved;
            return Ok(Step::Deliver(node));
        }
        let ev = self.c_peek()?;
        if let Some(a) = &ev.anchor
            && self.anchors.contains_key(a)
        {
            // ReusedAnchorWarning: the first display imports what linecache needs.
            self.touch(1)?; // Mark.__str__ in the f-string
            self.touch(if self.st.warned { 6 } else { 23 })?;
            self.st.warned = true;
        }
        self.c_resolver()?;
        self.touch(1)?; // descend_resolver
        if self.c_check(&[EvId::Scalar])? {
            let node = self.frame(|s| s.compose_scalar_node(ev.anchor.clone()))?;
            self.compose_node_exit(saved)?;
            return Ok(Step::Deliver(node));
        }
        let (is_seq, is_map) = if self.c_check(&[EvId::SeqStart])? {
            (true, false)
        } else if self.c_check(&[EvId::MapStart])? {
            (false, true)
        } else {
            (false, false)
        };
        if !is_seq && !is_map {
            return Err(Fail::crash(
                "UnboundLocalError",
                "cannot access local variable 'node' where it is not associated with a value",
            ));
        }
        // compose_sequence_node / compose_mapping_node
        self.push_frame()?;
        let start = self.c_get()?;
        let (tag, tag_cache) = self.collection_tag(&start, is_seq)?;
        self.touch(if is_seq { 2 } else { 3 })?; // SequenceNode / MappingNode __init__
        let node = self.st.cons.push_node(Node {
            tag,
            kind: if is_seq {
                NodeKind::Seq(Vec::new())
            } else {
                NodeKind::Map(Vec::new())
            },
            start: start.start.clone(),
            end: start.start.clone(),
            merge: None,
            anchor: ev.anchor.clone(),
            tag_cache,
        });
        if let Some(a) = ev.anchor {
            self.anchors.insert(a, node);
        }
        stack.push((
            if is_seq {
                Work::Seq(node)
            } else {
                Work::Map(node, None)
            },
            saved,
        ));
        Ok(Step::CheckEnd)
    }

    /// `ascend_resolver()`, `self.depth -= 1` and the return of `compose_node`.
    fn compose_node_exit(&mut self, saved: u32) -> R<()> {
        self.c_resolver()?;
        self.touch(1)?;
        self.depth = saved;
        Ok(())
    }

    /// The `while not check_event(SequenceEndEvent / MappingEndEvent)` test of the innermost collection.
    fn compose_check_end(&mut self, stack: &mut Vec<(Work, u32)>) -> R<Step> {
        let Some((top, saved)) = stack.last() else {
            return Ok(Step::Enter);
        };
        let (node, end_id) = match top {
            Work::Seq(n) => (*n, EvId::SeqEnd),
            Work::Map(n, _) => (*n, EvId::MapEnd),
        };
        let saved = *saved;
        if !self.c_check(&[end_id])? {
            return Ok(Step::Enter);
        }
        let end = self.c_get()?;
        self.st.cons.node_mut(node).end = end.end;
        self.touch(1)?; // check_end_doc_comment
        stack.pop();
        // leave compose_*_node, then the rest of compose_node
        self.depth = saved.saturating_add(1);
        self.compose_node_exit(saved)?;
        Ok(Step::Deliver(node))
    }

    /// The resolved tag of a collection start event (`tag is None or str(tag) == '!'` → `resolve(...)`).
    fn collection_tag(&mut self, ev: &Event, is_seq: bool) -> R<(String, TagCache)> {
        if let Some(t) = &ev.tag {
            self.touch(2)?;
            if t != "!" {
                return Ok((t.clone(), TagCache::Cached));
            }
        }
        self.c_resolver()?;
        self.touch(1)?; // resolve(): no implicit resolvers for collections
        Ok(if is_seq {
            ("tag:yaml.org,2002:seq".to_owned(), TagCache::Default(1))
        } else {
            ("tag:yaml.org,2002:map".to_owned(), TagCache::Default(2))
        })
    }

    /// `compose_scalar_node(anchor)` (the caller pushed its frame).
    fn compose_scalar_node(&mut self, anchor: Option<String>) -> R<NodeId> {
        let ev = self.c_get()?;
        let explicit = match &ev.tag {
            Some(t) => {
                self.touch(2)?;
                (t != "!").then(|| t.clone())
            }
            None => None,
        };
        let (tag, tag_cache) = if let Some(t) = explicit {
            (t, TagCache::Cached)
        } else {
            self.c_resolver()?;
            let t = self.frame(|s| s.resolve_scalar(&ev.value, ev.implicit.0))?;
            let cache = if t == super::resolver::STR {
                TagCache::Default(0)
            } else {
                TagCache::Fresh
            };
            (t, cache)
        };
        self.touch(2)?; // ScalarNode.__init__ -> Node.__init__
        let node = self.st.cons.push_node(Node {
            tag,
            kind: NodeKind::Scalar(ev.value),
            start: ev.start,
            end: ev.end,
            merge: None,
            anchor: anchor.clone(),
            tag_cache,
        });
        if let Some(a) = anchor {
            self.anchors.insert(a, node);
        }
        Ok(node)
    }
}
