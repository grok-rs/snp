//! WP-14 acceptance (docs/WBS.md "WP-14"): the verified scanner/parser texts of 07 §3.1.4 and the C-loader table of
//! 01 §12.7, the 07 §3.1.5 dump lines and the 03 §11.3 re-dump, duplicate and merge keys with Python key equality,
//! stream rules, the YAML 1.2 core switch, and the D-SEC-11 limits. Expected texts were produced by the oracle
//! (PyYAML 6.0.3 `CSafeLoader`/`CSafeDumper`, libyaml 0.2.5).
//!
//! Owner: WP-14.

#[cfg(test)]
mod acceptance {
    use cfgv::{Map, Value, value_map};
    use snp_yaml::emit::{Quote, dump, scalar_for_rev};
    use snp_yaml::{
        Document, Limits, Mark, Node, NodeId, NodeKind, ScalarStyle, Schema, compose, compose_all,
        construct, load,
    };

    fn load_default(text: &str) -> Result<Value, snp_yaml::YamlError> {
        load(text, &Limits::default())
    }

    /// `s` repeated `n` times (`str::repeat` is a disallowed method).
    fn rep(s: &str, n: usize) -> String {
        std::iter::repeat_n(s, n).collect()
    }

    fn err(text: &str) -> (String, String) {
        match load_default(text) {
            Ok(v) => panic!("{text:?} loaded as {v:?}"),
            Err(e) => (e.py_type().to_owned(), e.to_string()),
        }
    }

    fn value(text: &str) -> Value {
        load_default(text).unwrap_or_else(|e| panic!("{text:?}: {e}"))
    }

    /// 07 §3.1.4: the scanner and parser error categories (tab indentation, unclosed flow sequences and mappings, bad
    /// indent, unterminated quoted scalars, the missing `-` indicator, `@` and backtick, `a: b: c`, an unknown
    /// directive, a bad escape) byte-identical to `str(exc)` of the C loader.
    #[test]
    fn scanner_and_parser_errors_07_3_1_4() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "repos:\n\t- repo: local\n",
                "ScannerError",
                "while scanning for the next token\nfound character that cannot start any token\n  in \"<unicode string>\", line 2, column 1",
            ),
            (
                "\ta: 1\n",
                "ScannerError",
                "while scanning for the next token\nfound character that cannot start any token\n  in \"<unicode string>\", line 1, column 1",
            ),
            (
                "repos: [\n",
                "ParserError",
                "while parsing a flow node\ndid not find expected node content\n  in \"<unicode string>\", line 2, column 1",
            ),
            (
                "a: [1, 2",
                "ParserError",
                "while parsing a flow sequence\n  in \"<unicode string>\", line 1, column 4\ndid not find expected ',' or ']'\n  in \"<unicode string>\", line 2, column 1",
            ),
            (
                "hooks: {id: x, name: y\n",
                "ParserError",
                "while parsing a flow mapping\n  in \"<unicode string>\", line 1, column 8\ndid not find expected ',' or '}'\n  in \"<unicode string>\", line 2, column 1",
            ),
            (
                "a:\n  - b\n c: d\n",
                "ParserError",
                "while parsing a block mapping\n  in \"<unicode string>\", line 1, column 1\ndid not find expected key\n  in \"<unicode string>\", line 3, column 2",
            ),
            (
                "a:\n  b: 1\n   c: 2\n",
                "ScannerError",
                "mapping values are not allowed in this context\n  in \"<unicode string>\", line 3, column 5",
            ),
            (
                "a: 'x",
                "ScannerError",
                "while scanning a quoted scalar\n  in \"<unicode string>\", line 1, column 4\nfound unexpected end of stream\n  in \"<unicode string>\", line 1, column 6",
            ),
            (
                "a: \"x\n",
                "ScannerError",
                "while scanning a quoted scalar\n  in \"<unicode string>\", line 1, column 4\nfound unexpected end of stream\n  in \"<unicode string>\", line 2, column 1",
            ),
            (
                "- a\nb: c\n",
                "ParserError",
                "while parsing a block collection\n  in \"<unicode string>\", line 1, column 1\ndid not find expected '-' indicator\n  in \"<unicode string>\", line 2, column 1",
            ),
            (
                "key: @x\n",
                "ScannerError",
                "while scanning for the next token\nfound character that cannot start any token\n  in \"<unicode string>\", line 1, column 6",
            ),
            (
                "key: `x\n",
                "ScannerError",
                "while scanning for the next token\nfound character that cannot start any token\n  in \"<unicode string>\", line 1, column 6",
            ),
            (
                "a: b: c\n",
                "ScannerError",
                "mapping values are not allowed in this context\n  in \"<unicode string>\", line 1, column 5",
            ),
            (
                "%FOO bar\n---\na: 1\n",
                "ScannerError",
                "while scanning a directive\n  in \"<unicode string>\", line 1, column 1\nfound unknown directive name\n  in \"<unicode string>\", line 1, column 5",
            ),
            (
                "a: \"\\q\"\n",
                "ScannerError",
                "while parsing a quoted scalar\n  in \"<unicode string>\", line 1, column 4\nfound unknown escape character\n  in \"<unicode string>\", line 1, column 5",
            ),
            (
                "repos:\n- repo: x\n  hooks: [{id: a}\n",
                "ParserError",
                "while parsing a flow sequence\n  in \"<unicode string>\", line 3, column 10\ndid not find expected ',' or ']'\n  in \"<unicode string>\", line 4, column 1",
            ),
        ];
        assert!(cases.len() >= 14);
        for (text, class, message) in cases {
            assert_eq!(
                err(text),
                ((*class).to_owned(), (*message).to_owned()),
                "{text:?}"
            );
        }
    }

    /// 01 §12.7 / 07 §3.1.4: composer and constructor texts.
    #[test]
    fn composer_and_constructor_errors_01_12_7() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "a: *undefined\n",
                "ComposerError",
                "found undefined alias\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "a: &x 1\nb: &x 2\n",
                "ComposerError",
                "found duplicate anchor; first occurrence\n  in \"<unicode string>\", line 1, column 4\nsecond occurrence\n  in \"<unicode string>\", line 2, column 4",
            ),
            (
                "a: 1\n---\nb: 2\n",
                "ComposerError",
                "expected a single document in the stream\n  in \"<unicode string>\", line 1, column 1\nbut found another document\n  in \"<unicode string>\", line 2, column 1",
            ),
            (
                "---\n  a: 1\n---\nb: 2\n",
                "ComposerError",
                "expected a single document in the stream\n  in \"<unicode string>\", line 2, column 3\nbut found another document\n  in \"<unicode string>\", line 3, column 1",
            ),
            (
                "a: !foo bar\n",
                "ConstructorError",
                "could not determine a constructor for the tag '!foo'\n  in \"<unicode string>\", line 1, column 4",
            ),
            (
                "args: [=]\n",
                "ConstructorError",
                "could not determine a constructor for the tag 'tag:yaml.org,2002:value'\n  in \"<unicode string>\", line 1, column 8",
            ),
            (
                "? [1, 2]\n: x\n",
                "ConstructorError",
                "while constructing a mapping\n  in \"<unicode string>\", line 1, column 1\nfound unhashable key\n  in \"<unicode string>\", line 1, column 3",
            ),
            (
                "repos: []\nfoo: {<<: 5}\n",
                "ConstructorError",
                "while constructing a mapping\n  in \"<unicode string>\", line 2, column 6\nexpected a mapping or list of mappings for merging, but found scalar\n  in \"<unicode string>\", line 2, column 11",
            ),
            (
                "foo: {<<: [5]}\n",
                "ConstructorError",
                "while constructing a mapping\n  in \"<unicode string>\", line 1, column 6\nexpected a mapping for merging, but found scalar\n  in \"<unicode string>\", line 1, column 12",
            ),
            ("a: !!bool maybe\n", "KeyError", "'maybe'"),
            ("rev: 2020-13-45\n", "ValueError", "month must be in 1..12"),
            (
                "a: 0b_\n",
                "ValueError",
                "invalid literal for int() with base 2: ''",
            ),
            (
                "a: !!timestamp x\n",
                "AttributeError",
                "'NoneType' object has no attribute 'groupdict'",
            ),
            (
                "a: \u{0}\n",
                "ReaderError",
                "unacceptable character #x0000: control characters are not allowed\n  in \"<unicode string>\", position 3",
            ),
        ];
        for (text, class, message) in cases {
            assert_eq!(
                err(text),
                ((*class).to_owned(), (*message).to_owned()),
                "{text:?}"
            );
        }
    }

    /// 07 §3.1.5: the rev re-dump of 37 strings in the three quote styles, and the nested try-repo-like config.
    #[test]
    fn dump_lines_07_3_1_5() {
        let table: &[[&str; 4]] = &[
            ["v1.2.3", "v1.2.3", "'v1.2.3'", "\"v1.2.3\""],
            ["1.0", "'1.0'", "'1.0'", "\"1.0\""],
            ["1.10", "'1.10'", "'1.10'", "\"1.10\""],
            ["2", "'2'", "'2'", "\"2\""],
            [
                "2020-01-01",
                "'2020-01-01'",
                "'2020-01-01'",
                "\"2020-01-01\"",
            ],
            ["yes", "'yes'", "'yes'", "\"yes\""],
            ["on", "'on'", "'on'", "\"on\""],
            ["True", "'True'", "'True'", "\"True\""],
            ["null", "'null'", "'null'", "\"null\""],
            ["~", "'~'", "'~'", "\"~\""],
            ["0777", "'0777'", "'0777'", "\"0777\""],
            ["0x1F", "'0x1F'", "'0x1F'", "\"0x1F\""],
            ["1_000", "'1_000'", "'1_000'", "\"1_000\""],
            ["12:30", "'12:30'", "'12:30'", "\"12:30\""],
            [".inf", "'.inf'", "'.inf'", "\".inf\""],
            ["", "''", "''", "\"\""],
            ["\u{e9}", "\"\\xE9\"", "\"\\xE9\"", "\"\\xE9\""],
            ["#hash", "'#hash'", "'#hash'", "\"#hash\""],
            ["a #b", "'a #b'", "'a #b'", "\"a #b\""],
            ["abc#def", "abc#def", "'abc#def'", "\"abc#def\""],
            ["x\"y'z", "x\"y'z", "'x\"y''z'", "\"x\\\"y'z\""],
            ["'q", "'''q'", "'''q'", "\"'q\""],
            ["\"dq", "'\"dq'", "'\"dq'", "\"\\\"dq\""],
            [" lead", "' lead'", "' lead'", "\" lead\""],
            ["trail ", "'trail '", "'trail '", "\"trail \""],
            ["a: b", "'a: b'", "'a: b'", "\"a: b\""],
            ["@x", "'@x'", "'@x'", "\"@x\""],
            ["*x", "'*x'", "'*x'", "\"*x\""],
            ["!x", "'!x'", "'!x'", "\"!x\""],
            ["%x", "'%x'", "'%x'", "\"%x\""],
            ["&x", "'&x'", "'&x'", "\"&x\""],
            ["[x]", "'[x]'", "'[x]'", "\"[x]\""],
            ["-x", "-x", "'-x'", "\"-x\""],
            ["1e3", "1e3", "'1e3'", "\"1e3\""],
            ["y", "y", "'y'", "\"y\""],
            ["deadbeef", "deadbeef", "'deadbeef'", "\"deadbeef\""],
            [
                "aa658a0cb064bf927c7cbd4c3034e4d876de3c9c",
                "aa658a0cb064bf927c7cbd4c3034e4d876de3c9c",
                "'aa658a0cb064bf927c7cbd4c3034e4d876de3c9c'",
                "\"aa658a0cb064bf927c7cbd4c3034e4d876de3c9c\"",
            ],
        ];
        assert_eq!(table.len(), 37);
        for [rev, plain, single, double] in table {
            assert_eq!(scalar_for_rev(rev, Quote::Plain), *plain, "{rev:?}");
            assert_eq!(scalar_for_rev(rev, Quote::Single), *single, "{rev:?}");
            assert_eq!(scalar_for_rev(rev, Quote::Double), *double, "{rev:?}");
        }
        let hook2 = value_map! {
            "id" => "\u{e9}",
            "args" => vec![Value::from("--max=1"), Value::from("yes"), Value::from("")],
            "always_run" => true,
            "n" => 3,
        };
        let cfg = value_map! {
            "repos" => vec![value_map! {
                "repo" => "/tmp/x y",
                "rev" => "1.0",
                "hooks" => vec![value_map! {"id" => "flake8"}, hook2],
            }],
        };
        assert_eq!(
            dump(&cfg),
            "repos:\n-   repo: /tmp/x y\n    rev: '1.0'\n    hooks:\n    -   id: flake8\n    -   id: \"\\xE9\"\n        args:\n        - --max=1\n        - 'yes'\n        - ''\n        always_run: true\n        n: 3\n"
        );
    }

    /// 03 §11.3: autoupdate's fallback `yaml_dump(yaml_load(original))` (comments dropped, flow style normalised).
    #[test]
    fn autoupdate_redump_03_11_3() {
        let original = "# keep me\n\
                    repos: [{repo: /path/hooks1, rev: v1.1.0, hooks: [{id: h1}]},  # c\n        \
                    {repo: local, hooks: [{id: x, name: x, entry: x, language: system}]}]\n";
        assert_eq!(
            dump(&value(original)),
            "repos:\n-   repo: /path/hooks1\n    rev: v1.1.0\n    hooks:\n    -   id: h1\n-   repo: local\n    hooks:\n    -   id: x\n        name: x\n        entry: x\n        language: system\n"
        );
    }

    /// 01 §12.4: duplicate keys are accepted, the last value wins at the first key's position, and Python key
    /// equality merges `1`, `1.0` and `True` (keeping the first key object).
    #[test]
    fn duplicate_keys_and_python_key_equality() {
        assert_eq!(
            value("{a: 1, b: 2, a: 3}\n"),
            value_map! {"a" => 3, "b" => 2}
        );
        let v = value("{1: a, 1.0: b, true: c}\n");
        let m = v.as_map().expect("map");
        assert_eq!(m.len(), 1);
        let (k, val) = m.first().expect("entry");
        assert!(
            matches!(k, Value::Int(1)),
            "the first key object is kept: {k:?}"
        );
        assert_eq!(val, &Value::from("c"));
        let v = value("{0: a, false: b, 0.0: c, -0.0: d}\n");
        assert_eq!(v.as_map().map(Map::len), Some(1));
        assert_eq!(v.repr(), "{0: 'd'}");
        assert_eq!(value("repos: [a]\nrepos: [b]\n").repr(), "{'repos': ['b']}");
        // A NaN is one shared object in PyYAML, so `.nan` keys collapse too.
        assert_eq!(value("{.nan: 1, .nan: 2}\n").repr(), "{nan: 2}");
    }

    /// 01 §12.4: merge keys (`<<`), mappings and lists of mappings, explicit keys win, merged pairs first.
    #[test]
    fn merge_keys() {
        assert_eq!(
            value("c: {<<: [{x: 1}, {x: 2, z: 3}], w: 0}\n").repr(),
            "{'c': {'x': 1, 'z': 3, 'w': 0}}"
        );
        assert_eq!(
            value("b: &b {x: 1, y: 2}\nc:\n  <<: *b\n  x: 3\n").repr(),
            "{'b': {'x': 1, 'y': 2}, 'c': {'x': 3, 'y': 2}}"
        );
        assert_eq!(
            value("x: &x {repo: local}\nrepos:\n- <<: *x\n  hooks: []\n").repr(),
            "{'x': {'repo': 'local'}, 'repos': [{'repo': 'local', 'hooks': []}]}"
        );
        // The merge pair is deleted before the merged mapping is flattened: a self-merge is empty.
        assert_eq!(value("&a {<<: *a}\n").repr(), "{}");
        // `=` keys become strings.
        assert_eq!(value("{=: 1}\n").repr(), "{'=': 1}");
    }

    #[test]
    fn stream_rules_01_12_6() {
        for empty in ["", "# only a comment\n", "---\n", "--- # c\n...\n"] {
            assert_eq!(value(empty), Value::Null, "{empty:?}");
        }
        assert_eq!(value("\u{feff}a: 1\n").repr(), "{'a': 1}");
        assert_eq!(value("a: b #c\n").repr(), "{'a': 'b'}");
        assert_eq!(value("a: b#c\n").repr(), "{'a': 'b#c'}");
        let flow_key = value(concat!("{", "a:1}\n"));
        assert_eq!(flow_key.repr(), concat!("{", "'a:1': None}"));
        assert_eq!(value("[a:1]\n").repr(), "['a:1']");
        assert_eq!(value("%YAML 1.2\n---\na: yes\n").repr(), "{'a': True}");
        assert_eq!(value("a: \u{85}\n").repr(), "{'a': None}");
        assert_eq!(value("a: 'x\n  y'\n").repr(), "{'a': 'x y'}");
    }

    #[test]
    fn exotic_values_q12() {
        assert_eq!(
            value("d: 2020-01-01\nt: 2001-12-14 21:59:43.10 -5\n").repr(),
            "{'d': datetime.date(2020, 1, 1), 't': datetime.datetime(2001, 12, 14, 21, 59, 43, 100000, \
         tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=68400)))}"
        );
        assert_eq!(value("b: !!binary aGk=\n").repr(), "{'b': b'hi'}");
        assert_eq!(
            value("p: !!pairs [{a: 1}, {a: 2}]\n").repr(),
            "{'p': [('a', 1), ('a', 2)]}"
        );
        assert_eq!(
            value("i: 123456789012345678901234567890\n").repr(),
            "{'i': 123456789012345678901234567890}"
        );
        let set = value("s: !!set {a, b}\n");
        assert_eq!(
            set.as_map()
                .and_then(|m| m.get(&Value::from("s")))
                .map(Value::type_name),
            Some("set")
        );
    }

    /// `compose` keeps aliases as the anchor's node id, records anchors, styles, tags and spans.
    #[test]
    fn compose_shares_alias_ids_and_records_spans() {
        let text = "a: &s [commit]\nb: *s\nc: 'q'\n";
        let doc = compose(text, Schema::PyYaml11Safe, &Limits::default()).expect("compose");
        let root = doc.root.expect("root");
        let NodeKind::Map(pairs) = &doc.nodes[usize::try_from(root.0).unwrap()].kind else {
            panic!("root is a mapping");
        };
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0].1, pairs[1].1, "the alias is the anchor's node");
        let seq = &doc.nodes[usize::try_from(pairs[0].1.0).unwrap()];
        assert_eq!(seq.anchor.as_deref(), Some("s"));
        assert_eq!(text.get(seq.span.clone()).unwrap(), "&s [commit]");
        assert_eq!(seq.style, ScalarStyle::NotScalar);
        let q = &doc.nodes[usize::try_from(pairs[2].1.0).unwrap()];
        assert_eq!(q.style, ScalarStyle::Single);
        assert_eq!(text.get(q.span.clone()).unwrap(), "'q'");
        assert_eq!(&*q.tag, "tag:yaml.org,2002:str");
        assert_eq!(
            q.start,
            Mark {
                index: 24,
                line: 2,
                column: 3
            }
        );
    }

    #[test]
    fn compose_all_reads_every_document() {
        let docs = compose_all(
            "a: &x 1\n---\nb: &x 2\n...\n--- [*y]\n",
            Schema::PyYaml11Safe,
            &Limits::default(),
        );
        let e = docs.expect_err("an undefined alias in the third document");
        assert_eq!(
            e.to_string(),
            "found undefined alias\n  in \"<unicode string>\", line 5, column 6"
        );
        let docs = compose_all(
            "a: &x 1\n---\nb: *x\n",
            Schema::PyYaml11Safe,
            &Limits::default(),
        );
        assert!(docs.is_err(), "anchors are per document");
        let docs = compose_all(
            "a: &x 1\n---\nb: &x 2\n",
            Schema::PyYaml11Safe,
            &Limits::default(),
        )
        .expect("two documents");
        assert_eq!(docs.len(), 2);
        assert!(
            compose_all("", Schema::PyYaml11Safe, &Limits::default())
                .expect("empty")
                .is_empty()
        );
    }

    /// The lefthook reader: YAML 1.2 core resolution through `construct`.
    #[test]
    fn yaml12_core_schema() {
        let lim = Limits::default();
        let doc = compose(
        "a: yes\nb: on\nc: true\nd: 0777\ne: 0o17\nf: 1e3\ng: 2020-01-01\nh: ~\ni: 1_000\nj: <<\n",
        Schema::Yaml12Core,
        &lim,
    )
    .expect("compose");
        let v = construct(&doc, Schema::Yaml12Core, &lim).expect("construct");
        assert_eq!(
            v.repr(),
            "{'a': 'yes', 'b': 'on', 'c': True, 'd': 777, 'e': 15, 'f': 1000.0, 'g': '2020-01-01', 'h': None, \
         'i': '1_000', 'j': '<<'}"
        );
    }

    /// `construct` on a hand-built `Document` never panics on dangling ids.
    #[test]
    fn hand_built_documents_are_checked() {
        let lim = Limits::default();
        let start = Mark {
            index: 0,
            line: 0,
            column: 0,
        };
        let node = |kind, tag: &str| Node {
            kind,
            tag: tag.into(),
            style: ScalarStyle::NotScalar,
            span: 0..0,
            start,
            anchor: None,
        };
        let dangling = Document {
            nodes: vec![node(
                NodeKind::Seq(vec![NodeId(7)]),
                "tag:yaml.org,2002:seq",
            )],
            root: Some(NodeId(0)),
        };
        assert!(construct(&dangling, Schema::PyYaml11Safe, &lim).is_err());
        let bad_root = Document {
            nodes: Vec::new(),
            root: Some(NodeId(3)),
        };
        assert!(construct(&bad_root, Schema::PyYaml11Safe, &lim).is_err());
        let empty = Document {
            nodes: Vec::new(),
            root: None,
        };
        assert_eq!(
            construct(&empty, Schema::PyYaml11Safe, &lim).expect("empty"),
            Value::Null
        );
        let cyclic = Document {
            nodes: vec![node(
                NodeKind::Seq(vec![NodeId(0)]),
                "tag:yaml.org,2002:seq",
            )],
            root: Some(NodeId(0)),
        };
        let e = construct(&cyclic, Schema::PyYaml11Safe, &lim).expect_err("cycle");
        assert_eq!(
            e.to_string(),
            "found unconstructable recursive node\n  in \"<unicode string>\", line 1, column 1"
        );
    }

    #[test]
    fn recursive_aliases_are_rejected_d_sec_11() {
        for text in ["&a [*a]\n", "&a {x: *a}\n", "a: &a {b: [*a]}\n"] {
            let (class, message) = err(text);
            assert_eq!(class, "ConstructorError", "{text:?}");
            assert!(
                message
                    .starts_with("found unconstructable recursive node\n  in \"<unicode string>\""),
                "{text:?}: {message}"
            );
        }
    }

    /// `SafeConstructor.construct_scalar` follows `=` keys; a chain is bounded like nesting, and a cycle (Python's
    /// `RecursionError`) is the depth limit (D-SEC-11).
    #[test]
    fn value_key_chains() {
        assert_eq!(value("a: !!str {=: {=: x}}\n").repr(), "{'a': 'x'}");
        assert_eq!(value("a: !!int {=: '12'}\n").repr(), "{'a': 12}");
        let (class, message) = err("a: &a !!str {=: *a}\n");
        assert_eq!(class, "YAMLError");
        assert_eq!(
            message,
            "found a node nested deeper than the limit of 512 levels\n  in \"<unicode string>\", line 1, column 4"
        );
    }

    /// A 10 × 10 `lol` alias bomb: PyYAML shares the objects (12,345,679,021 nodes when expanded); snp refuses it
    /// before copying anything.
    #[test]
    fn alias_bomb_is_a_limit_error_d_sec_11() {
        let (class, message) = err(&bomb(10));
        assert_eq!(class, "YAMLError");
        assert!(
            message.starts_with(
                "found more than the limit of 1000000 nodes (aliases count at every use)\n"
            ),
            "{message}"
        );
        // A small expansion is fine.
        let v = value(&bomb(3));
        assert_eq!(v.as_map().map(Map::len), Some(3));
    }

    /// The `lol` bomb with `levels` levels of 10 aliases each.
    fn bomb(levels: usize) -> String {
        let mut lines = vec![format!("a0: &a0 [{}]", ["lol"; 10].join(", "))];
        for i in 1..levels {
            let refs = vec![format!("*a{}", i - 1); 10].join(", ");
            lines.push(format!("a{i}: &a{i} [{refs}]"));
        }
        lines.join("\n") + "\n"
    }

    /// Nesting 10,000 deep is a clean YAML error on a 256 KiB thread stack (the composer, constructor and the
    /// parser are iterative).
    #[test]
    fn deep_nesting_is_a_limit_error_on_a_small_stack_d_sec_11() {
        let handle = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let mut results = Vec::new();
                for text in [
                    format!("{}{}\n", rep("[", 10_000), rep("]", 10_000)),
                    format!("{}1{}\n", rep("{a: ", 10_000), rep("}", 10_000)),
                    format!("{}x\n", rep("- ", 10_000)),
                ] {
                    results.push(load_default(&text).map(|_| ()).map_err(|e| e.to_string()));
                }
                results
            })
            .expect("spawn");
        for r in handle.join().expect("no stack overflow") {
            let e = r.expect_err("too deep");
            assert!(
            e.starts_with(
                "found a node nested deeper than the limit of 512 levels\n  in \"<unicode string>\""
            ),
            "{e}"
        );
        }
        // 512 levels are accepted, 513 are not.
        assert!(load_default(&format!("{}{}\n", rep("[", 512), rep("]", 512))).is_ok());
        assert!(load_default(&format!("{}{}\n", rep("[", 513), rep("]", 513))).is_err());
    }

    /// Depth reached through aliases counts too.
    #[test]
    fn alias_depth_counts() {
        let inner = format!("{}{}", rep("[", 300), rep("]", 300));
        let text = format!("a: &a {inner}\nb: {}*a{}\n", rep("[", 300), rep("]", 300));
        let (class, message) = err(&text);
        assert_eq!(class, "YAMLError");
        assert!(
            message.starts_with("found a node nested deeper than the limit of 512 levels"),
            "{message}"
        );
    }

    #[test]
    fn byte_and_node_limits() {
        let small = Limits {
            max_depth: 512,
            max_constructed_nodes: 10,
            max_bytes: 64,
        };
        let e = load(&rep("a", 65), &small).expect_err("too large");
        assert_eq!(
            e.to_string(),
            "the input is 65 bytes long, more than the limit of 64 bytes"
        );
        let e = load("[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]\n", &small).expect_err("too many nodes");
        assert_eq!(
            e.to_string(),
            "found more than the limit of 10 nodes (aliases count at every use)\n  in \"<unicode string>\", line 1, column 29"
        );
        let ok = load("[1, 2, 3, 4, 5, 6, 7, 8, 9]\n", &small).expect("ten nodes");
        assert_eq!(ok.as_list().map(<[Value]>::len), Some(9));
        // Merge bombs count the copied pairs.
        let merge_bomb = "a: &a {k0: 0, k1: 1, k2: 2}\nb: &b {<<: [*a, *a, *a, *a]}\nc: {<<: [*b, *b, *b, *b]}\n";
        assert!(load(merge_bomb, &small).is_err());
        assert!(load_default(merge_bomb).is_ok());
    }
}
