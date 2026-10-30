//! Golden texts of every message in `src/messages.rs` (ARCHITECTURE §1.3 "Messages"). snp-yaml's messages never
//! name the tool, so the compat and snp brands print the same bytes; PyYAML/CPython texts were produced by the
//! oracle (PyYAML 6.0.3 `CSafeLoader`, CPython 3.12.3), the `limit` texts are snp's own (D-SEC-11).
//!
//! Owner: WP-14.

#[cfg(test)]
mod messages {
    use snp_yaml::{
        Document, Limits, Mark, Node, NodeId, NodeKind, ScalarStyle, Schema, construct, load,
    };

    /// `s` repeated `n` times (`str::repeat` is a disallowed method).
    fn rep(s: &str, n: usize) -> String {
        std::iter::repeat_n(s, n).collect()
    }

    fn err(text: &str) -> (String, String) {
        match load(text, &Limits::default()) {
            Ok(v) => panic!("{text:?} loaded as {v:?}"),
            Err(e) => (e.py_type().to_owned(), e.to_string()),
        }
    }

    fn check(cases: &[(&str, &str, &str)]) {
        for (text, class, message) in cases {
            assert_eq!(
                err(text),
                ((*class).to_owned(), (*message).to_owned()),
                "{text:?}"
            );
        }
    }

    #[test]
    fn constructor_messages() {
        check(&[
            (
                "a: !!omap {a: 1}\n",
                "ConstructorError",
                "while constructing an ordered map\nexpected a sequence, but found mapping\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "a: !!pairs a\n",
                "ConstructorError",
                "while constructing pairs\nexpected a sequence, but found scalar\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "a: !!pairs [a]\n",
                "ConstructorError",
                "while constructing pairs\n  in \"<unicode string>\", line 1, column 4\nexpected a mapping of length 1, but found scalar\n  in \"<unicode string>\", line 1, column 13",
            ),
            (
                "a: !!pairs [{a: 1, b: 2}]\n",
                "ConstructorError",
                "while constructing pairs\n  in \"<unicode string>\", line 1, column 4\nexpected a single mapping item, but found 2 items\n  in \"<unicode string>\", line 1, column 13",
            ),
            (
                "a: !!omap [{}]\n",
                "ConstructorError",
                "while constructing an ordered map\n  in \"<unicode string>\", line 1, column 4\nexpected a single mapping item, but found 0 items\n  in \"<unicode string>\", line 1, column 12",
            ),
            (
                "a: !!seq x\n",
                "ConstructorError",
                "expected a sequence node, but found scalar\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "a: !!map x\n",
                "ConstructorError",
                "expected a mapping node, but found scalar\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "a: !!null {a: 1}\n",
                "ConstructorError",
                "expected a scalar node, but found mapping\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "a: !e%C3%A9 x\n",
                "ConstructorError",
                "could not determine a constructor for the tag '!eé'\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "a: !!binary 'xé日 b'\n",
                "ConstructorError",
                "failed to convert base64 data into ascii: 'ascii' codec can't encode characters in position 1-2: ordinal not in range(128)\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "a: !!binary '\u{1f600}'\n",
                "ConstructorError",
                "failed to convert base64 data into ascii: 'ascii' codec can't encode character '\\U0001f600' in position 0: ordinal not in range(128)\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "a: !!binary 'é'\n",
                "ConstructorError",
                "failed to convert base64 data into ascii: 'ascii' codec can't encode character '\\xe9' in position 0: ordinal not in range(128)\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "a: !!binary 'aGk'\n",
                "ConstructorError",
                "failed to decode base64 data: Incorrect padding\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "a: !!binary 'a'\n",
                "ConstructorError",
                "failed to decode base64 data: Invalid base64-encoded string: number of data characters (1) cannot be 1 more than a multiple of 4\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "&a [*a]\n",
                "ConstructorError",
                "found unconstructable recursive node\n  in \"<unicode string>\", line 1, column 1",
            ),
        ]);
    }

    #[test]
    fn python_exception_messages() {
        let long = rep("x", 300);
        let truncated: String = format!("'{long}").chars().take(200).collect();
        let many = rep("9", 4400);
        let parts = vec!["1"; 180].join(":");
        let cases: Vec<(String, &str, String)> = vec![
        ("a: !!int ''\n".into(), "IndexError", "string index out of range".into()),
        (
            "a: !!timestamp x\n".into(),
            "AttributeError",
            "'NoneType' object has no attribute 'groupdict'".into(),
        ),
        (
            "a: !!timestamp {=: 2020-01-01}\n".into(),
            "TypeError",
            "expected string or bytes-like object, got 'list'".into(),
        ),
        (
            format!("a: !!float \"{parts}\"\n"),
            "OverflowError",
            "int too large to convert to float".into(),
        ),
        (
            format!("a: !!int \"{long}\"\n"),
            "ValueError",
            format!("invalid literal for int() with base 10: {truncated}"),
        ),
        (
            format!("a: !!int \"1:{many}\"\n"),
            "ValueError",
            "Exceeds the limit (4300 digits) for integer string conversion: value has 4400 digits; use \
             sys.set_int_max_str_digits() to increase the limit"
                .into(),
        ),
        (
            "a: !!float 'x y'\n".into(),
            "ValueError",
            "could not convert string to float: 'x y'".into(),
        ),
        ("a: !!bool maybe\n".into(), "KeyError", "'maybe'".into()),
        (
            "a: !e%F5%80%80%80 x\n".into(),
            "UnicodeDecodeError",
            "'utf-8' codec can't decode byte 0xf5 in position 2: invalid start byte".into(),
        ),
        (
            "a: !e%ED%A0%80 x\n".into(),
            "UnicodeDecodeError",
            "'utf-8' codec can't decode byte 0xed in position 2: invalid continuation byte".into(),
        ),
    ];
        for (text, class, message) in &cases {
            assert_eq!(
                err(text),
                ((*class).to_owned(), message.clone()),
                "{text:?}"
            );
        }
    }

    /// snp's resource-limit texts (D-SEC-11); class `YAMLError` (PyYAML's base class).
    #[test]
    fn limit_messages_d_sec_11() {
        let tiny = Limits {
            max_depth: 2,
            max_constructed_nodes: 3,
            max_bytes: 8,
        };
        let e = load("123456789", &tiny).expect_err("bytes");
        assert_eq!(e.py_type(), "YAMLError");
        assert_eq!(
            e.to_string(),
            "the input is 9 bytes long, more than the limit of 8 bytes"
        );
        let e = load("[[[]]]", &tiny).expect_err("depth");
        assert_eq!(
            e.to_string(),
            "found a node nested deeper than the limit of 2 levels\n  in \"<unicode string>\", line 1, column 3"
        );
        let e = load("[1,2,3]", &tiny).expect_err("nodes");
        assert_eq!(
            e.to_string(),
            "found more than the limit of 3 nodes (aliases count at every use)\n  in \"<unicode string>\", line 1, column 6"
        );
    }

    /// A hand-built `Document` with a dangling node id.
    #[test]
    fn invalid_document_message() {
        let start = Mark {
            index: 0,
            line: 0,
            column: 0,
        };
        let doc = Document {
            nodes: vec![Node {
                kind: NodeKind::Seq(vec![NodeId(9)]),
                tag: "tag:yaml.org,2002:seq".into(),
                style: ScalarStyle::NotScalar,
                span: 0..0,
                start,
                anchor: None,
            }],
            root: Some(NodeId(0)),
        };
        let e = construct(&doc, Schema::PyYaml11Safe, &Limits::default()).expect_err("dangling id");
        assert_eq!(
            e.to_string(),
            "found an invalid node reference in the document"
        );
    }

    /// `MarkedYAMLError.__str__` omits the context mark when it equals the problem mark.
    #[test]
    fn context_mark_is_omitted_when_equal() {
        assert_eq!(
            err("a: !!omap {a: 1}\n").1.lines().count(),
            3,
            "context, problem, one mark"
        );
    }
}
