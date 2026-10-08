//! Fuzz target `pycompat_shlex_split`: `shlex.split`.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-11. W0 stub: accepts every input until WP-11 drives `snp_pycompat` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pycompat as _;

fuzz_target!(|_data: &[u8]| {});
