//! Fuzz target `pyparsers_yamlcheck`: the ruamel-compatible YAML checker.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The first byte picks the mode and the CPython minor; the rest is split at the first NUL-NUL pair into two files
//! checked by one `Checker`, so the state ruamel carries between files (leftover generators, `deep_construct`,
//! a leaked `%YAML 1.1`) is exercised too.
//!
//! Owner: WP-17.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pyparsers::json::PyMinor;
use snp_pyparsers::yamlcheck::{self, Checker, Mode};

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let mode = match sel % 3 {
        0 => Mode::Safe,
        1 => Mode::AllowMultipleDocuments,
        _ => Mode::Unsafe,
    };
    let minor = if sel & 0x80 == 0 {
        PyMinor::P313
    } else {
        PyMinor::P312
    };
    let split = rest.windows(2).position(|w| w == [0, 0]);
    let (first, second) = match split {
        Some(i) => (rest.get(..i).unwrap_or_default(), rest.get(i + 2..).unwrap_or_default()),
        None => (rest, &[][..]),
    };
    let mut c = Checker::new(mode, minor);
    drop(c.check_file(first, "a.yaml"));
    drop(c.check_file(second, "b.yaml"));
    if let Ok(text) = std::str::from_utf8(first) {
        drop(yamlcheck::check(text, mode));
    }
});
