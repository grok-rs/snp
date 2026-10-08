//! Fuzz target `pch_p1_file_contents_sorter`: the native `file-contents-sorter` on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-27. W0 stub: accepts every input until WP-27 drives `snp_hooks` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_hooks as _;

fuzz_target!(|_data: &[u8]| {});
