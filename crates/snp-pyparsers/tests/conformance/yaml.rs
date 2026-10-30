//! `snp_pyparsers::yamlcheck` against ruamel.yaml 0.19.1 as `check-yaml` runs it (WBS WP-17 acceptance:
//! yaml-test-suite, the 09 §4.C catalogue, chunk-boundary and encoding cases, mutants, multi-file runs that
//! carry state between files, and the recursion thresholds of the real console script).
//!
//! Owner: WP-17.

use snp_pyparsers::json::PyMinor;
use snp_pyparsers::yamlcheck::{Checker, Mode};

use super::support::{Failures, Want, got, input, rep, vectors, want};

const MINORS: [(PyMinor, &str); 2] = [
    (PyMinor::P312, "yaml_312.json"),
    (PyMinor::P313, "yaml_313.json"),
];

const MODES: [(Mode, &str); 3] = [
    (Mode::Safe, "safe"),
    (Mode::AllowMultipleDocuments, "multi"),
    (Mode::Unsafe, "unsafe"),
];

/// `pch_vectors.py` `YAML_SHAPES`.
fn shape(name: &str, n: usize) -> String {
    match name {
        "fseq-empty" => rep("[", n) + &rep("]", n),
        "fseq-int" => rep("[", n) + "1" + &rep("]", n),
        "fseq-str" => rep("[", n) + "abc" + &rep("]", n),
        "fseq-dq" => rep("[", n) + "\"x\"" + &rep("]", n),
        "fseq-err" => rep("[", n) + "a: b: c",
        "fseq-tag" => rep("[", n) + "!foo x" + &rep("]", n),
        "fmap" => rep("{a: ", n) + "1" + &rep("}", n),
        "fmap-empty" => rep("{a: ", n) + "{}" + &rep("}", n),
        "fmap-dup" => rep("{a: ", n) + "{b: 1, b: 2}" + &rep("}", n),
        "bseq" => rep("- ", n) + "1",
        "bmap" => (0..n).map(|i| rep(" ", i) + "a:\n").collect::<String>() + &rep(" ", n) + "1\n",
        "key-fseq" => "? ".to_owned() + &rep("[", n) + &rep("]", n) + "\n",
        "key-fmap" => "? ".to_owned() + &rep("{a: ", n) + "1" + &rep("}", n) + "\n",
        "mixed" => rep("[{a: ", n) + "1" + &rep("}]", n),
        "anchors" => rep("[&a ", n) + "1" + &rep("]", n),
        "merge" => rep("{<<: ", n) + "{a: 1}" + &rep("}", n),
        other => panic!("unknown shape {other}"),
    }
}

fn run_files(files: &[Vec<u8>], m: Mode, v: PyMinor) -> Vec<Want> {
    let mut checker = Checker::new(m, v);
    let mut out = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let r = checker.check_file(f, &format!("f{i}.yaml"));
        let w = got(&r);
        let stop = matches!(w, Want::Crash(..));
        out.push(w);
        if stop {
            break;
        }
    }
    out
}

#[test]
fn check_matches_ruamel() {
    let mut f = Failures::default();
    let mut n = 0usize;
    for (v, file) in MINORS {
        let data = vectors(file);
        for case in data["cases"].as_array().unwrap() {
            let bytes = input(case);
            for (m, key) in MODES {
                let name = format!("{file} {} {key}", case["name"].as_str().unwrap());
                let g = run_files(std::slice::from_ref(&bytes), m, v);
                f.check(&name, &g[0], &want(&case["result"][key]));
                n += 1;
            }
        }
    }
    assert!(n >= 9000, "{n} cases");
    f.finish("yamlcheck");
}

#[test]
fn state_carries_over_between_files() {
    let mut f = Failures::default();
    for (v, file) in MINORS {
        let data = vectors(file);
        for case in data["sequences"].as_array().unwrap() {
            let files: Vec<Vec<u8>> = case["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(input)
                .collect();
            for (m, key) in MODES {
                let name = format!("{file} {} {key}", case["name"].as_str().unwrap());
                let g = run_files(&files, m, v);
                let w: Vec<Want> = case["result"][key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(want)
                    .collect();
                if g != w {
                    f.push(format!("{name}:\n     got {g:?}\n    want {w:?}"));
                }
            }
        }
    }
    f.finish("yamlcheck sequences");
}

#[test]
fn recursion_thresholds_match_check_yaml() {
    let mut failures = Failures::default();
    for (v, file) in MINORS {
        let data = vectors(file);
        for case in data["depth"].as_array().unwrap() {
            let sh = case["shape"].as_str().unwrap();
            let mode = case["mode"].as_str().unwrap();
            let m = MODES.iter().find(|(_, k)| *k == mode).unwrap().0;
            let n = usize::try_from(case["n"].as_u64().unwrap()).unwrap();
            let name = format!("{file} {sh} {mode} {n}");
            let outcome = run_files(&[shape(sh, n).into_bytes()], m, v)
                .into_iter()
                .next()
                .unwrap();
            let expected = want(&case["result"]);
            let same = match (&outcome, &expected) {
                (Want::Crash(a, _), Want::Crash(b, _)) => a == b,
                _ => outcome == expected,
            };
            if !same {
                failures.check(&name, &outcome, &expected);
            }
        }
    }
    failures.finish("yaml recursion thresholds");
}

#[test]
fn frame_model_matches_python() {
    let mut f = Failures::default();
    let mut n = 0usize;
    for (v, file) in MINORS {
        let data = vectors(file);
        for case in data["frames"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let doc = data["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"].as_str() == Some(name))
                .map(input)
                .unwrap();
            for (m, key) in MODES {
                let mut c = Checker::new(m, v);
                c.warm();
                drop(c.check_file(b"", "f0.yaml"));
                drop(c.check_file(&doc, "f1.yaml"));
                let want = case["peak"][key].as_u64().unwrap();
                if u64::from(c.frame_peak()) != want {
                    f.push(format!(
                        "{file} {name} {key}: modelled {} python {want}",
                        c.frame_peak()
                    ));
                }
                n += 1;
            }
        }
    }
    assert!(n > 1000, "{n}");
    f.finish("yaml frame model");
}

/// The deepest inputs Python accepts (and the first ones it rejects) run on a 256 KiB thread: no Rust recursion
/// grows with the input beyond what CPython's recursion limit allows (ARCHITECTURE §13.5 fuzz stacks).
#[test]
fn deep_inputs_fit_small_stacks() {
    let docs: Vec<String> = [
        ("fseq-empty", 600),
        ("key-fseq", 248),
        ("key-fmap", 198),
        ("merge", 600),
        ("mixed", 600),
        ("bmap", 600),
        ("fmap-dup", 600),
    ]
    .iter()
    .flat_map(|&(s, n)| [shape(s, n), shape(s, n.saturating_sub(2))])
    .chain([
        format!("a: !!str {}x{}", rep("{=: ", 480), rep("}", 480)),
        format!("? !!timestamp {}{}\n: 1", rep("[", 240), rep("]", 240)),
        format!("!!timestamp &a [{}*a{}]", rep("[", 240), rep("]", 240)),
        rep("[", 100_000),
    ])
    .collect();
    let h = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            for d in &docs {
                for (m, _) in MODES {
                    let mut c = Checker::new(m, PyMinor::P313);
                    drop(c.check_file(d.as_bytes(), "f.yaml"));
                    // a second file after a deep failure (leftover generators, deep_construct)
                    drop(c.check_file(d.as_bytes(), "g.yaml"));
                }
            }
        })
        .unwrap();
    h.join().unwrap();
}
