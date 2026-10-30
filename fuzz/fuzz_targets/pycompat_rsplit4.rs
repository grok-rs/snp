//! Fuzz target `pycompat_rsplit4`: `a, b, c, d = line.rsplit(maxsplit=3)` of pre-push lines
//! (`hook_impl.py:128-130`), str and bytes, and the other line/record splitters fed by git.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks;
//! `rsplit4` equals `rsplit_whitespace(_, Some(3))`; splitlines with `keepends` reproduces the
//! input in both directions; `zsplit` rejoined with NUL is the NUL-stripped input.
//!
//! Owner: WP-11.
#![no_main]

use std::ffi::OsStr;

use libfuzzer_sys::fuzz_target;
use snp_pycompat::{precommit, text};

fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    match text::rsplit4(&s) {
        Ok(f) => assert_eq!(f.to_vec(), text::rsplit_whitespace(&s, Some(3))),
        Err(e) => {
            assert!(e.got < 4);
            assert_eq!(e.got, text::rsplit_whitespace(&s, None).len());
            assert!(e.to_string().starts_with("not enough values to unpack"));
        }
    }
    match text::bytes_rsplit4(data) {
        Ok(f) => assert_eq!(f.to_vec(), text::bytes_rsplit_whitespace(data, Some(3))),
        Err(e) => assert_eq!(e.got, text::bytes_rsplit_whitespace(data, None).len()),
    }
    for (i, line) in precommit::pre_push_lines(&s).enumerate() {
        if line.is_err() || i > 64 {
            break;
        }
    }
    for line in precommit::pre_push_lines_bytes(data).take(64) {
        if line.is_err() {
            break;
        }
    }
    let keep: Vec<&str> = text::splitlines_keepends(&s).collect();
    assert_eq!(keep.concat(), s);
    let mut back: Vec<&str> = text::splitlines_keepends(&s).rev().collect();
    back.reverse();
    assert_eq!(back, keep);
    assert_eq!(text::splitlines(&s).count(), keep.len());
    let bkeep: Vec<&[u8]> = text::bytes_splitlines_keepends(data).collect();
    assert_eq!(bkeep.concat(), data);
    let mut bback: Vec<&[u8]> = text::bytes_splitlines_keepends(data).rev().collect();
    bback.reverse();
    assert_eq!(bback, bkeep);
    let records: Vec<&[u8]> = precommit::zsplit(data).collect();
    let start = data.iter().take_while(|&&c| c == 0).count();
    let end = data.len() - data.iter().rev().take_while(|&&c| c == 0).count();
    let core = if start < end {
        &data[start..end]
    } else {
        &data[..0]
    };
    assert_eq!(records.join(&0_u8), core);
    for line in precommit::merge_msg_conflicts(data) {
        assert_eq!(text::bytes_strip(line), line);
    }
    let _ = precommit::get_skips(Some(OsStr::new(&*s)));
    let _: Vec<&str> = text::split_whitespace(&s, Some(1)).collect();
});
