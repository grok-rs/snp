//! Fuzz target `pch_p0_check_json`: the native `check-json` on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-26. W0 stub: accepts every input until WP-26 drives `snp_hooks` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_hooks as _;

fuzz_target!(|_data: &[u8]| {});
