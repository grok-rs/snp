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
        // Config patterns (str) at pre-commit's config-validation depth; bytes patterns at pygrep's
        // (`python -m pre_commit.languages.pygrep`, measured on CPython 3.12.3: 494 groups compile, 495 raise).
        for (mode, max) in [(Mode::Str, 484), (Mode::Bytes, 494)] {
            let ok = format!("{}a{}", "(".repeat(max), ")".repeat(max));
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
            let deep = format!("{}a{}", "(".repeat(max + 1), ")".repeat(max + 1));
            assert_eq!(validate(&deep, mode), Err(PyReError::Recursion));
            assert_eq!(
                err(&deep, mode).py_last_line(),
                "RecursionError: maximum recursion depth exceeded"
            );
        }
    }

    #[test]
    fn pygrep_nesting_of_conditionals_and_noncapturing_groups() {
        // Measured with pygrep's `main` on CPython 3.12.3 (str: config validation, `tests/vectors/nesting.json`).
        let cond = |n: usize| format!("(a){}a{}", "(?(1)".repeat(n), ")".repeat(n));
        let noncap = |n: usize| format!("{}a{}", "(?:".repeat(n), ")".repeat(n));
        for (mode, conds, groups) in [(Mode::Str, 969, 484), (Mode::Bytes, 988, 494)] {
            assert!(validate(&cond(conds), mode).is_ok(), "{mode:?} {conds}");
            assert_eq!(validate(&cond(conds + 1), mode), Err(PyReError::Recursion));
            assert!(validate(&noncap(groups), mode).is_ok());
            assert_eq!(
                validate(&noncap(groups + 1), mode),
                Err(PyReError::Recursion)
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

    const BLOB_MS: Options = Options {
        mode: Mode::Bytes,
        haystack: Haystack::Blob,
        ignore_case: false,
        multiline_dotall: true,
    };

    #[test]
    fn nullable_loop_bodies_end_like_max_until() {
        // CPython's `MAX_UNTIL` leaves a loop after an empty iteration, keeping that iteration's priority; the
        // span end is what `pygrep --multiline` prints (`pre_commit/languages/pygrep.py`).
        let blob = |p: &str, h: &[u8]| PyRegex::compile(p, BLOB_MS).unwrap().search_bytes(h);
        assert_eq!(blob("(?:x*|y)+", b"xyx"), Ok(Some((0, 1))));
        assert_eq!(blob("(?:$|.)*", b"a\nb"), Ok(Some((0, 1))));
        assert_eq!(blob("(?:x*|y)+z", b"xyxz"), Ok(Some((0, 4))));
        assert_eq!(blob("(?:x*|y){2,}", b"xyx"), Ok(Some((0, 1))));
        for haystack in [Haystack::Blob, Haystack::Line] {
            let re = PyRegex::compile("(?a)(?x:$|[^A-_\u{ff} x])*", Options { haystack, ..BYTES })
                .unwrap();
            assert_eq!(re.search_bytes(b"\r\n"), Ok(Some((0, 1))), "{haystack:?}");
        }
        let ci = Options {
            ignore_case: true,
            ..Options::CONFIG
        };
        let re = PyRegex::compile(r"(?a:|\S)+\w", ci).unwrap();
        assert_eq!(re.search_bytes(".aµA ".as_bytes()), Ok(Some((0, 2))));
        assert_eq!(re.fullmatch(".aµA "), Ok(false));
        // A nullable `{2,}` loop nested in another (fancy-regex's epsilon slot of an earlier entry).
        let re =
            PyRegex::compile(r"(?=[\s\S])(?:(?:(?:\n|x)|\b){2,}){2,}", Options::CONFIG).unwrap();
        assert_eq!(re.search_bytes(b"xy"), Ok(Some((0, 1))));
    }

    #[test]
    fn lazy_bodies_survive_fancy_regex_tree_rewrites() {
        // fancy-regex 0.19 rewrites `(X+?)*` to `(X+?)?` and `a*b??a*` to `a*(?:ba*)?`.
        let str_span = |p: &str, h: &str| {
            PyRegex::compile(p, Options::CONFIG)
                .unwrap()
                .search_bytes(h.as_bytes())
        };
        assert_eq!(str_span("(?=x)(x+?)*", "xxx"), Ok(Some((0, 3))));
        assert_eq!(str_span("(?<=a)(x+?)*", "axxx"), Ok(Some((1, 4))));
        assert_eq!(
            str_span("(?i)(?=sK{0,2})([^a]{1,}?)*", "ss"),
            Ok(Some((0, 2)))
        );
        assert_eq!(str_span("(?=a)a*b??a*", "aba"), Ok(Some((0, 1))));
        let re = PyRegex::compile(
            "(?=x)((x)+?)*",
            Options {
                multiline_dotall: false,
                ..BLOB_MS
            },
        )
        .unwrap();
        assert_eq!(re.search_bytes(b"xxxx"), Ok(Some((0, 4))));
    }

    #[test]
    fn scoped_ascii_leading_set_follows_the_search_charset_prefix() {
        // `_compile_info` compiles the leading set of `(?a:\W)` with the top-level (Unicode) flags, so
        // `re.search` skips the positions only the ASCII set matches; `re.match`/`re.fullmatch` do not.
        let s = |p: &str| PyRegex::compile(p, Options::CONFIG).unwrap();
        let re = s(r"(?a:\W)");
        assert_eq!(re.search("é"), Ok(false));
        assert_eq!(re.search("aé"), Ok(false));
        assert_eq!(re.match_prefix("é"), Ok(true));
        assert_eq!(re.fullmatch("é"), Ok(true));
        assert_eq!(re.search_bytes("é".as_bytes()), Ok(None));
        let re = s(r"(?a:[\SA-k])");
        assert_eq!(re.search_bytes("\u{85}a".as_bytes()), Ok(Some((2, 3))));
        assert_eq!(re.match_prefix("\u{85}a"), Ok(true));
        for p in [r"(?ims)(?:(?a:[\S]a))", r"(?a:\Wa|\Wb)"] {
            let h = if p.contains("ims") { "\u{85}a" } else { "éa" };
            assert_eq!(s(p).search(h), Ok(false), "{p}");
            assert_eq!(s(p).match_prefix(h), Ok(true), "{p}");
            assert_eq!(s(p).fullmatch(h), Ok(true), "{p}");
        }
        assert_eq!(s(r"(?a:\W|x)").search("é"), Ok(false));
        // A cased literal under IGNORECASE, a zero-width start or a repeat: no charset prefix.
        assert_eq!(s(r"(?i)(?a:[\Sa])").search("\u{85}a"), Ok(true));
        assert_eq!(s(r"(?a:\W)*x").search("éx"), Ok(true));
    }

    #[test]
    fn anchor_runs_are_charged_to_the_expansion_budget() {
        // Each Unicode `\b`/`\B` lowers to four look-arounds over `[\pL\pN_]`, each `$` to `(?=\n?\z)`.
        for p in [
            r"\B".repeat(5000),
            r"\b".repeat(10000),
            "$".repeat(30000),
            format!("(?=a){}", r"\b".repeat(10000)),
        ] {
            assert!(validate(&p, Mode::Str).is_ok());
            let e = PyRegex::compile(&p, Options::CONFIG).unwrap_err();
            assert!(matches!(e, PyReError::Unsupported(_)), "{e:?}");
        }
        let re = PyRegex::compile(&r"\B".repeat(100), Options::CONFIG).unwrap();
        assert_eq!(re.search(""), Ok(false));
        assert_eq!(re.search("é"), Ok(false));
        assert_eq!(re.search("ab"), Ok(true));
        // ASCII and bytes word boundaries stay cheap.
        assert!(PyRegex::compile(&r"\b".repeat(5000), BYTES).is_ok());
    }

    #[test]
    fn latin1_lane_spans() {
        let re = PyRegex::compile(r"(?<=\xff)a+", BYTES).unwrap();
        assert_eq!(re.kind(), MatchKind::Tier2);
        assert_eq!(re.search_bytes(b"\xff\xffaa"), Ok(Some((2, 4))));
        let re = PyRegex::compile("(?<=b)a+", BYTES).unwrap();
        assert_eq!(re.search_bytes(b"xbaa"), Ok(Some((2, 4))));
        let re = PyRegex::compile(r"(?<=\xe9)\w+", BYTES).unwrap();
        assert_eq!(re.search_bytes(b"\xe9ab"), Ok(Some((1, 3))));
    }

    #[test]
    fn documented_divergences_of_d_misc_06() {
        // D7: `\U` above 0x7FFFFFFF in a str pattern is CPython's `OverflowError: Python int too large to convert
        // to C int` (`chr` in `re/_parser.py`); snp reports the `bad escape` of the values up to 0x7FFFFFFF.
        let e = err(r"\U80000000", Mode::Str);
        assert_eq!(
            e.py_last_line(),
            r"re.error: bad escape \U80000000 at position 0"
        );
        assert_eq!(
            err(r"\U7FFFFFFF", Mode::Str).py_last_line(),
            r"re.error: bad escape \U7FFFFFFF at position 0"
        );
        // Bytes patterns have no `\U` escape: exact.
        assert_eq!(
            err(r"[\U80000000-\U90000000]", Mode::Bytes).py_last_line(),
            r"re.error: bad escape \U at position 1"
        );
        // D8: a conditional on a group of a bounded nullable loop observes the iteration after an empty one,
        // which CPython's `MAX_UNTIL` never runs (`re.fullmatch` is None in CPython 3.12.3).
        let re = PyRegex::compile(
            r"(?x)((a)?(?<!\n)(?(1)[a-z]|(?:i|^))){0,2}",
            Options::CONFIG,
        )
        .unwrap();
        assert_eq!(re.fullmatch("ab"), Ok(true));
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
