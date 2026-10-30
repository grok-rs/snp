//! Fuzz target `pycompat_decode_utf8`: CPython's UTF-8 decoder (strict, `backslashreplace`,
//! `surrogateescape`) and the text helpers that consume decoded or raw bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks;
//! the strict decoder agrees with `std::str::from_utf8` and reports CPython's maximal invalid
//! subpart; the surrogateescape view re-encodes to the input.
//!
//! Owner: WP-11.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pycompat::chars::PyChar;
use snp_pycompat::{codec, eaw, int, repr, text, textio};

fuzz_target!(|data: &[u8]| {
    match (codec::decode_utf8(data), std::str::from_utf8(data)) {
        (Ok(a), Ok(b)) => {
            assert_eq!(a, b);
            let _ = repr::repr_str(a);
            let _ = eaw::len_cjk(a);
            let _ = int::parse_int(a).map_err(|e| e.to_string());
            let _ = int::parse_version(a);
            let u = textio::universal_newlines(a);
            assert!(!u.contains('\r'));
            let _ = textio::windows_text_write(a);
            let _ = textio::textwrap_indent(a, "    ");
            assert_eq!(text::strip_fsbytes(data), text::strip(a).as_bytes());
            assert_eq!(repr::repr_fsbytes(data), repr::repr_str(a));
        }
        (Err(e), Err(std_err)) => {
            assert_eq!(e.start, std_err.valid_up_to());
            assert!(e.start < e.end && e.end <= data.len());
            assert_eq!(Some(&e.first_byte), data.get(e.start));
            assert_eq!(e.py_type(), "UnicodeDecodeError");
            let msg = e.to_string();
            assert!(msg.starts_with("'utf-8' codec can't decode byte"), "{msg}");
        }
        (a, b) => panic!("{data:?}: {a:?} vs {b:?}"),
    }
    let br = codec::decode_utf8_backslashreplace(data);
    assert!(br.len() >= data.len() || std::str::from_utf8(data).is_ok());
    let mut back = Vec::new();
    for c in codec::fsdecode_chars(data) {
        match c {
            PyChar::Scalar(c) => {
                let mut buf = [0_u8; 4];
                back.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            PyChar::Surrogate(u) => {
                assert!((0xdc80..=0xdcff).contains(&u));
                back.push(u8::try_from(u - 0xdc00).unwrap());
            }
        }
    }
    assert_eq!(back, data);
    let stripped = text::strip_fsbytes(data);
    assert!(stripped.len() <= data.len());
    let _ = repr::repr_fsbytes(data);
});
