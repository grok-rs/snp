//! Shared helpers of snp-yaml's integration tests: the committed oracle vectors (`tests/vectors/*.json`, generated
//! by `cargo xtask yaml-vectors` from PyYAML 6.0.3 + libyaml 0.2.5) and the canonical value encoding they use.

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::{Value as Json, json};
use snp_yaml::{Limits, YamlError};

/// Reads `tests/vectors/<name>` and returns its `vectors` array.
pub(crate) fn vectors(name: &str) -> Vec<Json> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/vectors")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let doc: Json = serde_json::from_str(&text).expect("valid JSON");
    doc["vectors"].as_array().expect("vectors array").clone()
}

/// The generator's canonical encoding of a Python value.
pub(crate) fn canon(v: &cfgv::Value) -> Json {
    use cfgv::Value as V;
    match v {
        V::Null => json!(["null"]),
        V::Bool(b) => json!(["bool", b]),
        V::Int(i) => json!(["int", i.to_string()]),
        V::BigInt(b) => json!(["int", b.to_string()]),
        V::Float(x) => json!(["float", snp_pycompat::float::repr(*x)]),
        V::String(s) => json!(["str", s]),
        V::Bytes(b) => json!([
            "bytes",
            b.iter().fold(String::new(), |mut out, x| {
                write!(out, "{x:02x}").unwrap();
                out
            })
        ]),
        V::Date(_) => json!(["date", v.repr()]),
        V::DateTime(_) => json!(["datetime", v.repr()]),
        V::List(items) => json!(["list", items.iter().map(canon).collect::<Vec<_>>()]),
        V::Tuple(items) => json!(["tuple", items.iter().map(canon).collect::<Vec<_>>()]),
        V::Map(m) => json!([
            "dict",
            m.iter()
                .map(|(k, v)| json!([canon(k), canon(v)]))
                .collect::<Vec<_>>()
        ]),
        V::Set(s) => json!(["set", sort_members(s.iter().map(canon).collect())]),
        other => panic!("unexpected value {other:?}"),
    }
}

/// Sorts set members by their JSON text (both sides of a comparison use this order).
pub(crate) fn sort_members(mut members: Vec<Json>) -> Vec<Json> {
    members.sort_by_key(ToString::to_string);
    members
}

/// Normalises an expected canonical value: set members sorted with [`sort_members`].
pub(crate) fn normalize(v: &Json) -> Json {
    match v {
        Json::Array(items) => {
            let mut items: Vec<Json> = items.iter().map(normalize).collect();
            if items.first() == Some(&json!("set"))
                && let Some(Json::Array(members)) = items.get_mut(1)
            {
                *members = sort_members(std::mem::take(members));
            }
            Json::Array(items)
        }
        other => other.clone(),
    }
}

/// The outcome of `load` in the vectors' format: `{"value": canon}` or `{"error": [type, text]}`.
pub(crate) fn outcome(r: Result<cfgv::Value, YamlError>) -> Json {
    match r {
        Ok(v) => json!({ "value": canon(&v) }),
        Err(e) => json!({ "error": [e.py_type(), e.to_string()] }),
    }
}

/// `load` with the default limits.
pub(crate) fn load(text: &str) -> Result<cfgv::Value, YamlError> {
    snp_yaml::load(text, &Limits::default())
}

/// Compares a `load` outcome with an expected vector result. Python-only outcomes (`cycle`: a self-referencing
/// value; `too_large`: an expansion beyond the node limit) must be snp errors (D-SEC-11).
pub(crate) fn check_load(text: &str, expected: &Json, what: &str) -> Result<(), String> {
    let got = load(text);
    if expected.get("cycle").is_some() {
        return match got {
            Err(e)
                if e.to_string()
                    .starts_with("found unconstructable recursive node") =>
            {
                Ok(())
            }
            other => Err(format!(
                "{what}: {text:?}: expected the recursion error, got {other:?}"
            )),
        };
    }
    if expected.get("too_large").is_some() {
        return match got {
            Err(e) if e.py_type() == "YAMLError" => Ok(()),
            other => Err(format!(
                "{what}: {text:?}: expected a limit error, got {other:?}"
            )),
        };
    }
    let expected = match expected.get("value") {
        Some(v) => json!({ "value": normalize(v) }),
        None => expected.clone(),
    };
    let got = outcome(got);
    if got == expected {
        Ok(())
    } else {
        Err(format!(
            "{what}: {text:?}\n  expected {expected}\n       got {got}"
        ))
    }
}

/// Fails with every collected mismatch.
pub(crate) fn report(failures: &[String], total: usize) {
    assert!(
        failures.is_empty(),
        "{} of {total} vectors differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
