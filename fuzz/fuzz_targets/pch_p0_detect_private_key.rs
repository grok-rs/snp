//! Fuzz target `pch_p0_detect_private_key`: the native `detect-private-key` scan on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The 8-pattern epoch never finds more than the 10-pattern epoch, and both answer what upstream's
//! `any(line in content for line in BLACKLIST)` does, by plain substring search (the kernel's anchor fast path
//! against the former all-pattern scan).
//!
//! Owner: WP-26.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels;

/// `needle in hay`, byte by byte.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    (0..=hay.len()).any(|i| hay[i..].starts_with(needle))
}

/// `detect_private_key.py:6-17` for the epoch's pattern set: the first 8 patterns up to v4.0.1, all 10 after.
fn oracle(data: &[u8], epoch: u16) -> bool {
    let n = if epoch >= 2 { 10 } else { 8 };
    kernels::PRIVATE_KEY_PATTERNS
        .iter()
        .take(n)
        .any(|p| contains(data, p))
}

fuzz_target!(|data: &[u8]| {
    let e1 = kernels::has_private_key(data, 1);
    let e2 = kernels::has_private_key(data, 2);
    assert!(!e1 || e2);
    assert_eq!(e1, oracle(data, 1));
    assert_eq!(e2, oracle(data, 2));
});
