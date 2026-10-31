//! Fuzz target `pch_p1_pretty_format_json`: the native `pretty-format-json` core (CPython `json` decode, `pairs_first`, encode, `difflib`) on
//! arbitrary bytes: the first byte selects the options, the rest is the file text.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-27.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_pyparsers::json::{DumpOpts, PyMinor};

/// The console script's argparse over up to eight NUL-separated argv items taken from `data`.
fn parse(group: &str, script: &str, epoch: u16, data: &[u8]) {
    let argv: Vec<String> = data
        .split(|&b| b == 0)
        .take(8)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let env = snp_core::env::Env::from_pairs(Vec::<(String, String)>::new());
    let parsed = match group {
        "p1" => snp_hooks::pch::p1::parse_console_script(script, epoch, &argv, &env),
        "p2" => snp_hooks::pch::p2::parse_console_script(script, epoch, &argv, &env),
        _ => snp_hooks::pch::caveats::parse_console_script(script, epoch, &argv, &env),
    };
    assert!(parsed.is_some());
}

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let indent = match sel & 3 {
        0 => "2",
        1 => "\t",
        2 => "-1",
        _ => "",
    };
    let opts = DumpOpts {
        indent: indent.to_owned(),
        ensure_ascii: sel & 4 == 0,
        sort_keys: sel & 8 == 0,
        top_keys: if sel & 16 == 0 {
            Vec::new()
        } else {
            vec!["a".to_owned(), "id".to_owned()]
        },
    };
    let contents =
        snp_pycompat::textio::universal_newlines(&String::from_utf8_lossy(rest)).into_owned();
    if let Ok(pretty) = snp_pyparsers::json::pretty_format(&contents, &opts, PyMinor::P313) {
        assert!(pretty.ends_with('\n'));
        let a: Vec<&str> = snp_pycompat::text::splitlines_keepends(&contents).collect();
        let b: Vec<&str> = snp_pycompat::text::splitlines_keepends(&pretty).collect();
        let diff = snp_pyparsers::difflib::unified_diff(&a, &b, "f", "f");
        assert_eq!(diff.is_empty(), contents == pretty);
    }
    parse("p1", "pretty-format-json", 2, rest);
});
