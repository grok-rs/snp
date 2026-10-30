//! Fuzz target `yaml_compose_construct`: the PyYAML composer and constructor.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The first byte picks the schema and small or default limits (so the depth, node and size limits are hit
//! often); the rest is the text. Every composed document is constructed, its spans must slice the text on
//! character boundaries, and the resolver and the rev quoting run on the text as a scalar.
//!
//! Owner: WP-14.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_yaml::emit::{Quote, scalar_for_rev};
use snp_yaml::{Limits, Schema, compose, compose_all, construct, resolve_plain};

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(rest);
    let schema = if selector & 1 == 0 {
        Schema::PyYaml11Safe
    } else {
        Schema::Yaml12Core
    };
    let lim = if selector & 2 == 0 {
        Limits::default()
    } else {
        Limits {
            max_depth: 8,
            max_constructed_nodes: 64,
            max_bytes: 4096,
        }
    };
    if let Ok(doc) = compose(&text, schema, &lim) {
        for node in &doc.nodes {
            assert!(
                text.get(node.span.clone()).is_some(),
                "span {:?} of {text:?}",
                node.span
            );
        }
        let _ = construct(&doc, schema, &lim);
    }
    if let Ok(docs) = compose_all(&text, schema, &lim) {
        for doc in &docs {
            let _ = construct(doc, schema, &lim);
        }
    }
    let _ = resolve_plain(&text, schema);
    for q in [Quote::Plain, Quote::Single, Quote::Double] {
        let _ = scalar_for_rev(&text, q);
    }
});
