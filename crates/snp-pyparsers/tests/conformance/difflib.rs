//! `snp_pyparsers::difflib::unified_diff` against CPython's `difflib` (WBS WP-17 acceptance: difflib vectors).
//!
//! Owner: WP-17.

use serde_json::Value;
use snp_pyparsers::difflib::unified_diff;

use super::support::{Failures, text_matches, vectors};

fn lines(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn unified_diff_matches_cpython() {
    let data = vectors("difflib.json");
    let mut f = Failures::default();
    let mut n = 0usize;
    for case in data["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let (a, b) = (lines(&case["a"]), lines(&case["b"]));
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        let b: Vec<&str> = b.iter().map(String::as_str).collect();
        let file = case["file"].as_str().unwrap();
        let got = unified_diff(&a, &b, file, file);
        if text_matches(case, "diff", &got) != Some(true) {
            f.push(format!("{name}:\n{got}\n--- want ---\n{case}"));
        }
        n += 1;
    }
    assert!(n >= 500, "{n} cases");
    f.finish("difflib::unified_diff");
}
