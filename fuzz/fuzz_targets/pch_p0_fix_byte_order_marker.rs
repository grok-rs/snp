//! Fuzz target `pch_p0_fix_byte_order_marker`: the native `fix-byte-order-marker` core on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! With a BOM the result is the rest of the file exactly when it is strict UTF-8 (else the `UnicodeDecodeError`
//! text); without one nothing changes.
//!
//! Owner: WP-26.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels;

fuzz_target!(|data: &[u8]| {
    match kernels::fix_byte_order_marker(data) {
        Ok(None) => assert!(!data.starts_with(kernels::BOM)),
        Ok(Some(rest)) => {
            assert_eq!(rest, &data[3..]);
            assert!(std::str::from_utf8(rest).is_ok());
        }
        Err(msg) => assert!(msg.starts_with("'utf-8' codec can't decode")),
    }
});
