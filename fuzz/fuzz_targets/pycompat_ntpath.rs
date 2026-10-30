//! Fuzz target `pycompat_ntpath`: `ntpath` normalisation, joining, splitting, case folding and
//! the relative-path family over UTF-8 and UTF-16 units (lone surrogates included), plus the
//! lexical `posixpath` functions on raw bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks;
//! `normpath` is idempotent (unless `..` uncovers a drive, as in CPython); UTF-8 and UTF-16
//! units give the same results on valid text.
//!
//! Owner: WP-11.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pycompat::{ntpath, posixpath};

fn exercise_nt<U: ntpath::Unit>(a: &[U], b: &[U], cwd: &[U], env: &dyn Fn(&str) -> Option<Vec<U>>) {
    let n = ntpath::normpath(a);
    // idempotent unless `..` uncovered a drive-like component (CPython: normpath('~x:\\..\\.:.')
    // is '.:.', whose own normpath is '.:')
    let (da, ra, _) = ntpath::splitroot(a);
    let (dn, rn, _) = ntpath::splitroot(&n);
    if da.len() == dn.len() && ra.len() == rn.len() {
        assert_eq!(ntpath::normpath(&n), n);
    }
    let _ = ntpath::splitroot(a);
    let (drive, rest) = ntpath::splitdrive(a);
    assert_eq!(drive.len() + rest.len(), a.len());
    let _ = ntpath::join(a, &[b]);
    let _ = ntpath::join(b, &[a, b]);
    let _ = ntpath::split(a);
    let _ = ntpath::dirname(a);
    let _ = ntpath::basename(a);
    let (root, ext) = ntpath::splitext(a);
    assert_eq!(root.len() + ext.len(), a.len());
    let _ = ntpath::normcase(a);
    let _ = ntpath::isabs_312(a);
    let _ = ntpath::isabs_313(a);
    let _ = ntpath::commonpath(&[a, b]);
    let _ = ntpath::abspath_lexical(a, cwd);
    let _ = ntpath::relpath(a, Some(b), cwd);
    let _ = ntpath::relpath(a, None, cwd);
    let _ = ntpath::expanduser(a, env);
}

fuzz_target!(|data: &[u8]| {
    let mid = data.len() / 2;
    let (a, b) = data.split_at(mid);
    let cwd8 = b"C:\\Users\\u\\repo\\sub";
    let env8 = |k: &str| match k {
        "USERPROFILE" => Some(b"C:\\Users\\u".to_vec()),
        "USERNAME" => Some(b"u".to_vec()),
        _ => None,
    };
    exercise_nt(a, b, cwd8, &env8);

    // UTF-16 units: raw pairs (lone surrogates included) and the transcoded text
    let wide = |x: &[u8]| -> Vec<u16> {
        x.chunks(2)
            .map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect()
    };
    let cwd16: Vec<u16> = "C:\\Users\\u\\repo\\sub".encode_utf16().collect();
    let env16 = |k: &str| match k {
        "USERPROFILE" => Some("C:\\Users\\u".encode_utf16().collect()),
        "HOMEPATH" => Some("\\home".encode_utf16().collect()),
        _ => None,
    };
    exercise_nt(&wide(a), &wide(b), &cwd16, &env16);
    if let (Ok(ta), Ok(tb)) = (std::str::from_utf8(a), std::str::from_utf8(b)) {
        let (wa, wb): (Vec<u16>, Vec<u16>) =
            (ta.encode_utf16().collect(), tb.encode_utf16().collect());
        let n8 = String::from_utf8(ntpath::normpath(a)).unwrap();
        let n16 = String::from_utf16(&ntpath::normpath(&wa)).unwrap();
        assert_eq!(n8, n16);
        let j8 = String::from_utf8(ntpath::join(a, &[b])).unwrap();
        let j16 = String::from_utf16(&ntpath::join(&wa, &[&wb])).unwrap();
        assert_eq!(j8, j16);
        let c8 = String::from_utf8(ntpath::normcase(a)).unwrap();
        let c16 = String::from_utf16(&ntpath::normcase(&wa)).unwrap();
        assert_eq!(c8, c16);
    }

    // posixpath on raw bytes
    let cwd = b"/home/u/repo/sub";
    let n = posixpath::normpath(a);
    assert_eq!(posixpath::normpath(&n), n);
    let _ = posixpath::join(a, &[b]);
    let _ = posixpath::split(a);
    let (root, ext) = posixpath::splitext(a);
    assert_eq!(root.len() + ext.len(), a.len());
    let abs = posixpath::abspath(a, cwd);
    assert!(posixpath::isabs(&abs));
    let _ = posixpath::relpath(a, Some(b), cwd);
    let _ = posixpath::commonpath(&[a, b]);
    let none = || None;
    let named = |_: &[u8]| Some(b"/home/other".to_vec());
    let _ = posixpath::expanduser(a, Some(b), &none, &named);
});
