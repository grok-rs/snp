//! Fuzz target `pyre_compile_match`: Python-`re` compile + match (str and bytes).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-13. W0 stub: accepts every input until WP-13 drives `snp_pyre` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pyre as _;

fuzz_target!(|_data: &[u8]| {});
