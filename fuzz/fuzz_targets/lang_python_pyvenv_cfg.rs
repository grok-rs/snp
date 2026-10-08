//! Fuzz target `lang_python_pyvenv_cfg`: `pyvenv.cfg` parsing.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-24. W0 stub: accepts every input until WP-24 drives `snp_lang` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_lang as _;

fuzz_target!(|_data: &[u8]| {});
