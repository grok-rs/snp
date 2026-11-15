//! Lookups without a `Value` key: `StrKey` finds the entry the equal
//! `Value::String` finds.

#[cfg(test)]
mod probes {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hasher};

    use cfgv::{Map, Set, StrKey, Value};
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
}
