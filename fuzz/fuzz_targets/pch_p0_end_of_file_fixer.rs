//! Fuzz target `pch_p0_end_of_file_fixer`: the native `end-of-file-fixer` core on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The tail-only decision (from the last bytes that contain a non-EOL byte) equals the whole-file decision, and
//! the fix is idempotent.
//!
//! Owner: WP-26.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels;

fuzz_target!(|data: &[u8]| {
    let len = data.len() as u64;
    let whole = kernels::end_of_file(data, len);
    // The tail the native reads: from the last non-EOL byte on (or the whole file).
    let start = data
        .iter()
        .rposition(|&b| b != b'\n' && b != b'\r')
        .unwrap_or(0);
    assert_eq!(kernels::end_of_file(&data[start..], len), whole);
    if let Some(fixed) = kernels::end_of_file_bytes(data) {
        assert!(kernels::end_of_file_bytes(&fixed).is_none());
    }
});
