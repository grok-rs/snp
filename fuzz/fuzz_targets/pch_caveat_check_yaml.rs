//! Fuzz target `pch_caveat_check_yaml`: the native `check-yaml` (ruamel.yaml ≥ 0.19 port) on arbitrary bytes in its three modes,
//! two files per process so state carried between files is exercised.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-27.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_pyparsers::json::PyMinor;
use snp_pyparsers::yamlcheck::{Checker, Mode};

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
    let (a, b) = data.split_at(data.len() / 2);
    for mode in [Mode::Safe, Mode::AllowMultipleDocuments, Mode::Unsafe] {
        let mut c = Checker::new(mode, PyMinor::P313);
        for f in [a, b] {
            drop(c.check_file(f, "f.yaml"));
            for w in c.take_warnings() {
                assert!(w.render("site-packages").starts_with("site-packages/"));
            }
        }
    }
    parse("caveats", "check-yaml", 2, data);
});
