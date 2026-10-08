//! Fuzz target `yaml_load`: PyYAML `SafeLoader` load.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-14. W0 stub: accepts every input until WP-14 drives `snp_yaml` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_yaml as _;

fuzz_target!(|_data: &[u8]| {});
