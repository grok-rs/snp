//! Fuzz target `pch_p0_check_json`: the native `check-json` checks (epoch 1: plain `json.load`; epoch 2:
//! duplicate keys) on arbitrary bytes, with the message the hook prints.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-26.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pyparsers::json::{self, PyMinor};

fuzz_target!(|data: &[u8]| {
    for result in [
        json::check_v3(data, PyMinor::P313),
        json::check_ex(data, PyMinor::P313),
    ] {
        if let Err(e) = result {
            let line = format!("x: Failed to json decode ({e})");
            assert!(line.len() > 26);
        }
    }
});
