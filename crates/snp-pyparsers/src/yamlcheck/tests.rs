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
