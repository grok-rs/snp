//! `shlex_split`: no panic, tokens are never longer than the input.
#![no_main]

use file_identify::shlex_split;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(tokens) = shlex_split(s) {
        let total: usize = tokens.iter().map(String::len).sum();
        assert!(total <= s.len());
    }
});
