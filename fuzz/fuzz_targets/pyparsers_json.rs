//! Fuzz target `pyparsers_json`: CPython `json` decoder.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The first byte picks the CPython minor and the pretty-format options; the rest is the document, checked
//! (`check-json`), loaded and re-encoded (`dumps_pretty`), and run through `pretty-format-json`'s formatter.
//!
//! Owner: WP-17.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pyparsers::json::{self, DumpOpts, PyMinor};

fuzz_target!(|data: &[u8]| {
    let Some((&sel, doc)) = data.split_first() else {
        return;
    };
    let minor = if sel & 1 == 0 { PyMinor::P313 } else { PyMinor::P312 };
    drop(json::check(doc, minor));
    drop(json::check_ex(doc, minor));
    let opts = DumpOpts {
        indent: if sel & 2 == 0 { "  ".to_owned() } else { "\t".to_owned() },
        ensure_ascii: sel & 4 == 0,
        sort_keys: sel & 8 == 0,
        top_keys: if sel & 16 == 0 { Vec::new() } else { vec!["a".to_owned(), String::new()] },
    };
    if let Ok(v) = json::loads(doc, minor) {
        drop(json::dumps_pretty(&v, &opts));
    }
    if let Ok(text) = std::str::from_utf8(doc) {
        drop(json::pretty_format(text, &opts, minor));
    }
});
