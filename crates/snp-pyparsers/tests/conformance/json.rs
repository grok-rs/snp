//! `snp_pyparsers::json` against CPython 3.12.3 and 3.13 (WBS WP-17 acceptance: `JSONTestSuite` acceptance and
//! byte-identical messages; pretty-format-json encoding on a 500-document corpus; CPython's nesting thresholds).
//!
//! Owner: WP-17.

use serde_json::Value;
use snp_pyparsers::json::{self, DumpOpts, JsonValue, PyMinor};

use super::support::{Failures, Want, got, input, rep, text_matches, vectors, want};

const MINORS: [(PyMinor, &str); 2] = [
    (PyMinor::P312, "json_312.json"),
    (PyMinor::P313, "json_313.json"),
];

/// The deep-nesting shapes of `pch_vectors.py` `SHAPES`.
fn shape(name: &str, n: usize) -> String {
    match name {
        "arrays" => rep("[", n) + &rep("]", n),
        "stop" => rep("[", n) + "x",
        "eof" => rep("[", n),
        "errmsg" => rep("[", n) + "1 2",
        "errmsg_str" => rep("[", n) + "\"\u{1}\"",
        "int" => rep("[", n) + &rep("1", 4301),
        "const" => rep("[", n) + "NaN" + &rep("]", n),
        "hook" => rep("[", n - 1) + "{\"a\":1}" + &rep("]", n.saturating_sub(11)) + " x",
        "hook_raise" => rep("[", n - 1) + "{\"a\":1,\"a\":2}" + &rep("]", n - 1),
        "objects" => rep("{\"a\":", n) + "1" + &rep("}", n),
        "objects_err" => rep("{\"a\":", n) + "1}x" + &rep("}", n - 1),
        "mixed" => rep("[{\"a\":", n / 2) + "[]" + &rep("}]", n / 2),
        other => panic!("unknown shape {other}"),
    }
}

fn opts(o: &Value) -> DumpOpts {
    let indent = match &o["indent"] {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => panic!("indent {other:?}"),
    };
    DumpOpts {
        indent,
        ensure_ascii: o["ensure_ascii"].as_bool().unwrap(),
        sort_keys: o["sort_keys"].as_bool().unwrap(),
        top_keys: o["top_keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k.as_str().unwrap().to_owned())
            .collect(),
    }
}

/// The flat token list of a decoded value (`pch_vectors.py` `tree()`).
fn tokens(v: &JsonValue) -> Vec<Value> {
    let mut out = Vec::new();
    let mut todo: Vec<Result<&JsonValue, &str>> = vec![Ok(v)];
    while let Some(x) = todo.pop() {
        match x {
            Err(key) => out.push(serde_json::json!(["k", key])),
            Ok(JsonValue::Null) => out.push(serde_json::json!("n")),
            Ok(JsonValue::Bool(true)) => out.push(serde_json::json!("t")),
            Ok(JsonValue::Bool(false)) => out.push(serde_json::json!("f")),
            Ok(JsonValue::Int(i)) => out.push(serde_json::json!(["i", &**i])),
            Ok(JsonValue::Float(f)) => {
                out.push(serde_json::json!(["d", snp_pycompat::float::repr(*f)]));
            }
            Ok(JsonValue::Str(s)) => out.push(serde_json::json!(["s", s])),
            Ok(JsonValue::Array(items)) => {
                out.push(serde_json::json!(["a", items.len()]));
                todo.extend(items.iter().rev().map(Ok));
            }
            Ok(JsonValue::Object(pairs)) => {
                out.push(serde_json::json!(["o", pairs.len()]));
                for (k, v) in pairs.iter().rev() {
                    todo.push(Ok(v));
                    todo.push(Err(k));
                }
            }
            Ok(other) => panic!("unknown JsonValue {other:?}"),
        }
    }
    out
}

fn universal_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

#[test]
fn check_matches_cpython() {
    let mut f = Failures::default();
    for (v, file) in MINORS {
        let data = vectors(file);
        for case in data["inputs"].as_array().unwrap() {
            let name = format!("{file} {}", case["name"].as_str().unwrap());
            let input = input(case);
            let w = want(&case["check"]);
            f.check(&name, &got(&json::check_ex(&input, v)), &w);
            let w3 = want(&case["check_v3"]);
            f.check(
                &format!("{name} (v3)"),
                &got(&json::check_v3(&input, v)),
                &w3,
            );
            // The frozen API renders the same outcome as a string.
            let frozen = json::check(&input, v);
            let expect = match &w {
                Want::Ok => Ok(()),
                Want::Reported(m) => Err(m.clone()),
                Want::Crash(t, m) => Err(if m.is_empty() {
                    t.clone()
                } else {
                    format!("{t}: {m}")
                }),
            };
            if frozen != expect {
                f.push(format!("{name}: frozen check {frozen:?} != {expect:?}"));
            }
        }
    }
    f.finish("json::check");
}

#[test]
fn loads_matches_cpython() {
    let mut f = Failures::default();
    for (v, file) in MINORS {
        let data = vectors(file);
        for case in data["inputs"].as_array().unwrap() {
            let name = format!("{file} {}", case["name"].as_str().unwrap());
            let input = input(case);
            let r = &case["loads"];
            match (json::loads(&input, v), r.get("value")) {
                (Ok(val), Some(want_tokens)) => {
                    let got_tokens = tokens(&val);
                    if &Value::Array(got_tokens.clone()) != want_tokens {
                        f.push(format!("{name}: value {got_tokens:?} != {want_tokens:?}"));
                    }
                }
                (Err(msg), None) => {
                    let expect = match want(r) {
                        Want::Reported(m) => m,
                        Want::Crash(t, m) => format!("{t}: {m}"),
                        Want::Ok => String::new(),
                    };
                    if msg != expect {
                        f.push(format!("{name}: loads error {msg:?} != {expect:?}"));
                    }
                }
                (got_r, _) => f.push(format!("{name}: loads {got_r:?} vs {r}")),
            }
        }
    }
    f.finish("json::loads");
}

#[test]
fn pretty_format_matches_cpython() {
    let mut f = Failures::default();
    let mut corpus = 0usize;
    for (v, file) in MINORS {
        let data = vectors(file);
        let inputs = data["inputs"].as_array().unwrap();
        for case in data["pretty"].as_array().unwrap() {
            let name = format!("{file} {}", case["name"].as_str().unwrap());
            let text = match (case.get("text"), case.get("input")) {
                (Some(t), _) => t.as_str().unwrap().to_owned(),
                (None, Some(i)) => {
                    let raw = input(&inputs[usize::try_from(i.as_u64().unwrap()).unwrap()]);
                    universal_newlines(std::str::from_utf8(&raw).unwrap())
                }
                _ => panic!("{name}: no text"),
            };
            let o = opts(&case["opts"]);
            let r = json::pretty_format(&text, &o, v);
            let result = &case["result"];
            if result.get("pretty").is_some() || result.get("pretty_fnv").is_some() {
                corpus += 1;
                match &r {
                    Ok(s) if text_matches(result, "pretty", s) == Some(true) => {}
                    other => f.push(format!("{name}: {other:?}\n    want {result}")),
                }
            } else {
                f.check(&name, &got(&r), &want(result));
            }
        }
    }
    assert!(corpus >= 1000, "{corpus} pretty documents");
    f.finish("json::pretty_format");
}

/// `dumps_pretty` on `loads`' tree agrees with the exact path wherever Python neither crashes nor sees lone
/// surrogates.
#[test]
fn dumps_pretty_agrees_with_pretty_format() {
    let mut f = Failures::default();
    let data = vectors("json_313.json");
    for case in data["pretty"].as_array().unwrap() {
        let Some(text) = case.get("text").and_then(Value::as_str) else {
            continue;
        };
        let Some(p) = case["result"].get("pretty").and_then(Value::as_str) else {
            continue;
        };
        if text.starts_with('\u{feff}') || text.contains("\\ud") || text.contains("\\uD") {
            continue;
        }
        let o = opts(&case["opts"]);
        let Ok(val) = json::loads(text.as_bytes(), PyMinor::P313) else {
            continue;
        };
        let out = json::dumps_pretty(&val, &o);
        if out != p {
            f.push(format!("{}: {out:?}\n    want {p:?}", case["name"]));
        }
    }
    f.finish("json::dumps_pretty");
}

#[test]
fn recursion_thresholds_match_cpython() {
    let mut f = Failures::default();
    let mut v3_seen = false;
    for (v, file) in MINORS {
        let data = vectors(file);
        for case in data["depth"].as_array().unwrap() {
            let (mode, sh) = (
                case["mode"].as_str().unwrap(),
                case["shape"].as_str().unwrap(),
            );
            let n = usize::try_from(case["n"].as_u64().unwrap()).unwrap();
            let doc = shape(sh, n);
            let name = format!("{file} {mode} {sh} {n}");
            let w = want(&case["result"]);
            let g = if mode == "check" {
                got(&json::check_ex(doc.as_bytes(), v))
            } else if mode == "check_v3" {
                v3_seen = true;
                got(&json::check_v3(doc.as_bytes(), v))
            } else {
                let o = DumpOpts {
                    indent: "2".into(),
                    ensure_ascii: true,
                    sort_keys: true,
                    top_keys: Vec::new(),
                };
                match got(&json::pretty_format(&doc, &o, v)) {
                    Want::Reported(_) => Want::Reported("invalid".into()),
                    other => other,
                }
            };
            f.check(&name, &g, &w);
        }
    }
    assert!(
        v3_seen,
        "the vectors lack the check-json v3 (no hook) depth cases"
    );
    f.finish("json recursion thresholds");
}

#[test]
fn dumps_pretty_never_fails() {
    let deep = (0..20_000).fold(JsonValue::Null, |acc, _| JsonValue::Array(vec![acc]));
    let o = DumpOpts {
        indent: "1".into(),
        ensure_ascii: true,
        sort_keys: true,
        top_keys: Vec::new(),
    };
    let out = json::dumps_pretty(&deep, &o);
    assert!(out.starts_with("[\n ["));
    drop(deep); // `JsonValue`'s drop is iterative
    let dup = JsonValue::Object(vec![
        ("a".into(), JsonValue::Int("1".into())),
        ("a".into(), JsonValue::Object(Vec::new())),
    ]);
    assert_eq!(json::dumps_pretty(&dup, &o), "{\n \"a\": {}\n}\n");
}
