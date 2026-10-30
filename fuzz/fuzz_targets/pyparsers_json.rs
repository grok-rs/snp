//! Fuzz target `pyparsers_json`: CPython `json` decoder.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The first byte picks the CPython minor and the pretty-format options; the rest is the document, checked
//! (`check-json` v4+ and v3), loaded, re-encoded (`dumps_pretty`) and dropped, and run through
//! `pretty-format-json`'s formatter. Documents over 4 KiB get no indent, whose output would be quadratic in the
//! nesting depth. `JsonValue`'s iterative drop/clone/eq/debug at CPython's ~10 000-level thresholds are exercised on
//! 256 KiB stacks by the crate's unit tests (`json::tree`): calling them here, or a bracket-wrapping mode for deep
//! inputs, pushed the ASan quarantine to ~500 MB RSS, at the 512 MB budget.
//!
//! Owner: WP-17.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pyparsers::json::{self, DumpOpts, PyMinor};

fuzz_target!(|data: &[u8]| {
    let Some((&sel, doc)) = data.split_first() else {
        return;
    };
    let minor = if sel & 1 == 0 {
        PyMinor::P313
    } else {
        PyMinor::P312
    };
    drop(json::check(doc, minor));
    drop(json::check_ex(doc, minor));
    drop(json::check_v3(doc, minor));
    let opts = DumpOpts {
        // The pretty text of an indented document is quadratic in its depth (hundreds of MB at 10 000 levels, as
        // in Python), so large documents get no indent and stay within the 512 MB budget.
        indent: if doc.len() > 4096 {
            "0".to_owned()
        } else if sel & 2 == 0 {
            "  ".to_owned()
        } else {
            "\t".to_owned()
        },
        ensure_ascii: sel & 4 == 0,
        sort_keys: sel & 8 == 0,
        top_keys: if sel & 16 == 0 {
            Vec::new()
        } else {
            vec!["a".to_owned(), String::new()]
        },
    };
    if let Ok(v) = json::loads(doc, minor) {
        drop(json::dumps_pretty(&v, &opts));
    }
    if let Ok(text) = std::str::from_utf8(doc) {
        drop(json::pretty_format(text, &opts, minor));
    }
});
