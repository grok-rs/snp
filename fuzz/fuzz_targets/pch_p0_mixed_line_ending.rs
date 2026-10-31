//! Fuzz target `pch_p0_mixed_line_ending`: the native `mixed-line-ending` core on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Every `--fix` mode runs; a rewrite leaves a single ending kind, so fixing it again reports nothing.
//!
//! Owner: WP-26.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels::{self, LineFix};

fuzz_target!(|data: &[u8]| {
    for fix in [
        LineFix::Auto,
        LineFix::No,
        LineFix::Cr,
        LineFix::Crlf,
        LineFix::Lf,
    ] {
        let (ret, new) = kernels::mixed_line_ending(data, fix);
        if let Some(new) = new {
            assert!(ret);
            assert_eq!(kernels::mixed_line_ending(&new, LineFix::No), (false, None));
        }
    }
});
