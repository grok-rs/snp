//! Fuzz target `pch_p0_check_merge_conflict`: the native `check-merge-conflict` scan on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Every epoch's pattern set (and the builtin `--diff3`) scans the file; hits are in line order and every hit's
//! line really starts with its pattern.
//!
//! Owner: WP-26.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels;

fuzz_target!(|data: &[u8]| {
    let lines: Vec<&[u8]> = data.split_inclusive(|&b| b == b'\n').collect();
    for (epoch, diff3) in [(1, false), (3, false), (3, true)] {
        let patterns = kernels::merge_conflict_patterns(epoch, diff3);
        let hits = kernels::merge_conflicts(data, &patterns);
        let mut last = 0;
        for (lineno, pattern) in hits {
            assert!(lineno >= last && lineno >= 1);
            last = lineno;
            assert!(lines[lineno - 1].starts_with(pattern));
        }
    }
});
