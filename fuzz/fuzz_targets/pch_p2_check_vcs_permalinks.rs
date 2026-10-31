//! Fuzz target `pch_p2_check_vcs_permalinks`: the native `check-vcs-permalinks` patterns: an arbitrary domain (up to the first NUL, 64
//! bytes at most) compiled as CPython bytes patterns per epoch, searched in every line of the rest.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-27.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p1::kernels::binary_lines;
use snp_hooks::pch::p2::kernels::permalink_pattern;
use snp_pyre::{Haystack, Mode, Options, PyRegex};

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

const OPTS: Options = Options {
    mode: Mode::Bytes,
    haystack: Haystack::Line,
    ignore_case: false,
    multiline_dotall: false,
};

fuzz_target!(|data: &[u8]| {
    let (domain, rest) = match data.iter().position(|&b| b == 0) {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (&b"github.com"[..], data),
    };
    let domain = String::from_utf8_lossy(&domain[..domain.len().min(64)]).into_owned();
    for epoch in 1..=3 {
        if let Ok(re) = PyRegex::compile(&permalink_pattern(&domain, epoch), OPTS) {
            for line in binary_lines(rest) {
                let _found = re.search_bytes(line);
            }
        }
    }
    parse("p2", "check-vcs-permalinks", 3, data);
});
