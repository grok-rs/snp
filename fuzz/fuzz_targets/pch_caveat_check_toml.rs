//! Fuzz target `pch_caveat_check_toml`: the native `check-toml` (CPython 3.13 `tomllib` port) on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-27.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_pyparsers::json::PyMinor;

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
    for minor in [PyMinor::P312, PyMinor::P313] {
        if let Err(e) = snp_pyparsers::toml::check_ex(data, minor) {
            assert!(!e.into_message().is_empty());
        }
    }
    parse("caveats", "check-toml", 1, data);
});
