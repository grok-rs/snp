//! Fuzz target `config_revline`: the autoupdate rev-line rewrite.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The first byte picks the updates (repo index, rev shape, freeze); the rest is the config text. A line rewrite
//! keeps every non-rev line byte for byte (`Rewrite::Lines`), an update with the old rev is idempotent, and the
//! re-dump path only happens when the rev lines do not line up.
//!
//! Owner: WP-21.
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use snp_config::revline::{RevUpdate, Rewrite, rewrite};

const REVS: [&str; 6] = [
    "v2.0.0",
    "1.0",
    "yes",
    "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
    "a b",
    "'q'",
];

fuzz_target!(|data: &[u8]| {
    let Some((&sel, text)) = data.split_first() else {
        return;
    };
    let n = usize::from(sel & 0x3);
    let updates: Vec<RevUpdate> = (0..n)
        .map(|i| RevUpdate {
            repo_index: i,
            new_rev: Arc::from(REVS[usize::from(sel >> 2) % REVS.len()]),
            frozen_comment: (sel & 0x80 != 0).then(|| Arc::from("v1.2.3")),
        })
        .collect();
    match rewrite(text, &updates) {
        Ok(Rewrite::Lines(out)) => {
            let out = std::str::from_utf8(&out).expect("UTF-8 output");
            let input = std::str::from_utf8(text).expect("input was UTF-8");
            let changed = input
                .split_inclusive('\n')
                .zip(out.split_inclusive('\n'))
                .filter(|(a, b)| a != b)
                .count();
            assert!(changed <= n, "only rev lines change");
            if updates.is_empty() {
                assert_eq!(out, input);
            }
        }
        Ok(Rewrite::FullReformat(out)) => {
            assert!(std::str::from_utf8(&out).is_ok());
        }
        Ok(_) | Err(_) => {}
    }
});
