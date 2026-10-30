//! Fuzz target `pch_p0_no_commit_to_branch`: the native `no-commit-to-branch` argparse and its `re.match` of a
//! `--pattern` against a branch name, on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input is `pattern NUL branch NUL argv…`.
//!
//! Owner: WP-26.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels;
use snp_pyre::{Options, PyRegex};

fuzz_target!(|data: &[u8]| {
    let mut parts = data.split(|&b| b == 0);
    let pattern = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
    let branch = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
    if let Ok(re) = PyRegex::compile(&pattern, Options::CONFIG) {
        drop(re.match_prefix(&branch));
    }
    let argv: Vec<String> = parts
        .take(16)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let env = snp_core::env::Env::from_pairs(Vec::<(String, String)>::new());
    drop(kernels::parse_console_script(
        "no-commit-to-branch",
        2,
        &argv,
        &env,
    ));
});
