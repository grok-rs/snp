//! Fuzz target `pch_p1_destroyed_symlinks`: the native `destroyed-symlinks` core: `git status --porcelain=v2 -z` records, the content
//! comparison and the report on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-27.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p1::kernels::{
    destroyed_symlink_candidates, destroyed_symlinks_report, same_after_rstrip,
};

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
    let text = String::from_utf8_lossy(data);
    let (candidates, _err) = destroyed_symlink_candidates(&text);
    let paths: Vec<String> = candidates.iter().map(|c| c.path.clone()).collect();
    if !paths.is_empty() {
        assert!(destroyed_symlinks_report(&paths).len() >= paths.len() + 5);
    }
    let (a, b) = data.split_at(data.len() / 2);
    assert_eq!(same_after_rstrip(a, b), same_after_rstrip(b, a));
    parse("p1", "destroyed-symlinks", 1, data);
});
