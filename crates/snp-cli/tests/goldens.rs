//! The argparse goldens of `cargo xtask cli-goldens` (12 §A.16, ADR-058), replayed through snp:
//!
//! * `goldens/compat.json`: pre-commit 4.7.0 on CPython 3.13.14 (`pc470-py313`), compat brand;
//! * `goldens/snp.json`: a real CPython 3.13.14 argparse built from `export_json(SNP_SPEC, "snp")`, cross-checked by
//!   the generator against the prog-patched oracle for every parser identical to pre-commit's.
//!
//! Help/version cases must produce the same stdout ([`CliOutcome::Stdout`]), usage errors the same stderr
//! ([`CliOutcome::Usage`]), and successful parses the same namespace values (`vars(args)`, compared through
//! [`Namespace`]'s accessors).
//!
//! Owner: WP-12.

#[cfg(test)]
mod goldens {
    use std::ffi::OsString;

    use serde_json::Value;
    use snp_argparse::{Namespace, Outcome, ParseCfg, ParserSpec};
    use snp_cli::CliOutcome;
    use snp_core::brand::Brand;
    use snp_core::env::Env;

    fn os(v: &Value) -> OsString {
        match v {
            Value::String(s) => OsString::from(s),
            Value::Object(m) => {
                let digits: Vec<u8> = m["hex"]
                    .as_str()
                    .unwrap()
                    .bytes()
                    .map(|d| u8::try_from(char::from(d).to_digit(16).unwrap()).unwrap())
                    .collect();
                let bytes: Vec<u8> = digits
                    .iter()
                    .zip(digits.iter().skip(1))
                    .enumerate()
                    .filter(|(i, _)| i % 2 == 0)
                    .map(|(_, (hi, lo))| hi * 16 + lo)
                    .collect();
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt as _;
                    OsString::from_vec(bytes)
                }
                #[cfg(not(unix))]
                {
                    OsString::from(String::from_utf8_lossy_owned(bytes))
                }
            }
            other => panic!("bad string {other:?}"),
        }
    }

    fn has_hex(v: &Value) -> bool {
        match v {
            // a byte-string value, or a mapping (the case's `env`) holding one
            Value::Object(m) => m.contains_key("hex") || m.values().any(has_hex),
            Value::Array(a) => a.iter().any(has_hex),
            _ => false,
        }
    }

    /// The namespaces from the top level down the sub-command chain (the flat Python namespace is their union).
    fn chain(ns: &Namespace) -> Vec<&Namespace> {
        let mut out = vec![ns];
        let mut cur = ns;
        while let Some((_, sub)) = cur.command() {
            out.push(sub);
            cur = sub;
        }
        out
    }

    fn check_ns(id: &str, ns: &Namespace, expected: &serde_json::Map<String, Value>) {
        let levels = chain(ns);
        for (key, want) in expected {
            match want {
                Value::Null => {
                    for l in &levels {
                        assert!(
                            l.value(key).is_none()
                                && l.int(key).is_none()
                                && l.list(key).is_empty()
                                && !l.flag(key),
                            "{id}: {key} should be None"
                        );
                    }
                }
                Value::Bool(b) => {
                    let got = levels.iter().any(|l| l.flag(key));
                    assert_eq!(got, *b, "{id}: {key}");
                }
                Value::Number(n) => {
                    let got = levels.iter().rev().find_map(|l| l.int(key));
                    assert_eq!(got, n.as_i64(), "{id}: {key}");
                }
                Value::String(_) | Value::Object(_) => {
                    let got = levels.iter().rev().find_map(|l| l.value(key));
                    assert_eq!(got, Some(os(want).as_os_str()), "{id}: {key}");
                }
                Value::Array(items) => {
                    let want: Vec<OsString> = items.iter().map(os).collect();
                    let got = levels
                        .iter()
                        .rev()
                        .map(|l| l.list(key))
                        .find(|l| !l.is_empty())
                        .unwrap_or_default();
                    assert_eq!(got, want.as_slice(), "{id}: {key}");
                }
            }
        }
    }

    fn replay(file: &str, brand: Brand, spec: &'static ParserSpec) {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/goldens")
                .join(file),
        )
        .unwrap();
        let doc: Value = serde_json::from_str(&text).unwrap();
        let cases = doc["cases"].as_array().unwrap();
        assert!(cases.len() > 400, "{file}: {} cases", cases.len());
        let mut failures = Vec::new();
        for case in cases {
            let id = case["id"].as_str().unwrap();
            let argv: Vec<OsString> = case["argv"].as_array().unwrap().iter().map(os).collect();
            if !cfg!(unix) && (has_hex(&case["argv"]) || has_hex(&case["env"])) {
                continue;
            }
            let env = Env::from_pairs(
                case["env"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (OsString::from(k), os(v))),
            );
            let exit = case["exit"].as_i64().unwrap();
            let stdout = case["stdout"].as_str().unwrap();
            let stderr = case["stderr"].as_str().unwrap();
            if let Some(ns) = case["ns"].as_object() {
                let cfg = ParseCfg {
                    prog: brand.prog(),
                    env: &env,
                    columns: snp_argparse::columns(&env, None),
                };
                let run = [OsString::from("run")];
                let argv = if argv.is_empty() { &run[..] } else { &argv[..] };
                match snp_argparse::parse(spec, argv, &cfg) {
                    Outcome::Parsed(got) => {
                        if let Err(e) = std::panic::catch_unwind(|| check_ns(id, &got, ns)) {
                            failures.push(format!("{id}: {e:?}"));
                        }
                    }
                    other => failures.push(format!("{id}: expected a namespace, got {other:?}")),
                }
                continue;
            }
            let got = snp_cli::parse(brand, &argv, &env, None);
            let ok = match (&got, exit) {
                (CliOutcome::Stdout(s), 0) => s == stdout && stderr.is_empty(),
                (CliOutcome::Usage(s), 2) => s == stderr && stdout.is_empty(),
                _ => false,
            };
            if !ok {
                failures.push(format!(
                    "{id}: expected exit {exit}\n--- stdout\n{stdout}--- stderr\n{stderr}--- got\n{got:#?}"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{file}: {} of {} cases differ:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn compat_brand_matches_pre_commit() {
        replay("compat.json", Brand::PreCommit, snp_cli::PRE_COMMIT_SPEC);
    }

    #[test]
    fn snp_brand_matches_cpython_argparse() {
        replay("snp.json", Brand::Snp, snp_cli::SNP_SPEC);
    }
}
