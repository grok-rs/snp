//! `snp migrate builtin|upstream` on the test corpus (09 §7.1, WP-34 acceptance): the builtin ⇄ upstream round trip is
//! lossless (byte for byte) in both directions, both rewrites are idempotent and keep the config loadable, and what
//! `to_builtin` leaves alone is exactly what `repo: builtin` cannot run.
//!
//! Owner: WP-34.

#[cfg(test)]
mod builtin {
    use std::path::Path;

    use snp_migrate::{to_builtin, to_upstream};

    /// The configs of `tests/fixtures/builtin/<name>`, split at `---8<---` lines (the file's header comment stays with
    /// the first config).
    fn corpus(name: &str) -> Vec<String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/builtin")
            .join(name);
        let text = std::fs::read_to_string(path).unwrap();
        let v: Vec<String> = text.split("---8<---\n").map(str::to_owned).collect();
        assert!(v.len() >= 4, "{name}: {}", v.len());
        v
    }

    fn b(s: &str) -> String {
        String::from_utf8(to_builtin(s.as_bytes()).unwrap()).unwrap()
    }

    fn u(s: &str) -> String {
        String::from_utf8(to_upstream(s.as_bytes()).unwrap()).unwrap()
    }

    /// The PyYAML-safe value of `s` (its `Debug` rendering compares two configs structurally).
    fn value(s: &str) -> String {
        format!(
            "{:?}",
            snp_yaml::load(s, &snp_yaml::Limits::default()).unwrap()
        )
    }

    /// The `repo:` values of `s`, in order (one item per line, block or flow).
    fn repos(s: &str) -> Vec<String> {
        s.lines()
            .filter_map(|l| {
                let t = l.trim_start().trim_start_matches(['-', ' ', '{']);
                t.strip_prefix("repo: ")
            })
            .map(|r| {
                r.split([' ', ','])
                    .next()
                    .unwrap_or("")
                    .trim_matches('\'')
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn upstream_round_trip_is_byte_exact() {
        let mut converted = 0;
        for (i, src) in corpus("upstream.yaml").iter().enumerate() {
            let built = b(src);
            assert_eq!(u(&built), *src, "config {i}:\n{built}");
            assert_eq!(b(&built), built, "config {i}: to_builtin is idempotent");
            value(&built);
            converted += built.matches("repo: builtin").count();
        }
        // Config 0: 1, config 1: 1, config 2: 2 (black stays), config 3: 1 (the check-ast block stays),
        // config 4: 0, config 5: 0 (two items on one line), config 6: 1, config 7: 1 (anchored `repo` / `rev`
        // values stay).
        assert_eq!(converted, 7);
    }

    /// ADR-084: a pre-commit-hooks block left upstream is explained (ids without a builtin, keys that change what
    /// runs); a convertible block and other repos are not mentioned.
    #[test]
    fn why_a_block_stays_upstream() {
        let cfg = "repos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v6.0.0\n    hooks:\n    \
                   -   id: trailing-whitespace\n    -   id: debug-statements\n    -   id: double-quote-string-fixer\n\
                   -   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v6.0.0\n    hooks:\n    \
                   -   id: check-yaml\n        language: system\n\
                   -   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v6.0.0\n    hooks:\n    \
                   -   id: end-of-file-fixer\n\
                   -   repo: local\n    hooks:\n    -   {id: x, name: x, entry: x, language: system}\n";
        assert_eq!(
            snp_migrate::builtin_not_converted(cfg.as_bytes()).unwrap(),
            [
                "pre-commit-hooks (line 2) not converted: debug-statements, double-quote-string-fixer have no \
                 builtin",
                "pre-commit-hooks (line 8) not converted: hook check-yaml sets `language`",
            ]
        );
        assert_eq!(b(cfg).matches("repo: builtin").count(), 1);
    }

    #[test]
    fn builtin_round_trip_is_byte_exact() {
        for (i, src) in corpus("builtin.yaml").iter().enumerate() {
            let up = u(src);
            assert!(!up.contains("repo: builtin"), "config {i}:\n{up}");
            assert_eq!(b(&up), *src, "config {i}:\n{up}");
            assert_eq!(u(&up), up, "config {i}: to_upstream is idempotent");
            value(&up);
        }
    }

    #[test]
    fn what_stays_upstream() {
        let corpus = corpus("upstream.yaml");
        // config 3: check-ast is no builtin; config 4: additional_dependencies, a language, no hooks.
        let built = b(&corpus[3]);
        assert_eq!(
            repos(&built),
            ["builtin", "https://github.com/pre-commit/pre-commit-hooks"]
        );
        assert_eq!(b(&corpus[4]), corpus[4]);
        // config 7: a `repo` or `rev` with an anchor is left alone (removing it would undefine `*u` / `*r`).
        let built = b(&corpus[7]);
        assert_eq!(
            repos(&built),
            [
                "&u",
                "https://github.com/pre-commit/pre-commit-hooks",
                "builtin",
                "local"
            ]
        );
        value(&built);
        assert_eq!(b(&corpus[5]), corpus[5]);
        let built = b(&corpus[2]);
        assert_eq!(
            repos(&built),
            ["builtin", "https://github.com/psf/black", "builtin"]
        );
        assert!(
            built.contains("# was: https://github.com/pre-commit/pre-commit-hooks.git@'v5.0.0'\n")
        );
        assert!(built.contains("# was: git@github.com:pre-commit/pre-commit-hooks@\"v4.6.0\"\n"));
    }

    #[test]
    fn upstream_without_marker_uses_the_newest_tag() {
        let up = u("repos:\n- repo: builtin\n  hooks:\n  - id: check-json\n    language: system\n");
        assert_eq!(
            up,
            "repos:\n# was: builtin\n- repo: https://github.com/pre-commit/pre-commit-hooks\n  rev: v6.0.0\n  hooks:\n  - id: check-json\n"
        );
        // builtin's default language is system: the structural value round-trips without it.
        assert_eq!(
            value(&b(&up)),
            value("repos:\n- repo: builtin\n  hooks:\n  - id: check-json\n")
        );
    }

    #[test]
    fn refused_inputs_fail_with_exit_1() {
        for bad in [
            &b"\xff\xfe"[..],
            b"repos: [\n",
            b"repos:\r- repo: builtin\r  hooks: []\r",
            "repos:\u{2028}- repo: builtin\n".as_bytes(),
        ] {
            for f in [to_builtin, to_upstream] {
                match f(bad) {
                    Err(snp_core::failure::Failure::SystemExit { code: 1, stderr }) => {
                        assert!(stderr.starts_with(b"snp migrate: "), "{stderr:?}");
                    }
                    other => panic!("{bad:?}: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn multi_line_scalars_stay_upstream() {
        let src = "repos:\n- repo: \"https://github.com/pre-commit/\\\n    pre-commit-hooks\"\n  rev: v6.0.0\n  hooks:\n  - id: check-json\n";
        assert_eq!(b(src), src);
    }

    /// One generated upstream config: `(text, number of convertible blocks)`.
    fn generated() -> impl proptest::strategy::Strategy<Value = (String, usize)> {
        use std::fmt::Write as _;

        use proptest::prelude::*;
        let block = (
            prop_oneof![
                Just("https://github.com/pre-commit/pre-commit-hooks"),
                Just("https://github.com/pre-commit/pre-commit-hooks.git"),
                Just("git@github.com:pre-commit/pre-commit-hooks"),
                Just("https://github.com/someone/else"),
            ],
            prop_oneof![Just("v4.5.0"), Just("'v6.0.0'"), Just("\"v5.0.0\"")],
            proptest::option::of(Just("  # frozen: abc")),
            proptest::collection::vec(
                prop_oneof![
                    Just("check-json"),
                    Just("trailing-whitespace"),
                    Just("check-ast"),
                    Just("end-of-file-fixer"),
                ],
                1..4,
            ),
            any::<bool>(),
        );
        (
            prop_oneof![
                Just(("-   ", "    ")),
                Just(("  - ", "    ")),
                Just(("- ", "  "))
            ],
            any::<bool>(),
            proptest::collection::vec(block, 1..4),
        )
            .prop_map(|((dash, ind), crlf, blocks)| {
                let mut s = String::from("repos:\n");
                let mut n = 0;
                for (url, rev, comment, ids, with_args) in blocks {
                    write!(
                        s,
                        "{dash}repo: {url}\n{ind}rev: {rev}{}\n{ind}hooks:\n",
                        comment.unwrap_or("")
                    )
                    .unwrap();
                    for id in &ids {
                        writeln!(s, "{ind}{dash}id: {id}").unwrap();
                        if with_args {
                            writeln!(s, "{ind}{ind}args: [--x]").unwrap();
                        }
                    }
                    if !url.contains("someone") && !ids.contains(&"check-ast") {
                        n += 1;
                    }
                }
                if crlf {
                    s = s.replace('\n', "\r\n");
                }
                (s, n)
            })
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]
        /// Generated upstream configs: every convertible block becomes builtin and the way back is byte-exact.
        #[test]
        fn generated_round_trip((src, n) in generated()) {
            let built = b(&src);
            proptest::prop_assert_eq!(built.matches("repo: builtin").count(), n);
            proptest::prop_assert_eq!(u(&built), src);
        }
    }
}
