//! Fuzz target `pch_p0_check_executables_have_shebangs`: the native message (`shlex.quote` of an arbitrary path)
//! and argparse on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Owner: WP-26.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels;

fuzz_target!(|data: &[u8]| {
    for epoch in [1, 4, 5] {
        let m = kernels::shebang_message(data, epoch);
        assert!(m.starts_with(data));
        assert!(m.ends_with(b"double-check its shebang.\n"));
    }
    let argv: Vec<String> = data
        .split(|&b| b == 0)
        .take(16)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let env = snp_core::env::Env::from_pairs(Vec::<(String, String)>::new());
    drop(kernels::parse_console_script(
        "check-executables-have-shebangs",
        5,
        &argv,
        &env,
    ));
});
