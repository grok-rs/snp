//! Fuzz target `git_pre_push`: pre-push stdin parsing.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-18. W0 stub: accepts every input until WP-18 drives `snp_git` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_git as _;

fuzz_target!(|_data: &[u8]| {});
