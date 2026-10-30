//! Fuzz target `pyparsers_xml`: expat well-formedness.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-17.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pyparsers::json::PyMinor;
use snp_pyparsers::xml;

fuzz_target!(|data: &[u8]| {
    drop(xml::check(data));
    drop(xml::check_ex(data, "f.xml", PyMinor::P312));
});
