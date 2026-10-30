//! Replays the PyYAML oracle vectors (`tests/vectors/*.json`, `cargo xtask yaml-vectors`): typing and quoting of
//! 500+ scalars, the load and error corpus, composed arenas, dumps and `decode_text`.
//!
//! Owner: WP-14.

#[cfg(test)]
mod support;

#[cfg(test)]
mod vectors {
    use std::collections::HashMap;
    use std::fmt::Write as _;

    use cfgv::{Map, Value};
    use serde_json::Value as Json;
    use snp_yaml::emit::{Quote, dump, redump, scalar_for_rev};
    use snp_yaml::{
        Limits, NodeId, NodeKind, ScalarStyle, Schema, compose, decode_text, resolve_plain,
    };

    use crate::support;
    use crate::support::{check_load, report, vectors};

    #[test]
    fn scalar_typing_and_quoting_table_matches_pyyaml() {
        let vecs = vectors("scalars.json");
        assert!(vecs.len() >= 500, "the table has {} scalars", vecs.len());
        let mut failures = Vec::new();
        for v in &vecs {
            let s = v["s"].as_str().expect("s");
            let tag = resolve_plain(s, Schema::PyYaml11Safe).as_tag();
            if tag != v["tag"] {
                failures.push(format!("resolve {s:?}: expected {}, got {tag}", v["tag"]));
            }
            for (key, text) in [
                ("load_plain", format!("k: {s}\n")),
                ("load_tagged", String::new()),
            ] {
                if key == "load_tagged" {
                    continue;
                }
                if let Err(e) = check_load(&text, &v[key], key) {
                    failures.push(e);
                }
            }
            let quotes = [Quote::Plain, Quote::Single, Quote::Double];
            for (i, q) in quotes.into_iter().enumerate() {
                let got = scalar_for_rev(s, q);
                if v["rev"][i] != got.as_str() {
                    failures.push(format!(
                        "rev {s:?} {q:?}: expected {}, got {got:?}",
                        v["rev"][i]
                    ));
                }
            }
            let mut m = Map::new();
            m.insert(Value::from("k"), Value::from(s));
            let got = dump(&Value::Map(m));
            if v["dump"] != got.as_str() {
                failures.push(format!("dump {s:?}: expected {}, got {got:?}", v["dump"]));
            }
        }
        report(&failures, vecs.len());
    }

    /// The tagged form `k: !<tag> "<escaped s>"` exercises every constructor on the exact scalar text.
    #[test]
    fn scalar_constructors_match_pyyaml() {
        let vecs = vectors("scalars.json");
        let mut failures = Vec::new();
        for v in &vecs {
            let s = v["s"].as_str().expect("s");
            let tag = v["tag"].as_str().expect("tag");
            let mut quoted = scalar_for_rev(s, Quote::Double);
            if quoted.contains('\n') {
                // Folded at 80 columns: re-join the continuation lines for the one-line document.
                quoted = dump_one_line(s);
            }
            let text = format!("k: !<{tag}> {quoted}\n");
            if let Err(e) = check_load(&text, &v["load_tagged"], "load_tagged") {
                failures.push(e);
            }
        }
        report(&failures, vecs.len());
    }

    /// A double-quoted literal of `s` without folding.
    fn dump_one_line(s: &str) -> String {
        let mut out = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if c.is_ascii_graphic() || c == ' ' => out.push(c),
                c => write!(out, "\\U{:08X}", u32::from(c)).unwrap(),
            }
        }
        out.push('"');
        out
    }

    #[test]
    fn load_and_error_corpus_matches_pyyaml() {
        let vecs = vectors("load.json");
        let errors = vecs
            .iter()
            .filter(|v| v["result"].get("error").is_some())
            .count();
        assert!(errors >= 40, "the corpus has {errors} error documents");
        let mut failures = Vec::new();
        for v in &vecs {
            let text = v["yaml"].as_str().expect("yaml");
            if let Err(e) = check_load(text, &v["result"], "load") {
                failures.push(e);
            }
        }
        report(&failures, vecs.len());
    }

    #[test]
    fn composed_arenas_match_pyyaml() {
        let vecs = vectors("compose.json");
        let mut failures = Vec::new();
        for v in &vecs {
            let text = v["yaml"].as_str().expect("yaml");
            let doc = match compose(text, Schema::PyYaml11Safe, &Limits::default()) {
                Ok(doc) => doc,
                Err(e) => {
                    failures.push(format!("compose {text:?}: {e}"));
                    continue;
                }
            };
            let expected = v["nodes"].as_array().expect("nodes");
            let got = arena_json(&doc, text);
            if got != *expected {
                failures.push(format!(
                    "compose {text:?}\n  expected {}\n       got {}",
                    Json::Array(expected.clone()),
                    Json::Array(got)
                ));
            }
        }
        report(&failures, vecs.len());
    }

    /// The generator's view of a composed arena: nodes numbered in first-visit order from the root.
    struct Arena<'a> {
        doc: &'a snp_yaml::Document,
        text: &'a str,
        index_of: HashMap<usize, usize>,
        out: Vec<Json>,
    }

    impl Arena<'_> {
        /// Character index (after a BOM) of a byte offset of the text.
        fn char_index(&self, byte: usize) -> usize {
            let body_start = if self.text.starts_with('\u{feff}') {
                3
            } else {
                0
            };
            self.text
                .get(body_start..byte)
                .map_or(0, |s| s.chars().count())
        }

        /// Pre-order with first-visit numbering, like the generator's recursive visit.
        fn visit(&mut self, id: NodeId) -> usize {
            let id = usize::try_from(id.0).unwrap();
            if let Some(i) = self.index_of.get(&id) {
                return *i;
            }
            let i = self.out.len();
            self.index_of.insert(id, i);
            let n = &self.doc.nodes[id];
            let mut rec = serde_json::json!({
                "tag": &*n.tag,
                "start": [n.start.index, n.start.line, n.start.column],
                "end": self.char_index(n.span.end),
            });
            self.out.push(Json::Null);
            let extra = match &n.kind {
                NodeKind::Scalar(s) => {
                    let style = match n.style {
                        ScalarStyle::Plain => "",
                        ScalarStyle::Single => "'",
                        ScalarStyle::Double => "\"",
                        ScalarStyle::Literal => "|",
                        ScalarStyle::Folded => ">",
                        _ => "?",
                    };
                    serde_json::json!({"scalar": &**s, "style": style})
                }
                NodeKind::Seq(items) => {
                    let ids: Vec<usize> = items.iter().map(|c| self.visit(*c)).collect();
                    serde_json::json!({ "seq": ids })
                }
                NodeKind::Map(pairs) => {
                    let ids: Vec<[usize; 2]> = pairs
                        .iter()
                        .map(|(k, v)| {
                            let k = self.visit(*k);
                            [k, self.visit(*v)]
                        })
                        .collect();
                    serde_json::json!({ "map": ids })
                }
                _ => serde_json::json!({}),
            };
            if let (Json::Object(r), Json::Object(e)) = (&mut rec, extra) {
                r.extend(e);
            }
            self.out[i] = rec;
            i
        }
    }

    /// The arena in the generator's format.
    fn arena_json(doc: &snp_yaml::Document, text: &str) -> Vec<Json> {
        let mut arena = Arena {
            doc,
            text,
            index_of: HashMap::new(),
            out: Vec::new(),
        };
        if let Some(root) = doc.root {
            arena.visit(root);
        }
        arena.out
    }

    #[test]
    fn dumps_match_pyyaml() {
        let vecs = vectors("dump.json");
        let mut failures = Vec::new();
        for v in &vecs {
            let text = v["yaml"].as_str().expect("yaml");
            let value = match support::load(text) {
                Ok(value) => value,
                Err(e) => {
                    failures.push(format!("load {text:?}: {e}"));
                    continue;
                }
            };
            let expected = v["dump"].as_str().expect("dump");
            // `dump` writes a tree; PyYAML anchors shared objects, which only `redump` sees.
            if !expected.contains("&id0") {
                let got = dump(&value);
                if expected != got {
                    failures.push(format!(
                        "dump of {text:?}\n  expected {expected:?}\n       got {got:?}"
                    ));
                }
            }
            match redump(text, &Limits::default()) {
                Ok(got) if expected == got => {}
                other => failures.push(format!(
                    "redump of {text:?}\n  expected {expected:?}\n       got {other:?}"
                )),
            }
        }
        report(&failures, vecs.len());
    }

    /// 1,900+ seeded random documents (block and flow styles, anchors and merges, mutations, block scalars, odd line
    /// breaks, BOMs, `%XX` tags, long keys, inputs beyond the reader's 16 KiB chunk with control characters): the load
    /// outcome and `yaml_dump(yaml_load(text))` with its `&idNNN` anchors.
    #[test]
    fn random_documents_match_pyyaml() {
        let vecs = vectors("random.json");
        assert!(vecs.len() >= 1900, "{} random documents", vecs.len());
        let lim = Limits::default();
        let mut failures = Vec::new();
        for v in &vecs {
            let text = v["yaml"].as_str().expect("yaml");
            if let Err(e) = check_load(text, &v["result"], "load") {
                failures.push(e);
                continue;
            }
            if let Some(expected) = v.get("redump") {
                match redump(text, &lim) {
                    Ok(got) if expected == got.as_str() => {}
                    other => failures.push(format!(
                        "redump {text:?}\n  expected {expected}\n       got {other:?}"
                    )),
                }
            }
        }
        report(&failures, vecs.len());
    }

    #[test]
    fn decode_text_matches_cpython() {
        let vecs = vectors("decode.json");
        let mut failures = Vec::new();
        for v in &vecs {
            let hex = v["hex"].as_str().expect("hex");
            let bytes: Vec<u8> = (0..hex.len() / 2)
                .map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2).unwrap(), 16).unwrap())
                .collect();
            let got = match decode_text(&bytes) {
                Ok(text) => serde_json::json!({ "text": text }),
                Err(e) => serde_json::json!({ "error": e.to_string() }),
            };
            let mut expected = v.clone();
            if let Json::Object(m) = &mut expected {
                m.remove("hex");
            }
            if got != expected {
                failures.push(format!("decode {hex}: expected {expected}, got {got}"));
            }
        }
        report(&failures, vecs.len());
    }
}
