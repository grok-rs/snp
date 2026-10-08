//! Fuzz target `pyparsers_json`: CPython `json` decoder.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-17. W0 stub: accepts every input until WP-17 drives `snp_pyparsers` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pyparsers as _;

fuzz_target!(|_data: &[u8]| {});
