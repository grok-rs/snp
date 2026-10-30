//! Fuzz target `pyparsers_toml`: `tomllib`.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-17.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pyparsers::json::PyMinor;
use snp_pyparsers::toml;

fuzz_target!(|data: &[u8]| {
    drop(toml::check(data));
    drop(toml::check_ex(data, PyMinor::P312));
});
