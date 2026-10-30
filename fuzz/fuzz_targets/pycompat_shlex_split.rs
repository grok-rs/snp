//! Fuzz target `pycompat_shlex_split`: `shlex.split` (`lang_base.py:175` `hook_cmd`) and its
//! quoting inverses.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks;
//! `split(quote(s)) == [s]`; the tokens of a successful split survive `join` + `split`.
//!
//! Owner: WP-11.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_pycompat::shlex;

fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    match shlex::split(&s) {
        Ok(tokens) => {
            let joined = shlex::join(&tokens);
            assert_eq!(shlex::split(&joined).as_ref(), Ok(&tokens), "{s:?}");
        }
        Err(e) => {
            assert_eq!(e.py_type(), "ValueError");
            assert!(!e.to_string().is_empty());
        }
    }
    let quoted = shlex::quote(&s);
    assert_eq!(
        shlex::split(&quoted),
        Ok(vec![s.clone().into_owned()]),
        "{s:?}"
    );
    // the bytes quoting never alters the bytes inside the quotes
    let qb = shlex::quote_bytes(data);
    if std::str::from_utf8(data).is_ok() {
        assert_eq!(&*qb, quoted.as_bytes());
    }
});
