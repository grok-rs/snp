//! Fuzz target `pyre_compile_match`: Python-`re` compile + match (str and bytes).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Input: byte 0 selects the mode, the haystack shape and the pygrep flags; the rest is the pattern (UTF-8,
//! lossily decoded for str patterns). Every entry point runs: `validate`, `cfgv_is_valid`, `PyRegex::compile`,
//! then `search`, `match_prefix`, `fullmatch` and `search_bytes` over fixed haystacks (short, so that a
//! catastrophic tier-2 pattern stops at the backtrack limit quickly, 01 Addendum §A.8 item 6).
//!
//! Owner: WP-13.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pyre::{Haystack, Mode, Options, PyReError, PyRegex};

const HAYSTACKS: [&[u8]; 10] = [
    b"",
    b"\n",
    b"a",
    b"a\n",
    b"src/main.py",
    b"tests/test_x.py\n",
    b"caf\xc3\xa9 K\xe2\x84\xaa \xc4\xb0",
    b"\xff\xfe\x00x\r\n",
    b"aaaaaaaaaaaaaaab",
    b"# noqa: E501\n",
];

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let mode = if selector & 1 == 0 {
        Mode::Str
    } else {
        Mode::Bytes
    };
    let haystack = match (selector >> 1) & 3 {
        0 => Haystack::Path,
        1 => Haystack::Line,
        _ => Haystack::Blob,
    };
    let opts = Options {
        mode,
        haystack,
        ignore_case: selector & 8 != 0,
        multiline_dotall: selector & 16 != 0,
    };
    let pattern = String::from_utf8_lossy(rest);

    let validated = snp_pyre::validate(&pattern, mode);
    if mode == Mode::Str {
        let _ = snp_pyre::cfgv_is_valid(&pattern);
    }
    let compiled = PyRegex::compile(&pattern, opts);
    // The parser is the single accept/reject authority: compile may only add engine limits (D5/D6).
    match (&validated, &compiled) {
        (Ok(_), Ok(_)) | (Err(_), Err(_)) | (Ok(_), Err(PyReError::Unsupported(_))) => {}
        _ => panic!(
            "validate and compile disagree on {pattern:?}: {validated:?} vs {:?}",
            compiled.as_ref().err()
        ),
    }
    if let Err(e) = &compiled {
        let _ = (e.to_string(), e.py_type(), e.py_last_line());
        return;
    }
    let Ok(re) = compiled else { return };
    let _ = (re.source(), re.kind(), re.warnings());
    for h in HAYSTACKS {
        if haystack == Haystack::Line
            && h.iter()
                .take(h.len().saturating_sub(1))
                .any(|&b| b == b'\n')
        {
            continue;
        }
        let _ = re.search_bytes(h);
        if let Ok(text) = std::str::from_utf8(h) {
            let _ = re.search(text);
            let _ = re.match_prefix(text);
            let _ = re.fullmatch(text);
        }
    }
});
