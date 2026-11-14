//! `parse_shebang_bytes` / `parse_shebang`: no panic, bounded output, and
//! the same result however the input is split into reads.
#![no_main]

mod identify_common;

use file_identify::{MAX_SHEBANG_BYTES, parse_shebang, parse_shebang_bytes};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (control, content) = identify_common::control(data);
    let cmd = parse_shebang_bytes(content);
    // Every token is (an unquoted part of) the input.
    let total: usize = cmd.iter().map(String::len).sum();
    assert!(total <= content.len());
    // A shebang line longer than the limit is ignored.
    if let Some(first) = cmd.first() {
        assert!(first.len() <= MAX_SHEBANG_BYTES);
    }
    let streamed = parse_shebang(identify_common::Chunked::new(content, control)).expect("in-memory read");
    assert_eq!(streamed, cmd);
});
