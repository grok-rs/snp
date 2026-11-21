//! Contract test of the cfgv API snp-config binds (ARCHITECTURE §1.5): names every cfgv item of §1.5 with
//! explicit types and exercises the behaviour snp-config's schemas rely on, so a change to crates/snp-cfgv that
//! breaks one of these bindings fails here, next to its consumer, instead of somewhere inside snp-config's
//! schemas. (Until ADR-116 this was the ADR-061 drift check of the sibling cfgv-rs checkout pinned by `deps.lock`.)
//!
//! Owner: WP-00 (integration lead); changes together with the §1.5 bindings.

#[cfg(test)]
mod cfgv_bindings {
    use std::path::Path;
    use std::sync::Arc;

    use cfgv::{
        BigInt, CheckFn, Cx, Date, DateTime, HashedKey, LoadError, Map, MapItem, RegexCheck,
        RegexPattern, Schema, Set, StrKey, ValidationError, Validator, Value, ValueState,
    };

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// The shape of pre-commit's custom validators (`NotAllowed('entry')`, `WarnMutableRev`, …) as
    /// snp-config writes them: an `impl cfgv::MapItem`.
    #[derive(Debug)]
    struct NotAllowed(&'static str);

    impl MapItem for NotAllowed {
        fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
            if dct.contains_key(&Value::from(self.0)) {
                cx.warn(format!("{} seen", self.0));
                return Err(ValidationError::new(format!(
                    "{:?} cannot be overridden",
                    self.0
                )));
            }
            Ok(())
        }
    }

    /// Every `MapItem` method, through a trait object.
    fn map_item(item: &dyn MapItem, dct: &mut Map, cx: &mut Cx<'_>) {
        has::<Result<(), ValidationError>>(item.check(dct, cx));
        item.apply_default(dct);
        item.remove_default(dct);
        has::<Option<&Value>>(item.key());
        has::<Option<&CheckFn>>(item.check_fn());
    }

    /// `load_from_filename_with` with snp-config's argument types: a `&str` display name (the `OsStr`
    /// from `-c` is passed through `to_string_lossy`) and a loader returning a `Display` error.
    fn load(
        path: &Path,
        schema: &Schema,
        display: Option<&str>,
        cx: &mut Cx<'_>,
    ) -> Result<Value, LoadError> {
        cfgv::load_from_filename_with(
            path,
            schema,
            |text: &str| -> Result<Value, String> { Ok(Value::from(text)) },
            display,
            cx,
        )
    }

    #[test]
    fn value_variants() {
        let date: Date = Date::new(2024, 2, 29).unwrap();
        let datetime: DateTime = DateTime::new(date, 12, 0, 0, 0, None).unwrap();
        let values: [Value; 13] = [
            Value::Null,
            Value::Bool(true),
            Value::Int(1),
            Value::BigInt(BigInt::from(1_u128 << 100)),
            Value::Float(0.5),
            Value::String(String::from("s")),
            Value::Bytes(b"b".to_vec()),
            Value::List(vec![Value::Null]),
            Value::Tuple(Vec::new()),
            Value::Map(Map::default()),
            Value::Set(Set::default()),
            Value::Date(date),
            Value::DateTime(datetime),
        ];
        has::<[Value; 13]>(values);
        has::<indexmap::IndexMap<Value, Value, ValueState>>(Map::default());
        has::<indexmap::IndexSet<Value, ValueState>>(Set::default());
        send_sync::<Value>();
        send_sync::<Schema>();
        send_sync::<CheckFn>();
        send_sync::<ValidationError>();
    }

    /// `StrKey` (`PriorityAliases`, the dump marker, the rev-line rewrite): the entry of the equal `str` key, never a
    /// key of another type.
    #[test]
    fn str_key() {
        let mut m = Map::default();
        m.insert(Value::Bytes(b"repo".to_vec()), Value::Int(1));
        assert_eq!(m.get(&StrKey("repo")), None);
        m.insert(Value::from("repo"), Value::Int(2));
        let key: StrKey<'_> = StrKey("repo");
        assert_eq!(m.get(&key), Some(&Value::Int(2)));
        assert!(m.contains_key(&StrKey("repo")));
        let set: Set = std::iter::once(Value::from("a")).collect();
        assert!(set.contains(&StrKey("a")));
    }

    /// `HashedKey` (the snp-config items' keys) over `ValueState` maps (ADR-111): the same entry as a plain lookup,
    /// setdefault order, and the built-in item's key through `Validator::hashed_key`.
    #[test]
    fn hashed_key() {
        let mut m: Map = Map::with_capacity_and_hasher(2, ValueState);
        m.insert(Value::from("id"), Value::Int(1));
        let key: HashedKey = HashedKey::new("id");
        assert_eq!(key.get(&m), m.get(&Value::from("id")));
        assert_eq!(key.get_full(&m), Some((0, &Value::Int(1))));
        assert!(key.contained_in(&m));
        if let Some(v) = key.get_mut(&mut m) {
            *v = Value::Int(2);
        }
        let stages = HashedKey::new("stages");
        assert_eq!(stages.set_default(&mut m, &Value::Null), &Value::Null);
        let keys: Vec<&Value> = m.keys().collect();
        assert_eq!(keys, [&Value::from("id"), &Value::from("stages")]);
        assert_eq!(format!("At key: {key}"), "At key: id");
        let item = Validator::optional_no_default("files", CheckFn::check_string());
        assert_eq!(item.hashed_key().map(HashedKey::value), item.key());
        assert_eq!(Validator::custom(NotAllowed("entry")).hashed_key(), None);
    }

    #[test]
    fn cx() {
        let mut seen: Vec<String> = Vec::new();
        {
            let mut cx: Cx<'_> = Cx::new(|msg: String| seen.push(msg));
            cx.warn("one");
        }
        assert_eq!(seen, ["one"]);
        let mut collected: Vec<String> = Vec::new();
        {
            let mut cx: Cx<'_> = Cx::collect(&mut collected);
            cx.warn("two");
        }
        assert_eq!(collected, ["two"]);
        let mut cx: Cx<'_> = Cx::discard();
        cx.warn("dropped");
    }

    #[test]
    fn regex_checks() {
        // snp-config: `CheckFn::check_regex_with(snp_pyre::cfgv_is_valid)` (bool form), never called here.
        has::<CheckFn>(CheckFn::check_regex_with(snp_pyre::cfgv_is_valid));
        let check = CheckFn::check_regex_with(|p: &str| !p.ends_with('\\'));
        assert!(check.check(&Value::from("a\\Z")).is_ok());
        assert_eq!(
            check.check(&Value::from("foo\\")).unwrap_err().message(),
            r"'foo\\' is not a valid python regex",
        );
        // pre-commit's `check_string_regex = check_and(check_string, check_regex)`.
        let string_regex: CheckFn = CheckFn::check_and([
            CheckFn::check_string(),
            CheckFn::check_regex_with(|_: &str| true),
        ]);
        assert!(string_regex.check(&Value::Int(1)).is_err());
        // The general form (deliberately unused by snp-config, ADR-070 / D-CRASH-04).
        let general: RegexCheck =
            Arc::new(|p: RegexPattern<'_>| -> Result<bool, ValidationError> {
                match p {
                    RegexPattern::Str(s) => Ok(!s.is_empty()),
                    _ => Err(ValidationError::uncaught("TypeError", "bytes")),
                }
            });
        has::<RegexCheck>(general);
        has::<CheckFn>(CheckFn::check_regex_try_with(|_: RegexPattern<'_>| {
            Ok(true)
        }));
        has::<ValidationError>(ValidationError::uncaught("OverflowError", "too large"));
    }

    #[test]
    fn schemas_and_validation() {
        let schema = Schema::map(
            "Config",
            None,
            vec![
                Validator::custom(NotAllowed("entry")),
                Validator::optional("fail_fast", CheckFn::check_bool(), false),
                // extension keys are appended to the known-key list of pre-commit's WarnAdditionalKeys
                Validator::warn_additional_keys(
                    vec![Value::from("fail_fast"), Value::from("priority")],
                    |extra: &[Value], _keys: &[Value], _dct: &Map, cx: &mut Cx<'_>| {
                        cx.warn(format!("Unexpected key(s) present: {}", extra.len()));
                    },
                ),
            ],
        );
        let mut value = Map::default();
        value.insert(Value::from("other"), Value::Int(1));
        let value = Value::Map(value);
        let mut warnings = Vec::new();
        cfgv::validate_with(&value, &schema, &mut Cx::collect(&mut warnings)).unwrap();
        assert_eq!(warnings, ["Unexpected key(s) present: 1"]);
        has::<fn(&Value, &Schema, &mut Cx<'_>) -> Result<(), ValidationError>>(cfgv::validate_with);
        has::<fn(Value, &Schema) -> Value>(cfgv::apply_defaults_owned);
        let defaulted = cfgv::apply_defaults_owned(value, &schema);
        assert_eq!(defaulted.as_map().map(indexmap::IndexMap::len), Some(2));
        let mut entry = Map::default();
        entry.insert(Value::from("entry"), Value::from("x"));
        let err = cfgv::validate_with(&Value::Map(entry.clone()), &schema, &mut Cx::discard())
            .unwrap_err();
        assert!(err.to_string().ends_with("cannot be overridden"));
        has::<fn(&dyn MapItem, &mut Map, &mut Cx<'_>)>(map_item);
    }

    #[test]
    fn load_from_filename() {
        has::<fn(&Path, &Schema, Option<&str>, &mut Cx<'_>) -> Result<Value, LoadError>>(load);
        let schema = Schema::map("Config", None, Vec::<Validator>::new());
        let missing = Path::new("/nonexistent-snp-dir/.pre-commit-config.yaml");
        let err = load(missing, &schema, Some("cfg.yaml"), &mut Cx::discard()).unwrap_err();
        assert!(matches!(err, LoadError::Invalid(_)));
        assert!(err.to_string().ends_with("cfg.yaml is not a file"));
    }
}
