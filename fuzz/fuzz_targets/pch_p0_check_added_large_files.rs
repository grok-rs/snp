//! Fuzz target `pch_p0_check_added_large_files`: the native `check-added-large-files` argparse (all three epochs,
//! `--maxkb` Python `int()` included) and size arithmetic on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-26.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels;

fuzz_target!(|data: &[u8]| {
    let argv: Vec<String> = data
        .split(|&b| b == 0)
        .take(16)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let env = snp_core::env::Env::from_pairs(Vec::<(String, String)>::new());
    for epoch in 1..=3 {
        if let Some(Ok(ns)) =
            kernels::parse_console_script("check-added-large-files", epoch, &argv, &env)
        {
            assert!(ns.int("maxkb").is_some());
        }
    }
    let size = data.iter().fold(0_u64, |acc, &b| {
        acc.wrapping_mul(131).wrapping_add(u64::from(b))
    });
    let kb = kernels::size_kb(size);
    assert!(kb.saturating_mul(1024) >= size);
});
