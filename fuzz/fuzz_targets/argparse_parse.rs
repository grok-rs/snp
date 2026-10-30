//! Fuzz target `argparse_parse`: argparse emulation over both specs (`PRE_COMMIT_SPEC`, `SNP_SPEC`).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Input layout: byte 0 selects the brand and whether help is also rendered; the rest is split on NUL into
//! `COLUMNS`, `PRE_COMMIT_COLOR` and the argv items (raw bytes on Unix, so non-UTF-8 argv is exercised). The
//! target drives `snp_cli::parse` (the full `main.py` front end: `[]` → `run`, `help` re-parse, typed facade),
//! `snp_argparse::parse` directly, the help/usage formatter at the derived width and `textwrap`.
//!
//! Owner: WP-12.
#![no_main]

use std::ffi::OsString;

use libfuzzer_sys::fuzz_target;
use snp_argparse::ParseCfg;
use snp_core::brand::Brand;
use snp_core::env::Env;

fn os(bytes: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        OsString::from_vec(bytes.to_vec())
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let mut parts = rest.split(|&b| b == 0);
    let columns = parts.next().unwrap_or_default();
    let color = parts.next().unwrap_or_default();
    // Bound the argv length: argparse is linear, but huge inputs only slow the fuzzer down.
    let argv: Vec<OsString> = parts.take(512).map(os).collect();
    let mut env_pairs = vec![(OsString::from("COLUMNS"), os(columns))];
    if mode & 2 != 0 {
        env_pairs.push((OsString::from("PRE_COMMIT_COLOR"), os(color)));
    }
    let env = Env::from_pairs(env_pairs);
    let brand = if mode & 1 == 0 {
        Brand::PreCommit
    } else {
        Brand::Snp
    };
    let tty = (mode & 4 != 0).then_some(usize::from(mode >> 4));
    let _ = snp_cli::parse(brand, &argv, &env, tty);

    let spec = if mode & 1 == 0 {
        snp_cli::PRE_COMMIT_SPEC
    } else {
        snp_cli::SNP_SPEC
    };
    let cfg = ParseCfg {
        prog: brand.prog(),
        env: &env,
        // Keep pathological widths cheap to render while covering negative formatter widths (COLUMNS=1).
        columns: snp_argparse::columns(&env, tty).min(4096),
    };
    let _ = snp_argparse::parse(spec, &argv, &cfg);
    if mode & 8 != 0 {
        let path: Vec<&str> = argv.iter().take(3).filter_map(|a| a.to_str()).collect();
        let _ = snp_argparse::format_help(spec, &path, &cfg);
        let _ = snp_argparse::format_usage(spec, &path, &cfg);
        if let Some(text) = argv.first().and_then(|a| a.to_str()) {
            let width = usize::from(mode >> 4);
            let _ = snp_argparse::textwrap::wrap(text, width);
            let _ = snp_argparse::textwrap::fill(text, width, "  ", "    ");
        }
    }
});
