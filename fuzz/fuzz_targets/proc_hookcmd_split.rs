//! Fuzz target `proc_hookcmd_split`: hook `entry` splitting.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input is `entry \0 arg \0 arg …` (lossy UTF-8). `hook_cmd` must never panic; on success the argv is
//! non-empty, ends with the args verbatim, and starts with `[self_exe, "__hazmat"]` exactly when the first two shlex
//! tokens of the entry are `pre-commit`/`snp` and `hazmat`.
//!
//! Owner: WP-15.
#![no_main]

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use snp_proc::hookcmd::{HookCmdError, hook_cmd};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let mut parts = text.split('\0');
    let entry = parts.next().unwrap_or_default();
    let args: Vec<Arc<str>> = parts.map(Arc::from).collect();
    let exe = Path::new("/opt/snp/bin/snp");
    match hook_cmd(entry, &args, exe) {
        Ok(argv) => {
            assert!(!argv.is_empty());
            let tail = argv.len().checked_sub(args.len()).unwrap_or(0);
            let got_args: Vec<OsString> = argv.get(tail..).unwrap_or_default().to_vec();
            let want_args: Vec<OsString> = args.iter().map(|a| OsString::from(&**a)).collect();
            assert_eq!(got_args, want_args);
            let tokens = snp_pycompat::shlex::split(entry).unwrap_or_default();
            let hazmat = tokens.len() >= 2
                && tokens.get(1).is_some_and(|t| t == "hazmat")
                && tokens
                    .first()
                    .is_some_and(|t| t == "pre-commit" || t == "snp");
            if hazmat {
                assert_eq!(argv.first(), Some(&exe.as_os_str().to_os_string()));
                assert_eq!(argv.get(1), Some(&OsString::from("__hazmat")));
            }
        }
        Err(HookCmdError::Empty) => assert!(args.is_empty()),
        Err(_) => {}
    }
});
