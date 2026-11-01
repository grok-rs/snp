//! The 09 §4.C catalogue (ruamel 0.19.1 under CPython 3.12/3.13); the full corpus is in
//! `tests/conformance/yaml.rs`.
//!
//! Owner: WP-17.

use super::*;

fn run(text: &str, m: Mode) -> Result<(), String> {
    Checker::new(m, PyMinor::P313)
        .check_text(text, "f.yaml")
        .map_err(CheckError::into_message)
}

#[test]
fn catalogue() {
    for (input, want) in [
        (
            "a: 1\na: 2\n",
            "while constructing a mapping\n  in \"f.yaml\", line 1, column 1\nfound duplicate key \"a\" with value \"2\" (original value: \"1\")\n  in \"f.yaml\", line 2, column 1\n\nTo suppress this check see:\n    https://yaml.dev/doc/ruamel.yaml/api/#Duplicate_keys\n",
        ),
        (
            "a: 1\n---\nb: 2\n",
            "expected a single document in the stream\n  in \"f.yaml\", line 1, column 1\nbut found another document\n  in \"f.yaml\", line 2, column 1",
        ),
        (
            "a: !foo 1",
            "could not determine a constructor for the tag '!foo'\n  in \"f.yaml\", line 1, column 4",
        ),
        (
            "a: b: c",
            "mapping values are not allowed here\n  in \"f.yaml\", line 1, column 5",
        ),
        (
            "a: \"\x07\"",
            "unacceptable character #x0007: special characters are not allowed\n  in \"f.yaml\", position 4",
        ),
        (
            "a: *nope",
            "found undefined alias 'nope'\n  in \"f.yaml\", line 1, column 4",
        ),
        (
            "key: value\n\tbad: tab",
            "while scanning for the next token\nfound character '\\t' that cannot start any token\n  in \"f.yaml\", line 2, column 1",
        ),
        (
            "a: [1, 2\n",
            "while parsing a flow sequence\n  in \"f.yaml\", line 1, column 4\nexpected ',' or ']', but got '<stream end>'\n  in \"f.yaml\", line 2, column 1",
        ),
        (
            "1: a\ntrue: b",
            "while constructing a mapping\n  in \"f.yaml\", line 1, column 1\nfound duplicate key \"True\" with value \"b\" (original value: \"a\")\n  in \"f.yaml\", line 2, column 1\n\nTo suppress this check see:\n    https://yaml.dev/doc/ruamel.yaml/api/#Duplicate_keys\n",
        ),
        (
            "a: !!int abc",
            "ValueError: invalid literal for int() with base 10: 'abc'",
        ),
        ("a: 2001-13-45", "ValueError: month must be in 1..12"),
        ("a: !!bool maybe", "KeyError: 'maybe'"),
        (
            "%YAML 1.3\n---\na: 1",
            "AssertionError: version minor part can only be 2 or 1, got (1, 3)",
        ),
    ] {
        assert_eq!(run(input, Mode::Safe), Err(want.to_owned()), "{input:?}");
    }
    for ok in [
        "",
        "\u{feff}a: 1",
        "a: 1\r\nb: 2\r\n",
        "? [1, 2]",
        "%YAML 1.1\n---\na: yes",
        "a: !!binary \"!!!\"",
        "---\n...\n",
        "a: &x 1\nb: &x 2",
        "a: &b {x: 1}\nc:\n  <<: *b",
    ] {
        assert_eq!(run(ok, Mode::Safe), Ok(()), "{ok:?}");
    }
    assert_eq!(
        run("a: 1\n---\nb: 1\n", Mode::AllowMultipleDocuments),
        Ok(())
    );
    assert_eq!(run("a: !foo 1\nb: *x\nc: 1\nc: 2", Mode::Unsafe), Ok(()));
}

/// `s` repeated `n` times.
fn rep(s: &str, n: usize) -> String {
    std::iter::repeat_n(s, n).collect()
}

/// `k0: 0\nk1: 1\n…` with `n` keys.
fn flat_mapping(n: usize) -> String {
    use std::fmt::Write as _;
    (0..n).fold(String::new(), |mut s, i| {
        writeln!(s, "k{i}: {i}").unwrap_or_default();
        s
    })
}

/// A document of `n` entries.
type Shape = fn(usize) -> String;

/// The fastest of three runs of `check-yaml` on `doc`, which must pass.
fn best_time(doc: &str, m: Mode) -> std::time::Duration {
    (0..3)
        .map(|_| {
            let t = std::time::Instant::now();
            assert_eq!(run(doc, m), Ok(()));
            t.elapsed()
        })
        .min()
        .unwrap_or_default()
}

/// Construction is linear in the length of one collection: 4× the entries may cost at most 8× the time (the old
/// per-child node clones made it 16×). Covers block and flow sequences, mappings and merge-key lists.
#[test]
fn construction_is_linear_in_collection_length() {
    let shapes: [(&str, Shape); 4] = [
        ("block seq", |n| rep("- i\n", n)),
        ("flow seq", |n| {
            let items: Vec<String> = (0..n).map(|i| i.to_string()).collect();
            format!("[{}]\n", items.join(", "))
        }),
        ("mapping", flat_mapping),
        ("merge list", |n| {
            let refs: Vec<&str> = std::iter::repeat_n("*a", n).collect();
            format!("a: &a {{x: 1}}\nb:\n  <<: [{}]\n  y: 2\n", refs.join(", "))
        }),
    ];
    for (name, make) in shapes {
        for m in [Mode::Safe, Mode::AllowMultipleDocuments] {
            // 5 k / 20 k entries: big enough to dominate the fixed costs, small enough for a debug build on a slow
            // runner (20 k / 80 k took ~2 minutes on macOS Intel)
            let small = best_time(&make(5_000), m);
            let big = best_time(&make(20_000), m);
            let ratio = big.as_secs_f64() / small.as_secs_f64().max(1e-6);
            assert!(
                ratio < 8.0,
                "{name} {m:?}: 5k {small:?}, 20k {big:?} (×{ratio:.1})"
            );
        }
    }
}

/// A chain of merges whose `<<` lists double each level stays near Python's cost (it was ~16× slower).
#[test]
fn merge_chains_do_not_copy_per_step() {
    use std::fmt::Write as _;
    let mut doc = String::from("a0: &a0 {k0: 1}\n");
    for i in 1..14 {
        writeln!(
            doc,
            "a{i}: &a{i} {{<<: [*a{p}, *a{p}], k{i}: 1}}",
            p = i - 1
        )
        .unwrap_or_default();
    }
    let t = best_time(&doc, Mode::Safe);
    assert!(t < std::time::Duration::from_secs(20), "{t:?}");
}

/// `--allow-multiple-documents` drops each document after constructing it (ruamel's memory profile): the
/// arenas hold one document, not the whole file.
#[test]
fn documents_are_dropped_after_construction() {
    let doc = rep("---\na: 1\nb: [1, 2]\n", 2000);
    let mut c = Checker::new(Mode::AllowMultipleDocuments, PyMinor::P313);
    assert!(c.check_text(&doc, "f.yaml").is_ok());
    let (nodes, objs) = c.st.cons.arena_len();
    assert!(nodes <= 8 && objs <= 8, "nodes {nodes}, objects {objs}");
    // A document that fails construction keeps its state for the next file, as in Python.
    let bad = "---\na: 1\n---\nb: 1\nb: 2\n";
    assert!(c.check_text(bad, "g.yaml").is_err());
    assert!(c.st.cons.arena_len().0 > 0);
    assert!(c.check_text("a: 1\n", "h.yaml").is_ok());
}

/// 300k-entry collections in release mode, 3× over the measured times (sequence ~0.5 s, mapping ~1.9 s; ruamel
/// needs ~9 s for 100k keys and the old quadratic constructor ~50 s for 300k).
#[test]
#[ignore = "perf"]
fn large_collections_perf() {
    let seq = rep("- i\n", 300_000);
    let map = flat_mapping(300_000);
    for (name, doc, budget) in [("sequence", &seq, 1.6), ("mapping", &map, 5.7)] {
        for m in [Mode::Safe, Mode::AllowMultipleDocuments] {
            let t = best_time(doc, m);
            assert!(t.as_secs_f64() < budget, "{name} {m:?}: {t:?}");
        }
    }
}

/// `check-yaml -m s1.yaml … s6.yaml` under both oracles writes exactly `tests/vectors/yaml_warnings.txt` to stderr
/// (site-packages path replaced by `SP`; identical in 3.12 and 3.13): every `compose_node` call site, a message
/// seen twice in one process shown once, `MantissaNoDotYAML1_1Warning` per distinct message.
#[test]
fn warnings_match_ruamel() {
    let files: [(&str, &str); 7] = [
        ("s1.yaml", "&x a: &x [&x 1, &x 2]\n"),
        ("s2.yaml", "&r [1, &r 2]\n"),
        ("s3.yaml", "a: &x 1\nb: &x 2\n"),
        ("s3.yaml", "a: &x 1\nb: &x 2\n"),
        (
            "s4.yaml",
            "%YAML 1.1\n---\n[!!float 1e5, !!float 1e5, !!float 1E5, 1e5]\n",
        ),
        ("s5.yaml", "%YAML 1.1\n---\n- !!float 1e5\n"),
        ("s6.yaml", "a: &k 1\n&k b: 2\n"),
    ];
    let want = include_str!("../../tests/vectors/yaml_warnings.txt");
    for v in [PyMinor::P312, PyMinor::P313] {
        let mut c = Checker::new(Mode::AllowMultipleDocuments, v);
        let mut got = String::new();
        for (name, text) in files {
            assert!(c.check_text(text, name).is_ok(), "{name}");
            for w in c.take_warnings() {
                got.push_str(&w.render("SP"));
            }
        }
        assert_eq!(got, want, "{v:?}");
    }
}
