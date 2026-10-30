//! Fuzz target `pch_p0_trailing_whitespace`: the native `trailing-whitespace` core on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The first byte selects markdown mode and a `--chars` set (taken from the next bytes up to a NUL); the rest is
//! the file. The fix never grows the file and, with the default set, is idempotent; the console
//! script's argparse runs over the selector bytes as argv.
//!
//! Owner: WP-26.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels;

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let md = sel & 1 == 1;
    let (chars, content) = if sel & 2 == 2 {
        match rest.iter().position(|&b| b == 0) {
            Some(i) => (Some(&rest[..i]), &rest[i + 1..]),
            None => (Some(rest), &b""[..]),
        }
    } else {
        (None, rest)
    };
    if let Some(fixed) = kernels::trailing_whitespace(content, md, chars) {
        assert!(fixed.len() <= content.len());
        // Idempotent for the default set (ASCII whitespace includes `\r`, so no new CRLF can appear); a custom
        // `--chars` set can expose a `\r` that turns into a CRLF ending, in Python as here.
        if !md && chars.is_none() {
            assert!(kernels::trailing_whitespace(&fixed, md, chars).is_none());
        }
    }
    let argv: Vec<String> = content
        .split(|&b| b == 0)
        .take(8)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let env = snp_core::env::Env::from_pairs(Vec::<(String, String)>::new());
    drop(kernels::parse_console_script(
        "trailing-whitespace-fixer",
        1,
        &argv,
        &env,
    ));
});
