//! Fuzz target `git_hook_script_classify`: the hook-script classifier.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input is a hook script; `hookscripts::classify` (11 §A.5.1) reads it from a scratch file. Checked: no panic,
//! a regular file always classifies, the rank order holds (snp's id wins over the compat ids it carries) and
//! `is_manager`/`owner_name` agree.
//!
//! Owner: WP-18.
#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use snp_git::hookscripts::{Owner, SNP_SCRIPT_ID, classify};

fn scratch() -> &'static PathBuf {
    static FILE: OnceLock<PathBuf> = OnceLock::new();
    FILE.get_or_init(|| std::env::temp_dir().join(format!("snp-fuzz-hook-script-{}", std::process::id())))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fuzz_target!(|data: &[u8]| {
    let path = scratch();
    std::fs::write(path, data).expect("write the script");
    let owner = classify(path).expect("a regular file always classifies");
    assert_ne!(owner, Owner::Missing);
    assert_ne!(owner, Owner::NotAFile);
    if contains(data, SNP_SCRIPT_ID.as_bytes()) {
        assert_eq!(owner, Owner::Snp { stale: false });
    }
    assert_eq!(owner.is_manager(), owner.owner_name().is_some());
});
