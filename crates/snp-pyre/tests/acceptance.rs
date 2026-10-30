//! The named acceptance items of WBS WP-13 as explicit cases (the truth tables of `tests/vectors.rs` cover them
//! in bulk). Every expected text, position and answer was taken from CPython 3.12.3 (`re.compile`,
//! `str(e)`, `traceback.format_exception_only(e)[-1]`, `re.search`/`re.match`/`re.fullmatch`).
//!
//! Owner: WP-13.

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "test inputs built with fixed repeat counts"
)]
mod acceptance {
    use std::sync::Arc;

    use snp_pyre::{
        Haystack, MatchKind, Mode, Options, PyReError, PyRegex, RegexCache, cfgv_is_valid, validate,
    };

    const BYTES: Options = Options {
        mode: Mode::Bytes,
        haystack: Haystack::Line,
        ..Options::CONFIG
    };

    fn err(src: &str, mode: Mode) -> PyReError {
        validate(src, mode).expect_err(src)
    }

    #[test]
    fn nesting_484_accepted_485_recursion() {
        for mode in [Mode::Str, Mode::Bytes] {
            let ok = format!("{}a{}", "(".repeat(484), ")".repeat(484));
            assert!(validate(&ok, mode).is_ok());
            let re = PyRegex::compile(
                &ok,
                Options {
                    mode,
                    ..Options::CONFIG
                },
            )
            .unwrap();
            assert_eq!(re.search("xay"), Ok(true));
            let deep = format!("{}a{}", "(".repeat(485), ")".repeat(485));
            assert_eq!(validate(&deep, mode), Err(PyReError::Recursion));
            assert_eq!(
                err(&deep, mode).py_last_line(),
                "RecursionError: maximum recursion depth exceeded"
            );
        }
    }

    #[test]
    fn overflow_value_and_rejected_constructs() {
        for mode in [Mode::Str, Mode::Bytes] {
            let e = err("a{4294967295}", mode);
            assert_eq!(e, PyReError::Overflow);
            assert_eq!(
                e.py_last_line(),
                "OverflowError: the repetition number is too large"
            );
            assert!(validate("a{4294967294}", mode).is_ok());
            assert_eq!(err(r"\z", mode).to_string(), r"bad escape \z at position 0");
            assert_eq!(
                err(r"a\z", mode).to_string(),
                r"bad escape \z at position 1"
            );
            assert_eq!(
                err("(?<n>x)", mode).to_string(),
                "unknown extension ?<n at position 1"
            );
        }
        let e = err("(?a)(?u)x", Mode::Str);
        assert_eq!(
            e,
            PyReError::Value("ASCII and UNICODE flags are incompatible".into())
        );
        assert_eq!(
            e.py_last_line(),
            "ValueError: ASCII and UNICODE flags are incompatible"
        );
        assert_eq!(
            err("(?a)(?L)x", Mode::Bytes).py_last_line(),
            "ValueError: ASCII and LOCALE flags are incompatible"
        );
        assert_eq!(
            err("(?u)x", Mode::Bytes).py_last_line(),
            "re.error: bad inline flags: cannot use 'u' flag with a bytes pattern at position 3"
        );
    }

    #[test]
    fn messages_positions_and_line_columns() {
        let e = err("(", Mode::Bytes);
        assert_eq!(e.py_type(), "re.error");
        assert_eq!(
            e.py_last_line(),
            "re.error: missing ), unterminated subpattern at position 0"
        );
        assert_eq!(
            err("a)", Mode::Str).to_string(),
            "unbalanced parenthesis at position 1"
        );
        let e = err("x\n(?P<1>y)", Mode::Str);
        assert_eq!(
            e.to_string(),
            "bad character in group name '1' at position 6 (line 2, column 5)"
        );
        assert!(matches!(e, PyReError::Re { pos: Some(6), .. }));
        assert_eq!(
            err("(?<n>x)\n", Mode::Str).to_string(),
            "unknown extension ?<n at position 1 (line 1, column 2)"
        );
        // Positions count characters for str patterns.
        assert_eq!(
            err("é)", Mode::Str).to_string(),
            "unbalanced parenthesis at position 1"
        );
    }

    #[test]
    fn future_warnings_in_order() {
        let w = validate("[[b]", Mode::Str).unwrap();
        assert_eq!(
            w.iter().map(|w| &*w.msg).collect::<Vec<_>>(),
            ["Possible nested set at position 1"]
        );
        let re = PyRegex::compile("x[a&&b]|[a||b]", Options::CONFIG).unwrap();
        assert_eq!(
            re.warnings().iter().map(|w| &*w.msg).collect::<Vec<_>>(),
            [
                "Possible set intersection at position 3",
                "Possible set union at position 10"
            ]
        );
    }

    #[test]
    fn cfgv_bool_hook_rejects_every_python_exception() {
        assert!(cfgv_is_valid(r"^src/.*\.py$"));
        for bad in ["(", "a{4294967295}", "(?a)(?u)x", r"\z"] {
            assert!(!cfgv_is_valid(bad), "{bad}");
        }
        assert!(!cfgv_is_valid(&format!(
            "{}a{}",
            "(".repeat(485),
            ")".repeat(485)
        )));
    }

    #[test]
    fn python_match_semantics() {
        let s = |p: &str| PyRegex::compile(p, Options::CONFIG).unwrap();
        // `$` matches before a trailing newline (search), not for fullmatch.
        assert_eq!(s("a$").search("a\n"), Ok(true));
        assert_eq!(s("a$").fullmatch("a\n"), Ok(false));
        assert_eq!(s("x+$").search("xx\n"), Ok(true));
        // `^$` on "" and "\n".
        assert_eq!(s("^$").kind(), MatchKind::EmptyOrNewline);
        assert_eq!(s("^$").search(""), Ok(true));
        assert_eq!(s("^$").search("\n"), Ok(true));
        assert_eq!(s("^$").search("a"), Ok(false));
        // \b / \B on non-ASCII and empty haystacks.
        assert_eq!(s(r"\bé").search("é"), Ok(true));
        assert_eq!(s(r"\B").search(""), Ok(false));
        assert_eq!(s(r"a\B").search("aé"), Ok(true));
        // Unicode IGNORECASE: simple folding, the Turkish-i rule, no full case folding.
        assert_eq!(s("(?i)ǅ").search("ǆ"), Ok(true));
        assert_eq!(s("(?i)\u{130}").search("i"), Ok(true));
        assert_eq!(s("(?i)i").search("\u{130}"), Ok(true));
        assert_eq!(s("(?i)straße").search("STRASSE"), Ok(false));
        // re.match anchors at 0 only.
        assert_eq!(s("b").match_prefix("ab"), Ok(false));
        assert_eq!(s("a").match_prefix("ab"), Ok(true));
    }

    #[test]
    fn bytes_spans() {
        let re = PyRegex::compile(r"\s+$", BYTES).unwrap();
        assert_eq!(re.search_bytes(b"x  \n"), Ok(Some((1, 4))));
        let re = PyRegex::compile(r"\xff+", BYTES).unwrap();
        assert_eq!(re.search_bytes(b"a\xff\xffb"), Ok(Some((1, 3))));
        // `\w` is ASCII on bytes patterns.
        let re = PyRegex::compile(r"\w+", BYTES).unwrap();
        assert_eq!(re.search_bytes("é ab".as_bytes()), Ok(Some((3, 5))));
    }

    #[test]
    fn cache_shares_compiles_per_source_and_options() {
        let cache = RegexCache::new();
        let a = cache.get(r"\.py$", Options::CONFIG).unwrap();
        assert!(Arc::ptr_eq(
            &a,
            &cache.get(r"\.py$", Options::CONFIG).unwrap()
        ));
        let ci = Options {
            ignore_case: true,
            ..Options::CONFIG
        };
        assert!(!Arc::ptr_eq(&a, &cache.get(r"\.py$", ci).unwrap()));
        assert_eq!(
            cache.get("(", Options::CONFIG).unwrap_err().py_type(),
            "re.error"
        );
    }

    #[test]
    fn unsupported_is_reported_not_panicked() {
        // D5 (01 Addendum §A.6.13): valid Python beyond the engines' size budget.
        assert!(validate("a{100000}", Mode::Str).is_ok());
        let e = PyRegex::compile("a{100000}", Options::CONFIG).unwrap_err();
        assert!(matches!(e, PyReError::Unsupported(_)));
        assert_eq!(e.py_type(), "snp");
    }
}
