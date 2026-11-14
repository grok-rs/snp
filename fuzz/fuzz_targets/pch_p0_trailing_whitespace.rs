//! Fuzz target `pch_p0_trailing_whitespace`: the native `trailing-whitespace` core on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The first byte selects markdown mode and a `--chars` set (taken from the next bytes up to a NUL); the rest is
//! the file. The fix is what a line-by-line transliteration of upstream's `_fix_file` gives (the kernel's
//! borrowed-parts fast path against the former copying code), is shorter than the file (it only deletes bytes, so
//! the kernel's output reservation at the file's length holds it) and, with the default set, is idempotent; the
//! console script's argparse runs over the selector bytes as argv.
//!
//! Owner: WP-26.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels;

/// `bytes.isspace()` / `bytes.rstrip()` whitespace.
const ASCII_WS: &[u8] = b" \t\n\r\x0b\x0c";

/// `trailing_whitespace_fixer._fix_file` (`:8-22`) and `_process_line` (`:25-41`): `readlines()`, each line
/// rewritten and joined; `None` when nothing changed.
fn oracle(content: &[u8], md: bool, chars: Option<&[u8]>) -> Option<Vec<u8>> {
    let rstrip = |s: &[u8]| -> Vec<u8> {
        let mut v = s.to_vec();
        while v
            .last()
            .is_some_and(|b| chars.unwrap_or(ASCII_WS).contains(b))
        {
            v.pop();
        }
        v
    };
    let mut out = Vec::new();
    for line in content.split_inclusive(|&b| b == b'\n') {
        let (body, eol): (&[u8], &[u8]) = if let Some(b) = line.strip_suffix(b"\r\n") {
            (b, b"\r\n")
        } else if let Some(b) = line.strip_suffix(b"\n") {
            (b, b"\n")
        } else {
            (line, b"")
        };
        let isspace = !body.is_empty() && body.iter().all(|b| ASCII_WS.contains(b));
        if md && !isspace && body.ends_with(b"  ") {
            out.extend(rstrip(&body[..body.len() - 2]));
            out.extend_from_slice(b"  ");
        } else {
            out.extend(rstrip(body));
        }
        out.extend_from_slice(eol);
    }
    (out != content).then_some(out)
}

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
    let got = kernels::trailing_whitespace(content, md, chars);
    assert_eq!(got, oracle(content, md, chars));
    if let Some(fixed) = got {
        assert!(fixed.len() < content.len());
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
