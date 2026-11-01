//! Fuzz target `pch_p1_requirements_txt_fixer`: the native `requirements-txt-fixer` core on arbitrary bytes at every epoch.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-27.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p1::kernels::{requirement_name, requirements_fix};

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
    for epoch in 1..=4 {
        // A rewrite always differs from the input; it may be empty (a lone `pkg-resources==0.0.0` line is removed
        // and the file truncated, as the Python hook does).
        if let Ok(Some(fixed)) = requirements_fix(data, epoch) {
            assert_ne!(fixed.as_slice(), data);
        }
    }
    drop(requirement_name(data));
    parse("p1", "requirements-txt-fixer", 4, data);
});
