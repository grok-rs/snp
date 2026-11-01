//! Fuzz target `cmd_script_args`: the hook-script era parsers (11 §A.5.3) and the template renderer.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Input layout: byte 0 selects the owner class handed to `parse_script_args` (pre-commit eras 1–3 and unknown,
//! prek, snp 1.x, snp) and the hook type; the rest is the script bytes. The same bytes (up to the first NUL, as a
//! path can hold no NUL) are also rendered as the `--config` value and `SNP_EXE` of snp's template, whose output
//! must parse back through the snp-script parser.
//!
//! Owner: WP-33.
#![no_main]

use std::ffi::OsString;
use std::path::PathBuf;

use libfuzzer_sys::fuzz_target;
use snp_cmd::scripts::{self, Owner};
use snp_core::stage::HookType;

fn os(bytes: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        OsString::from_vec(bytes.to_vec())
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, script)) = data.split_first() else {
        return;
    };
    let owner = match mode % 8 {
        0 => Owner::PreCommit { era: Some(1) },
        1 => Owner::PreCommit { era: Some(2) },
        2 => Owner::PreCommit { era: Some(3) },
        3 => Owner::PreCommit { era: None },
        4 => Owner::Prek,
        5 => Owner::SnpV1,
        6 => Owner::Snp { stale: false },
        _ => Owner::Foreign,
    };
    let _ = scripts::parse_script_args(&owner, script);

    let word = script.split(|&b| b == 0).next().unwrap_or_default();
    let t = HookType::ALL[usize::from(mode >> 4) % HookType::ALL.len()];
    let exe = PathBuf::from(os(word));
    let rendered = scripts::render(&os(word), t, mode & 8 != 0, &exe);
    let back = scripts::parse_script_args(&Owner::Snp { stale: false }, &rendered);
    // A rendered script always parses back (a newline in the word is quoted, so lines stay intact).
    if back.is_none() && !word.is_empty() && !word.contains(&b'\n') && !word.contains(&b'\r') {
        panic!("rendered script did not parse back: {word:?}");
    }
});
