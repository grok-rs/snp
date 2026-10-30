//! `snp_pyparsers::xml` against expat 2.6.1 as `check-xml` drives it through `xml.sax` (WBS WP-17 acceptance:
//! the W3C XML conformance suite, the 09 §4.C catalogue, encodings and mutants; identical under CPython 3.12.3
//! and 3.13).
//!
//! Owner: WP-17.

use std::fmt::Write as _;

use snp_pyparsers::json::PyMinor;
use snp_pyparsers::xml;

use super::support::{Failures, got, input, rep, vectors, want};

#[test]
fn check_matches_expat() {
    let data = vectors("xml.json");
    let mut f = Failures::default();
    let mut n = 0usize;
    for case in data["cases"].as_array().unwrap() {
        let bytes = input(case);
        for v in [PyMinor::P312, PyMinor::P313] {
            let name = format!("{} {v:?}", case["name"].as_str().unwrap());
            f.check(
                &name,
                &got(&xml::check_ex(&bytes, "f.xml", v)),
                &want(&case["result"]),
            );
            n += 1;
        }
    }
    assert!(n >= 7000, "{n} cases");
    f.finish("xml::check");
}

/// Deep element nesting and long entity chains (in content, in attribute values, in parameter entities) run on a
/// 256 KiB thread: expansion is iterative (ARCHITECTURE §13.5 fuzz stacks).
#[test]
fn deep_inputs_fit_small_stacks() {
    let n = 3000usize;
    let mut chain = String::from("<!DOCTYPE a [");
    let mut pchain = String::from("<!DOCTYPE a [");
    for i in 0..n {
        write!(chain, "<!ENTITY e{i} \"&e{};\">", i + 1).unwrap();
        write!(pchain, "<!ENTITY % p{} \"%p{};\">", n - i, n - i + 1).unwrap();
    }
    write!(chain, "<!ENTITY e{n} \"x\">").unwrap();
    let docs = vec![
        rep("<a>", 200_000) + &rep("</a>", 200_000),
        format!("{chain}]><a>&e0;</a>"),
        format!("{chain}]><a b=\"&e0;\"/>"),
        format!("{pchain}]><a/>"),
        format!(
            "<!DOCTYPE a [<!ELEMENT a {}b{}>]><a/>",
            rep("(", 50_000),
            rep(")", 50_000)
        ),
    ];
    let h = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            for d in &docs {
                drop(xml::check_ex(d.as_bytes(), "f.xml", PyMinor::P313));
            }
        })
        .unwrap();
    h.join().unwrap();
}

#[test]
fn catalogue_09() {
    for (input, want) in [
        (&b""[..], "f.xml:1:0: no element found"),
        (b"<a></b>", "f.xml:1:5: mismatched tag"),
        (b"<a/><b/>", "f.xml:1:4: junk after document element"),
        (b"<a b=\"1\" b=\"2\"/>", "f.xml:1:9: duplicate attribute"),
        (
            b"\n<?xml version=\"1.0\"?><a/>",
            "f.xml:2:0: XML or text declaration not at start of entity",
        ),
        (
            "<a>\u{e9}\u{e9}\u{e9}<b></a>".as_bytes(),
            "f.xml:1:11: mismatched tag",
        ),
        ("<a>\u{1f600}</b>".as_bytes(), "f.xml:1:6: mismatched tag"),
    ] {
        assert_eq!(
            xml::check_ex(input, "f.xml", PyMinor::P313).map_err(|e| e.to_string()),
            Err(want.to_owned()),
            "{input:?}"
        );
    }
    assert_eq!(xml::check(b"<y:c/>"), Ok(()));
    assert_eq!(
        xml::check(b"<a"),
        Err("<unknown>:1:0: unclosed token".to_owned())
    );
    assert!(xml::EXPAT_VERSION.starts_with("expat_2."));
}
