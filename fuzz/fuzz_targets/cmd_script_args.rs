//! Fuzz target `cmd_script_args`: the hook-script era parsers (11 §A.5.3).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-33. W0 stub: accepts every input until WP-33 drives `snp_cmd` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_cmd as _;

fuzz_target!(|_data: &[u8]| {});
