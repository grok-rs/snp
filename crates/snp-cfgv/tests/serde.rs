//! Tests of the `serde` feature.
#![cfg(feature = "serde")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    reason = "test code: failing loudly is the point"
)]

use std::collections::BTreeMap;

use cfgv::{BigInt, Date, DateTime, DeserializeError, TzOffset, Value, from_value, value_map};
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Hook<'a> {
    id: &'a str,
    name: String,
    #[serde(default)]
    args: Vec<String>,
    always_run: bool,
    log_file: Option<String>,
    stages: Vec<Stage>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Stage {
    PreCommit,
    PrePush,
}

fn list<I: IntoIterator<Item = V>, V: Into<Value>>(items: I) -> Value {
    items.into_iter().map(Into::into).collect()
}

#[test]
fn deserialize_struct_borrowing_strings() {
    let value = value_map! {
        "id" => "black",
        "name" => "black",
        "args" => list(["--check", "-q"]),
        "always_run" => true,
        "log_file" => Value::Null,
        "stages" => list(["pre-commit", "pre-push"]),
    };
    let hook: Hook<'_> = from_value(&value).unwrap();
    assert_eq!(
        hook,
        Hook {
            id: "black",
            name: "black".into(),
            args: vec!["--check".into(), "-q".into()],
            always_run: true,
            log_file: None,
            stages: vec![Stage::PreCommit, Stage::PrePush],
        }
    );
}

#[test]
fn deserialize_errors_are_reported() {
    let value = value_map! {"id" => 1, "name" => "x", "always_run" => true, "stages" => list::<[&str; 0], &str>([])};
    let err: DeserializeError = from_value::<Hook<'_>>(&value).unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid type: integer `1`, expected a borrowed string"
    );
    let value =
        value_map! {"id" => "x", "name" => "x", "always_run" => true, "stages" => list(["commit"])};
    let err = from_value::<Hook<'_>>(&value).unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown variant `commit`, expected `pre-commit` or `pre-push`"
    );
}

#[derive(Debug, Deserialize, PartialEq)]
enum Shape {
    Unit,
    Newtype(u32),
    Tuple(u8, u8),
    Struct { x: i64 },
}

#[test]
fn deserialize_enums() {
    let shapes = list([
        Value::from("Unit"),
        value_map! {"Newtype" => 7},
        value_map! {"Tuple" => list([1, 2])},
        value_map! {"Struct" => value_map! {"x" => -3}},
        value_map! {"Unit" => Value::Null},
    ]);
    let parsed: Vec<Shape> = from_value(&shapes).unwrap();
    assert_eq!(
        parsed,
        [
            Shape::Unit,
            Shape::Newtype(7),
            Shape::Tuple(1, 2),
            Shape::Struct { x: -3 },
            Shape::Unit
        ]
    );
    assert!(from_value::<Shape>(&Value::from(1)).is_err());
    assert!(from_value::<Shape>(&value_map! {"Newtype" => 1, "Unit" => 2}).is_err());
}

#[test]
fn deserialize_scalars_and_containers() {
    assert_eq!(from_value::<Option<i32>>(&Value::Null).unwrap(), None);
    assert_eq!(from_value::<Option<i32>>(&Value::from(3)).unwrap(), Some(3));
    assert_eq!(from_value::<f64>(&Value::from(1.5)).unwrap(), 1.5);
    assert_eq!(from_value::<()>(&Value::Null).unwrap(), ());
    let big = Value::from(u64::MAX);
    assert!(matches!(big, Value::BigInt(_)));
    assert_eq!(from_value::<u64>(&big).unwrap(), u64::MAX);
    let huge = Value::BigInt("-100000000000000000000".parse::<BigInt>().unwrap());
    assert_eq!(
        from_value::<i128>(&huge).unwrap(),
        -100_000_000_000_000_000_000
    );
    let enormous = Value::BigInt("1".repeat(50).parse::<BigInt>().unwrap());
    assert_eq!(from_value::<String>(&enormous).unwrap(), "1".repeat(50));
    let tuple = Value::Tuple(vec![Value::from("a"), Value::from(1)]);
    assert_eq!(
        from_value::<(String, i64)>(&tuple).unwrap(),
        ("a".into(), 1)
    );
    let map = value_map! {"b" => 2, "a" => 1};
    let parsed: BTreeMap<String, u8> = from_value(&map).unwrap();
    assert_eq!(
        parsed.into_iter().collect::<Vec<_>>(),
        [("a".into(), 1), ("b".into(), 2)]
    );
    let date = Value::from(Date::new(2020, 1, 2).unwrap());
    assert_eq!(from_value::<String>(&date).unwrap(), "2020-01-02");
    let bytes = Value::Bytes(b"hi".to_vec());
    assert_eq!(
        from_value::<serde_bytes_like::Bytes>(&bytes).unwrap().0,
        b"hi"
    );
    assert!(from_value::<(String,)>(&list(["a", "b"])).is_err());
}

/// Minimal borrowed-bytes target, to avoid a `serde_bytes` dev-dependency.
mod serde_bytes_like {
    use serde::de::{Deserialize, Deserializer, Visitor};

    pub(crate) struct Bytes(pub(crate) Vec<u8>);

    impl<'de> Deserialize<'de> for Bytes {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = Bytes;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("bytes")
                }
                fn visit_bytes<E>(self, v: &[u8]) -> Result<Bytes, E> {
                    Ok(Bytes(v.to_vec()))
                }
            }
            d.deserialize_bytes(V)
        }
    }
}

#[test]
fn value_from_json() {
    let value: Value = serde_json::from_str(
        r#"{"a": [1, -2, 1.5, "x", null, true], "b": {"c": 18446744073709551615}, "a": 0}"#,
    )
    .unwrap();
    assert_eq!(value.repr(), "{'a': 0, 'b': {'c': 18446744073709551615}}");
    let value: Value = serde_json::from_str(r#"[{"k": "v"}, [], 1e3]"#).unwrap();
    assert_eq!(value.repr(), "[{'k': 'v'}, [], 1000.0]");
}

#[test]
fn value_to_json() {
    let tz = TzOffset::from_hours_minutes(false, 1, 0).unwrap();
    let dt = DateTime::new(Date::new(2020, 1, 1).unwrap(), 10, 0, 0, 0, Some(tz)).unwrap();
    let value = value_map! {
        "list" => list([Value::from(1), Value::Null, Value::from(false)]),
        "tuple" => Value::Tuple(vec![Value::from("x")]),
        "big" => Value::from(u64::MAX),
        "huge" => Value::BigInt("9".repeat(45).parse::<BigInt>().unwrap()),
        "float" => 0.5,
        "date" => dt,
        "set" => Value::Set([Value::from(2), Value::from(1)].into_iter().collect()),
    };
    assert_eq!(
        serde_json::to_string(&value).unwrap(),
        format!(
            r#"{{"list":[1,null,false],"tuple":["x"],"big":{},"huge":"{}","float":0.5,"date":"2020-01-01 10:00:00+01:00","set":[2,1]}}"#,
            u64::MAX,
            "9".repeat(45)
        )
    );
}

#[test]
fn json_round_trip() {
    let text = r#"{"repos":[{"repo":"local","hooks":[{"id":"x","args":["--a"],"n":1.25}]}],"fail_fast":false}"#;
    let value: Value = serde_json::from_str(text).unwrap();
    assert_eq!(serde_json::to_string(&value).unwrap(), text);
}

/// Regression: a struct failed to deserialize from a map with a non-string
/// key (YAML `true:`, `1:`, `null:`), although cfgv's `Map` schemas accept
/// such keys; unknown non-string keys are now ignored like unknown string
/// keys.
#[test]
fn non_string_keys_are_ignored_or_reported_by_their_python_str() {
    #[derive(Debug, Deserialize, PartialEq)]
    enum Flag {
        True,
    }
    #[derive(Debug, Deserialize, PartialEq)]
    struct Loose {
        id: String,
        #[serde(default)]
        args: Vec<String>,
    }
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Strict {
        id: String,
    }
    let keys = [
        Value::Bool(true),
        Value::from(1),
        Value::Null,
        Value::from(1.5),
        Value::Tuple(vec![Value::from("a")]),
        Value::from(&b"id"[..]),
    ];
    for key in keys {
        let mut map = cfgv::Map::new();
        map.insert("id".into(), "x".into());
        map.insert(key.clone(), "junk".into());
        let value = Value::Map(map);
        assert_eq!(
            from_value::<Loose>(&value).unwrap(),
            Loose {
                id: "x".into(),
                args: Vec::new()
            },
            "key {}",
            key.repr()
        );
        let err = from_value::<Strict>(&value).unwrap_err();
        assert!(
            err.to_string()
                .starts_with(&format!("unknown field `{key}`")),
            "{err}"
        );
    }
    // Enum variants are matched the same way.
    assert_eq!(from_value::<Flag>(&Value::Bool(true)).ok(), None);
    assert_eq!(
        from_value::<Flag>(&value_map! {true => Value::Null}).unwrap(),
        Flag::True
    );
}

/// Regression: `check_int` accepts booleans (Python's `bool` is an `int`),
/// but deserializing a validated boolean into an integer failed.
#[test]
fn integer_targets_accept_booleans_like_check_int() {
    #[derive(Debug, Deserialize, PartialEq)]
    struct Hook {
        priority: i32,
    }
    let check = cfgv::CheckFn::check_int();
    for b in [false, true] {
        let value = Value::Bool(b);
        assert!(check.check(&value).is_ok());
        assert_eq!(from_value::<i64>(&value).unwrap(), i64::from(b));
        assert_eq!(from_value::<u8>(&value).unwrap(), u8::from(b));
        assert_eq!(from_value::<i128>(&value).unwrap(), i128::from(b));
        assert_eq!(from_value::<u128>(&value).unwrap(), u128::from(b));
        assert_eq!(
            from_value::<Option<u32>>(&value).unwrap(),
            Some(u32::from(b))
        );
        assert_eq!(from_value::<bool>(&value).unwrap(), b);
    }
    // Floats follow `check_type(float)`, which rejects booleans.
    assert!(from_value::<f64>(&Value::Bool(true)).is_err());
    // Integers are still not booleans.
    assert!(from_value::<bool>(&Value::from(1)).is_err());
    assert_eq!(
        from_value::<Hook>(&value_map! {"priority" => true}).unwrap(),
        Hook { priority: 1 }
    );
}

#[test]
fn big_integers_beyond_i128() {
    let u = Value::from(u128::MAX);
    assert_eq!(from_value::<u128>(&u).unwrap(), u128::MAX);
    assert_eq!(serde_json::to_string(&u).unwrap(), u128::MAX.to_string());
    let huge = Value::from(BigInt::from_str_radix(&"f".repeat(4000), 16).unwrap());
    // Python's str() would raise; the text is the hexadecimal literal.
    let text = from_value::<String>(&huge).unwrap();
    assert_eq!(text.len(), 4002);
    assert!(text.starts_with("0xfff"));
}
