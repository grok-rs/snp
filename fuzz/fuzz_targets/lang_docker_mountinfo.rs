//! Fuzz target `lang_docker_mountinfo`: `/proc/self/mountinfo` parsing.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-25. W0 stub: accepts every input until WP-25 drives `snp_lang` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_lang as _;

fuzz_target!(|_data: &[u8]| {});
