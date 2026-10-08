//! Fuzz target `store_marker_parse`: store marker parsing.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-22. W0 stub: accepts every input until WP-22 drives `snp_store` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_store as _;

fuzz_target!(|_data: &[u8]| {});
