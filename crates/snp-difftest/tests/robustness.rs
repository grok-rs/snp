//! Property tests of the harness's parsers and normalisers (WP-35; the harness is tooling, so these proptests stand in
//! for the fuzz targets of the shipped parsers): no input makes them panic, the normalisers are idempotent, and the
//! brand map's apply/invert round trip holds for texts assembled from its own phrases and arbitrary filler. The
//! resource cases feed alias chains ("billion laughs") and extreme `count:`/`timeout:` integers and require every
//! parse to finish quickly; difftest.yml runs this binary under a memory cap, so an unbounded expansion fails CI
//! instead of the machine.
//!
//! Owner: WP-35.

#[cfg(test)]
mod robustness {
    use std::fmt::Write as _;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use proptest::prelude::*;
    use snp_difftest::{
        BrandMap, Scenario, Tier, Yv, canonical_usage, mask_paths, normalise, strip_diag,
    };

    fn map() -> BrandMap {
        BrandMap::workspace().unwrap()
    }

    /// Lines that exercise every normaliser branch, mixed with arbitrary text.
    fn line() -> impl Strategy<Value = String> {
        prop_oneof![
            Just(String::from("Traceback (most recent call last):")),
            Just(String::from("  File \"x.py\", line 1, in <module>")),
            Just(String::from("ValueError: boom")),
            Just(String::new()),
            Just(String::from(
                "During handling of the above exception, another exception occurred:"
            )),
            Just(String::from("usage: pre-commit run [-h]")),
            Just(String::from(
                "pre-commit run: error: unrecognized arguments: --x"
            )),
            Just(String::from("- duration: 0.03s")),
            Just(String::from("[INFO] Installing environment for x.")),
            Just(String::from("h.......Passed")),
            Just(String::from(
                "[INFO] Stashing unstaged files to /r/store/patch1700000000-42."
            )),
            Just(String::from("-   repo: /r/tmp/tmpab12cd34/shadow-repo")),
            Just(String::from(
                "    rev: 508372f149f5f691200929308eaba68eb7ddd4da"
            )),
            Just(String::from(
                "[WinError 32] The process cannot access the file: 'C:\\x'"
            )),
            Just(String::from("pre-commit 4.6.2")),
            Just(String::from("  in \"<unicode string>\", line 2, column 1")),
            "[ -~]{0,40}",
            "\\PC{0,20}",
        ]
    }

    fn text() -> impl Strategy<Value = Vec<u8>> {
        prop::collection::vec((line(), prop::bool::ANY), 0..24).prop_map(|ls| {
            let mut out = Vec::new();
            for (l, crlf) in ls {
                out.extend_from_slice(l.as_bytes());
                out.extend_from_slice(if crlf { b"\r\n" } else { b"\n" });
            }
            out
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, ..ProptestConfig::default() })]

        #[test]
        fn scenario_parsing_never_panics(s in "\\PC{0,400}") {
            drop(Scenario::parse(&s, Path::new("fuzz.yaml")));
            drop(Yv::parse(&s));
            drop(BrandMap::parse(&s));
        }

        #[test]
        fn structured_scenarios_never_panic(
            keys in prop::collection::vec(
                prop_oneof![
                    Just("work"), Just("steps"), Just("repos"), Just("compare"), Just("requires"),
                    Just("expected"), Just("brands"), Just("status"), Just("expect_divergence"), Just("files"),
                    Just("tool"), Just("git"), Just("shell"), Just("signal"), Just("generated"), Just("count"),
                ],
                0..8,
            ),
            vals in prop::collection::vec("[ -~]{0,12}", 0..8),
        ) {
            let mut doc = String::from("name: f\n");
            for (k, v) in keys.iter().zip(vals.iter().chain(std::iter::repeat(&String::new()))) {
                writeln!(doc, "{k}: {v:?}").unwrap();
            }
            drop(Scenario::parse(&doc, Path::new("fuzz.yaml")));
        }

        #[test]
        fn normalisers_never_panic_and_are_idempotent(b in text(), raw in prop::collection::vec(any::<u8>(), 0..300)) {
            let m = map();
            for input in [&b, &raw] {
                for tier in [Tier::A, Tier::B, Tier::C] {
                    let once = normalise(tier, input, &m);
                    if !matches!(tier, Tier::A) {
                        prop_assert_eq!(normalise(tier, &once, &BrandMap::identity()), once.clone());
                    }
                }
                let u = canonical_usage(input);
                prop_assert_eq!(canonical_usage(&u), u);
                drop(mask_paths(input, &[b"/r/store", b"/r/snp-store"]));
                drop(strip_diag(input, None));
                drop(m.reduce_snp_only_parsers(input));
            }
        }

        #[test]
        fn brand_map_round_trip(picks in prop::collection::vec((0_usize..64, "[a-z ]{0,10}"), 0..12)) {
            let m = map();
            let phrases: Vec<&str> = m.substitutions().iter().map(|r| r.from.as_str()).collect();
            let mut t = String::new();
            for (i, filler) in picks {
                let p = phrases.get(i % phrases.len().max(1)).copied().unwrap_or_default();
                writeln!(t, "{filler}{p}{filler}").unwrap();
            }
            let forward = m.apply(t.as_bytes());
            prop_assert_eq!(m.invert(&forward), t.into_bytes());
        }
    }

    /// `a0: &a0 [x…]`, `aN: &aN [*aN-1…]`: `fan`^`depth` nodes once expanded.
    fn alias_chain(depth: usize, fan: usize) -> String {
        let mut doc = format!("a0: &a0 [{}]\n", vec!["x"; fan].join(", "));
        for l in 1..=depth {
            let prev = format!("*a{}", l - 1);
            writeln!(doc, "a{l}: &a{l} [{}]", vec![prev.as_str(); fan].join(", ")).unwrap();
        }
        doc
    }

    fn extreme_int() -> impl Strategy<Value = i64> {
        prop_oneof![
            Just(i64::MAX),
            Just(i64::MIN),
            Just(-1_i64),
            Just(0_i64),
            Just(100_000_i64),
            Just(100_001_i64),
            Just(67_108_864_i64),
            Just(67_108_865_i64),
            Just(4_294_967_296_i64),
            Just(86_400_i64),
            Just(86_401_i64),
            any::<i64>(),
        ]
    }

    /// Generous for debug builds: the budgets make each of these a few milliseconds in release.
    const BOUND: Duration = Duration::from_secs(10);

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, failure_persistence: None, ..ProptestConfig::default() })]

        #[test]
        fn alias_chains_are_bounded(depth in 1_usize..=10, fan in 1_usize..=10, scenario in any::<bool>()) {
            let mut doc = alias_chain(depth, fan);
            let start = Instant::now();
            if scenario {
                doc.push_str("name: b\nsteps: [{tool: [run]}]\n");
                drop(Scenario::parse(&doc, Path::new("bomb.yaml")));
            } else {
                drop(Yv::parse(&doc));
                drop(BrandMap::parse(&doc));
            }
            prop_assert!(start.elapsed() < BOUND, "{depth}x{fan}: {:?}", start.elapsed());
        }

        #[test]
        fn extreme_integers_are_bounded(
            count in extreme_int(),
            timeout in extreme_int(),
            unit in prop_oneof![Just(""), Just("x"), Just("0123456789abcdef")],
            files in 1_usize..=8,
        ) {
            let mut doc = String::from("name: n\nwork:\n  files:\n");
            for i in 0..files {
                writeln!(doc, "    f{i}: {{generate: {{count: {count}, repeat: {unit:?}}}}}").unwrap();
            }
            writeln!(doc, "  generated: {{pattern: \"g{{i}}\", count: {count}, content: {unit:?}}}").unwrap();
            writeln!(doc, "steps: [{{tool: [run], timeout: {timeout}}}]").unwrap();
            let start = Instant::now();
            drop(Scenario::parse(&doc, Path::new("ints.yaml")));
            prop_assert!(start.elapsed() < BOUND, "{count}/{timeout}: {:?}", start.elapsed());
        }
    }
}
