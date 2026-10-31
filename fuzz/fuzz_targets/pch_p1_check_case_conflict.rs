//! Fuzz target `pch_p1_check_case_conflict`: the native `check-case-conflict` core on arbitrary `git ls-files` / file name / staged-added
//! lists (NUL-separated groups of newline-separated names).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-27.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p1::kernels::{case_conflicts, py_lower, py_str_cmp};

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

/// The newline-separated names of one group.
fn lines(g: Option<&[u8]>) -> Vec<&[u8]> {
    g.map(|g| g.split(|&b| b == b'\n').collect())
        .unwrap_or_default()
}

fuzz_target!(|data: &[u8]| {
    let mut groups = data.splitn(3, |&b| b == 0);
    let repo = lines(groups.next());
    let files = lines(groups.next());
    let added = lines(groups.next());
    for epoch in 1..=3 {
        for windows in [false, true] {
            let names = case_conflicts(&repo, &files, &added, epoch, windows);
            assert!(names.windows(2).all(|w| py_str_cmp(&w[0], &w[1]).is_le()));
        }
    }
    drop(py_lower(data));
    parse("p1", "check-case-conflict", 3, data);
});
