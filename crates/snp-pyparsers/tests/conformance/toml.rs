//! `snp_pyparsers::toml` against CPython's `tomllib` as `check-toml` runs it (WBS WP-17 acceptance: toml-test,
//! the 09 §4.C catalogue, mutants, and the recursion thresholds of the real console script).
//!
//! Owner: WP-17.

use snp_pyparsers::json::PyMinor;
use snp_pyparsers::toml;

use super::support::{Failures, got, input, rep, vectors, want};

const MINORS: [(PyMinor, &str); 2] = [
    (PyMinor::P312, "toml_312.json"),
    (PyMinor::P313, "toml_313.json"),
];

/// `pch_vectors.py` `TOML_SHAPES`.
fn shape(name: &str, n: usize) -> String {
    let body = match name {
        "arr-int" => rep("[", n) + "1" + &rep("]", n),
        "arr-empty" => rep("[", n) + &rep("]", n),
        "arr-true" => rep("[", n) + "true" + &rep("]", n),
        "arr-str" => rep("[", n) + "\"x\"" + &rep("]", n),
        "arr-mlstr" => rep("[", n) + "\"\"\"x\\u0041\"\"\"" + &rep("]", n),
        "arr-date" => rep("[", n) + "1979-05-27T07:32:00+01:00" + &rep("]", n),
        "arr-err" => rep("[", n) + "x",
        "arr-comment" => rep("[#c\n", n) + &rep("]", n),
        "tbl-int" => rep("{b = ", n) + "1" + &rep("}", n),
        "tbl-empty" => rep("{b = ", n) + "{}" + &rep("}", n),
        "tbl-quoted" => rep("{\"b\" = ", n) + "1" + &rep("}", n),
        "tbl-dup" => rep("{b = ", n) + "{c = 1, c = 2}" + &rep("}", n),
        "mixed" => rep("[{b = ", n) + "1" + &rep("}]", n),
        other => panic!("unknown shape {other}"),
    };
    format!("a = {body}")
}

#[test]
fn check_matches_tomllib() {
    let mut f = Failures::default();
    let mut n = 0usize;
    for (v, file) in MINORS {
        let data = vectors(file);
        for case in data["cases"].as_array().unwrap() {
            let name = format!("{file} {}", case["name"].as_str().unwrap());
            f.check(
                &name,
                &got(&toml::check_ex(&input(case), v)),
                &want(&case["result"]),
            );
            n += 1;
        }
    }
    assert!(n >= 2000, "{n} cases");
    f.finish("toml::check");
}

#[test]
fn recursion_thresholds_match_check_toml() {
    let mut failures = Failures::default();
    for (v, file) in MINORS {
        let data = vectors(file);
        for case in data["depth"].as_array().unwrap() {
            let sh = case["shape"].as_str().unwrap();
            let n = usize::try_from(case["n"].as_u64().unwrap()).unwrap();
            let name = format!("{file} {sh} {n}");
            let got_outcome = got(&toml::check_ex(shape(sh, n).as_bytes(), v));
            let want_outcome = want(&case["result"]);
            // Crash texts of the recursion limit vary with the frame that overflows; the type is what matters.
            let same = match (&got_outcome, &want_outcome) {
                (
                    super::support::Want::Crash(ty_got, _),
                    super::support::Want::Crash(ty_want, _),
                ) => ty_got == ty_want,
                _ => got_outcome == want_outcome,
            };
            if !same {
                failures.check(&name, &got_outcome, &want_outcome);
            }
        }
    }
    failures.finish("toml recursion thresholds");
}

#[test]
fn frozen_check_is_313() {
    assert_eq!(toml::check(b"a = 1"), Ok(()));
    let long_key = format!("a.{} = 1", vec!["x"; 1000].join("."));
    assert!(
        toml::check(long_key.as_bytes())
            .unwrap_err()
            .starts_with("RecursionError: TOML key has more than")
    );
    assert_eq!(
        toml::check_ex(long_key.as_bytes(), PyMinor::P312).map_err(|e| e.to_string()),
        Ok(())
    );
}
