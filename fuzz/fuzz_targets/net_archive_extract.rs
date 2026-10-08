//! Fuzz target `net_archive_extract`: safe archive extraction.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-16. W0 stub: accepts every input until WP-16 drives `snp_net` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_net as _;

fuzz_target!(|_data: &[u8]| {});
