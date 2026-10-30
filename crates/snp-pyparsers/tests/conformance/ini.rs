//! `snp_pyparsers::ini` against CPython's `configparser` as `detect-aws-credentials` drives it (09 §4.D).
//!
//! Owner: WP-17.

use snp_pyparsers::CheckError;
use snp_pyparsers::ini::aws_credentials_ex;

use super::support::{Failures, vectors};

#[test]
fn aws_credentials_match_cpython() {
    let data = vectors("ini.json");
    let mut f = Failures::default();
    for case in data["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let text = case["text"].as_str().unwrap();
        let r = &case["result"];
        let got = aws_credentials_ex(text, "creds.ini");
        match (got, r.get("pairs"), r.get("crash")) {
            (Ok(pairs), Some(want), _) => {
                let want: Vec<(String, String)> = want
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        (
                            p[0].as_str().unwrap().to_owned(),
                            p[1].as_str().unwrap().to_owned(),
                        )
                    })
                    .collect();
                if pairs != want {
                    f.push(format!("{name} {text:?}: {pairs:?} != {want:?}"));
                }
            }
            (Err(CheckError::Crash(e)), _, Some(want)) => {
                let (ty, msg) = (want[0].as_str().unwrap(), want[1].as_str().unwrap());
                let got_msg = String::from_utf8_lossy(&e.text);
                if e.ty != ty || got_msg != msg {
                    f.push(format!(
                        "{name} {text:?}: {}: {got_msg} != {ty}: {msg}",
                        e.ty
                    ));
                }
            }
            (got, _, _) => f.push(format!("{name} {text:?}: {got:?} vs {r}")),
        }
    }
    f.finish("ini::aws_credentials");
}
