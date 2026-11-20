//! Lookups without a `Value` key, and the single-probe validators: `StrKey`
//! finds the entry the equal `Value::String` finds, and `apply_defaults`,
//! `remove_defaults` and `check` agree with reference models written with the
//! plain `contains_key` / `get` / `insert` probes of Python cfgv's code.

#[cfg(test)]
mod probes {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hasher};

    use cfgv::{
        CheckFn, ConditionValue, Cx, Map, Schema, Set, StrKey, Validator, Value, value_map,
    };
    use proptest::prelude::*;

    /// FNV-1a: a third, fixed-key hasher that is neither `SipHash` flavour.
    #[derive(Default)]
    struct Fnv(u64);

    impl Hasher for Fnv {
        fn finish(&self) -> u64 {
            self.0
        }

        fn write(&mut self, bytes: &[u8]) {
            for b in bytes {
                self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }

    fn hashes_agree<S: BuildHasher>(build: &S, s: &str) -> bool {
        build.hash_one(StrKey(s)) == build.hash_one(Value::from(s))
    }

    const NAMES: &[&str] = &[
        "id",
        "name",
        "repo",
        "hooks",
        "",
        "Id",
        "id ",
        "1",
        "True",
        "None",
        "é",
        "\u{1f600}",
    ];

    /// A key: a string, or a value of another type that spells a probe name.
    fn key() -> impl Strategy<Value = Value> {
        let name = || prop::sample::select(NAMES.to_vec());
        prop_oneof![
            4 => name().prop_map(Value::from),
            1 => name().prop_map(|s| Value::Bytes(s.as_bytes().to_vec())),
            1 => name().prop_map(|s| Value::Tuple(vec![Value::from(s)])),
            1 => name().prop_map(|s| Value::List(vec![Value::from(s)])),
            1 => (0_i64..3).prop_map(Value::Int),
            1 => any::<bool>().prop_map(Value::Bool),
            1 => Just(Value::Null),
            1 => Just(Value::Float(1.0)),
        ]
    }

    #[test]
    fn str_key_hashes_like_the_value_under_every_hasher() {
        let random = RandomState::new();
        let fixed = BuildHasherDefault::<DefaultHasher>::default();
        let fnv = BuildHasherDefault::<Fnv>::default();
        for s in ["", "id", "language_version", "é", "\u{1f600}x", "a\u{0}b"] {
            assert!(hashes_agree(&random, s), "{s:?}");
            assert!(hashes_agree(&fixed, s), "{s:?}");
            assert!(hashes_agree(&fnv, s), "{s:?}");
        }
    }

    /// Python: `b'id' != 'id'`, `('id',) != 'id'`, `1 != '1'`, `True != 'True'`.
    #[test]
    fn str_key_never_matches_a_non_string_key() {
        let mut m: Map = [
            (Value::Bytes(b"id".to_vec()), Value::from("bytes")),
            (Value::Tuple(vec![Value::from("id")]), Value::from("tuple")),
            (Value::Int(1), Value::from("int")),
            (Value::Bool(true), Value::from("bool")),
            (Value::Null, Value::from("null")),
        ]
        .into_iter()
        .collect();
        for k in ["id", "1", "True", "true", "None", "null", ""] {
            assert_eq!(m.get(&StrKey(k)), None, "{k:?}");
            assert!(!m.contains_key(&StrKey(k)), "{k:?}");
        }
        m.insert(Value::from("id"), Value::from("str"));
        assert_eq!(m.get(&StrKey("id")), Some(&Value::from("str")));
        // `True == 1`: the bool key replaced the int key's value, so the map holds 5 entries.
        assert_eq!(m.get_index_of(&StrKey("id")), Some(4));
        assert_eq!(Map::new().get(&StrKey("id")), None);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn str_key_hashes_like_any_string(s in any::<String>()) {
            prop_assert!(hashes_agree(&RandomState::new(), &s));
            prop_assert!(hashes_agree(&BuildHasherDefault::<DefaultHasher>::default(), &s));
            prop_assert!(hashes_agree(&BuildHasherDefault::<Fnv>::default(), &s));
        }

        /// The very entry of `m.get(&Value::from(k))`, or none when that finds none, in maps and sets of any mix
        /// and order of key types.
        #[test]
        fn str_key_finds_the_value_entry(entries in prop::collection::vec((key(), 0_i64..100), 0..24)) {
            let m: Map = entries.iter().map(|(k, v)| (k.clone(), Value::Int(*v))).collect();
            let s: Set = entries.into_iter().map(|(k, _)| k).collect();
            for k in NAMES {
                let same = match (m.get(&StrKey(k)), m.get(&Value::from(*k))) {
                    (Some(a), Some(b)) => std::ptr::eq(a, b),
                    (None, None) => true,
                    _ => false,
                };
                prop_assert!(same, "{k:?} in {m:?}");
                prop_assert_eq!(s.get_index_of(&StrKey(k)), s.get_index_of(&Value::from(*k)));
            }
        }
    }

    // --- Single-probe validators vs the double-probe reference --------------

    fn s(k: &str) -> Value {
        Value::from(k)
    }

    /// Every item kind that probes: optional, optional recurse (default then recurse), conditional optional,
    /// conditional recurse, required, conditional with `ensure_absent`.
    fn schema() -> Schema {
        let inner = || {
            Schema::map(
                "R",
                None,
                vec![Validator::optional("x", CheckFn::check_any(), 0)],
            )
        };
        Schema::map(
            "M",
            None,
            vec![
                Validator::optional("a", CheckFn::check_string(), "A"),
                Validator::optional_recurse("r", inner(), value_map! {}),
                Validator::conditional_optional(
                    "b",
                    CheckFn::check_string(),
                    "B",
                    "c",
                    ConditionValue::Exact(s("yes")),
                ),
                Validator::conditional_recurse("q", inner(), "c", ConditionValue::Not(s("no"))),
            ],
        )
    }

    fn check_schema() -> Schema {
        Schema::map(
            "M",
            None,
            vec![
                Validator::required("id", CheckFn::check_string()),
                Validator::conditional_with_absent(
                    "rev",
                    CheckFn::check_string(),
                    "repo",
                    ConditionValue::NotIn(vec![s("local"), s("meta")]),
                    true,
                ),
                Validator::conditional_optional_with_absent(
                    "b",
                    CheckFn::check_string(),
                    "B",
                    "c",
                    ConditionValue::In(vec![s("yes")]),
                    true,
                ),
            ],
        )
    }

    fn set_default(m: &mut Map, k: &str, default: Value) {
        if !m.contains_key(&s(k)) {
            m.insert(s(k), default);
        }
    }

    fn remove_if_default(m: &mut Map, k: &str, default: &Value) {
        if m.get(&s(k)) == Some(default) {
            m.shift_remove(&s(k));
        }
    }

    fn inner_ref(m: &mut Map, k: &str, apply: bool) {
        if let Some(Value::Map(inner)) = m.get_mut(&s(k)) {
            if apply {
                set_default(inner, "x", Value::Int(0));
            } else {
                remove_if_default(inner, "x", &Value::Int(0));
            }
        }
    }

    /// `schema()`'s `apply_defaults` / `remove_defaults`, written as Python cfgv does it.
    fn reference(mut m: Map, apply: bool) -> Map {
        let cond_b = m.get(&s("c")) == Some(&s("yes"));
        if apply {
            set_default(&mut m, "a", s("A"));
            set_default(&mut m, "r", value_map! {});
            inner_ref(&mut m, "r", true);
            if cond_b {
                set_default(&mut m, "b", s("B"));
            }
        } else {
            remove_if_default(&mut m, "a", &s("A"));
            if m.contains_key(&s("r")) {
                inner_ref(&mut m, "r", false);
                remove_if_default(&mut m, "r", &value_map! {});
            }
            if cond_b {
                remove_if_default(&mut m, "b", &s("B"));
            }
        }
        if m.get(&s("c")).is_some_and(|c| *c != s("no")) {
            inner_ref(&mut m, "q", apply);
        }
        m
    }

    fn map_key() -> impl Strategy<Value = Value> {
        prop_oneof![
            4 => prop::sample::select(vec!["a", "r", "b", "c", "q", "x", "id", "rev", "repo"]).prop_map(s),
            1 => Just(Value::Bytes(b"a".to_vec())),
            1 => Just(Value::Int(1)),
            1 => Just(Value::Bool(true)),
        ]
    }

    fn map_value() -> impl Strategy<Value = Value> {
        prop_oneof![
            prop::sample::select(vec!["A", "B", "yes", "no", "local", "meta", "v"]).prop_map(s),
            Just(value_map! {}),
            Just(value_map! {"x" => 0}),
            Just(value_map! {"x" => 1, "y" => 0}),
            Just(Value::Int(0)),
            Just(Value::Null),
        ]
    }

    fn arb_map() -> impl Strategy<Value = Map> {
        prop::collection::vec((map_key(), map_value()), 0..10).prop_map(|e| e.into_iter().collect())
    }

    fn ordered(m: &Map) -> Vec<(&Value, &Value)> {
        m.iter().collect()
    }

    /// `check_schema()`'s error message, written as Python cfgv does it (`_check_required`, `_check_conditional`).
    fn check_reference(m: &Map) -> Option<String> {
        let leaf = |msg: String| Some(format!("\n==> At M()\n=====> {msg}"));
        let at = |k: &str, msg: &str| Some(format!("\n==> At M()\n==> At key: {k}\n=====> {msg}"));
        if !m.contains_key(&s("id")) {
            return leaf("Missing required key: id".to_owned());
        }
        if m.get(&s("id")).and_then(Value::as_str).is_none() {
            return at(
                "id",
                &format!("Expected string got {}", m[&s("id")].type_name()),
            );
        }
        let repo = m.get(&s("repo"));
        if ConditionValue::NotIn(vec![s("local"), s("meta")]).matches(repo) {
            if !m.contains_key(&s("rev")) {
                return leaf("Missing required key: rev".to_owned());
            }
            if m.get(&s("rev")).and_then(Value::as_str).is_none() {
                return at(
                    "rev",
                    &format!("Expected string got {}", m[&s("rev")].type_name()),
                );
            }
        } else if m.contains_key(&s("repo"))
            && let Some(found) = m.get(&s("rev"))
        {
            return leaf(format!(
                "Expected rev to be absent when repo is any of ('local', 'meta'), found rev: {}",
                found.repr()
            ));
        }
        let c = m.get(&s("c"));
        if ConditionValue::In(vec![s("yes")]).matches(c) {
            if let Some(b) = m.get(&s("b"))
                && b.as_str().is_none()
            {
                return at("b", &format!("Expected string got {}", b.type_name()));
            }
        } else if m.contains_key(&s("c"))
            && let Some(found) = m.get(&s("b"))
        {
            return leaf(format!(
                "Expected b to be absent when c is not any of ('yes',), found b: {}",
                found.repr()
            ));
        }
        None
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]

        /// Same entries in the same order (Python `setdefault` appends; `del` keeps the order of the rest).
        #[test]
        fn defaults_agree_with_the_reference(m in arb_map()) {
            let schema = schema();
            let applied = schema.apply_defaults_owned(Value::Map(m.clone()));
            let expected = reference(m.clone(), true);
            prop_assert_eq!(applied.as_map().map(ordered), Some(ordered(&expected)));
            let removed = schema.remove_defaults_owned(Value::Map(m.clone()));
            let expected = reference(m, false);
            prop_assert_eq!(removed.as_map().map(ordered), Some(ordered(&expected)));
        }

        /// Same verdict and byte-identical message (the missing-key error before the value's check, the
        /// `ensure_absent` branch only when the condition key is present).
        #[test]
        fn check_agrees_with_the_reference(m in arb_map()) {
            let got = check_schema()
                .check_with(&Value::Map(m.clone()), &mut Cx::discard())
                .err()
                .map(|e| e.to_string());
            prop_assert_eq!(got, check_reference(&m), "{:?}", m);
        }
    }
}
