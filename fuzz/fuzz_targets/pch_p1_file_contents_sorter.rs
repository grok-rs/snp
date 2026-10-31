//! Fuzz target `pch_p1_file_contents_sorter`: the native `file-contents-sorter` core on arbitrary bytes (the first byte selects the flags
//! and the epoch); sorting is idempotent.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-27.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p1::kernels::{SortOpts, file_contents_sort};

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
    let o = SortOpts {
        ignore_case: sel & 1 == 1,
        unique: sel & 2 == 2,
        strip_both: sel & 4 == 4,
        empty_stays_empty: sel & 8 == 8,
    };
    if let Some(sorted) = file_contents_sort(rest, o) {
        assert!(file_contents_sort(&sorted, o).is_none());
    }
    parse("p1", "file-contents-sorter", u16::from(sel >> 4) % 6, rest);
});
