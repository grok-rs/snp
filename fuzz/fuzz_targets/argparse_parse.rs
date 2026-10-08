//! Fuzz target `argparse_parse`: argparse emulation over both specs (`PRE_COMMIT_SPEC`, `SNP_SPEC`).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-12. W0 stub: accepts every input until WP-12 drives `snp_argparse`/`snp_cli` with it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_argparse as _;
use snp_cli as _;

fuzz_target!(|_data: &[u8]| {});
