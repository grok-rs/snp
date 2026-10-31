//! Fuzz target `pch_p0_detect_private_key`: the native `detect-private-key` scan on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The 8-pattern epoch never finds more than the 10-pattern epoch.
//!
//! Owner: WP-26.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels;

fuzz_target!(|data: &[u8]| {
    let old = kernels::has_private_key(data, 1);
    let new = kernels::has_private_key(data, 2);
    assert!(!old || new);
});
