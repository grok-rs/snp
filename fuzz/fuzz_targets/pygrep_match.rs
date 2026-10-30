//! Fuzz target `pygrep_match`: native pygrep over arbitrary file bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input is `flags pattern NUL content`: the flag byte selects `-i`, `--multiline` and `--negate`; the pattern
//! is prepared once (an invalid one is the one-line error) and run over the content written to a file, in all
//! four matching procedures; the output always ends with a newline when it is not empty.
//!
//! Owner: WP-26.
#![no_main]

use std::ffi::OsStr;
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use snp_core::cancel::CancelToken;
use snp_hooks::pygrep;
use snp_pyre::RegexCache;

fuzz_target!(|data: &[u8]| {
    let Some((&flags, rest)) = data.split_first() else {
        return;
    };
    let (pattern, content) = match rest.iter().position(|&b| b == 0) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, &b""[..]),
    };
    let pattern = String::from_utf8_lossy(pattern).into_owned();
    let mut args: Vec<Arc<str>> = Vec::new();
    for (bit, flag) in [(1, "-i"), (2, "--multiline"), (4, "--negate")] {
        if flags & bit != 0 {
            args.push(Arc::from(flag));
        }
    }
    let Ok(compiled) = pygrep::prepare(&args, &pattern, 80) else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("snp-fuzz-pygrep-{}", std::process::id()));
    if std::fs::create_dir_all(&dir).is_err() || std::fs::write(dir.join("f"), content).is_err() {
        return;
    }
    let mut argv: Vec<&OsStr> = args.iter().map(|a| OsStr::new(&**a)).collect();
    argv.push(OsStr::new(&pattern));
    argv.push(OsStr::new("f"));
    let o = pygrep::run_partition(
        &compiled,
        &argv,
        &dir,
        &RegexCache::new(),
        &CancelToken::new(),
    );
    assert!(o.code == 0 || o.code == 1 || o.code == 2);
    assert!(o.output.is_empty() || o.output.ends_with(b"\n"));
});
