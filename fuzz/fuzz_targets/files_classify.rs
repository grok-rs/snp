//! Fuzz target `files_classify`: tiered classification.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-22. W0 stub: accepts every input until WP-22 drives `snp_files` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_files as _;

fuzz_target!(|_data: &[u8]| {});
