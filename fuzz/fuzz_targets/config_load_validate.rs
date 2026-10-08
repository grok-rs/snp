//! Fuzz target `config_load_validate`: config load + validation.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-21. W0 stub: accepts every input until WP-21 drives `snp_config` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_config as _;

fuzz_target!(|_data: &[u8]| {});
