//! Fuzz target `proc_shebang_resolve`: shebang parsing in executable resolution.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-15. W0 stub: accepts every input until WP-15 drives `snp_proc` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_proc as _;

fuzz_target!(|_data: &[u8]| {});
