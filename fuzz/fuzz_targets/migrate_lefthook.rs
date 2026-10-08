//! Fuzz target `migrate_lefthook`: the lefthook converter.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-34. W0 stub: accepts every input until WP-34 drives `snp_migrate` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_migrate as _;

fuzz_target!(|_data: &[u8]| {});
