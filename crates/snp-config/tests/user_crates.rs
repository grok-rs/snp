//! ADR-061 drift check for cfgv 0.3 (`snp-support`, pinned by `deps.lock`): names every cfgv item of
//! ARCHITECTURE §1.5 with explicit types and exercises the behaviour snp-config's schemas rely on, so a
//! breaking change on the branch fails here (the CI drift job) instead of inside a later package.
//!
//! Owner: WP-00 (integration lead); changes only with a `deps.lock` bump.

#[cfg(test)]
mod cfgv_bindings {
    use std::path::Path;
    use std::sync::Arc;

    use cfgv::{
        BigInt, CheckFn, Cx, Date, DateTime, LoadError, Map, MapItem, RegexCheck, RegexPattern,
        Schema, Set, ValidationError, Validator, Value,
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
            Value::Map(Map::new()),
            Value::Set(Set::new()),
            Value::Date(date),
            Value::DateTime(datetime),
        ];
        has::<[Value; 13]>(values);
        has::<indexmap::IndexMap<Value, Value>>(Map::new());
        has::<indexmap::IndexSet<Value>>(Set::new());
        send_sync::<Value>();
        send_sync::<Schema>();
        send_sync::<CheckFn>();
        send_sync::<ValidationError>();
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
        let mut value = Map::new();
        value.insert(Value::from("other"), Value::Int(1));
        let value = Value::Map(value);
        let mut warnings = Vec::new();
        cfgv::validate_with(&value, &schema, &mut Cx::collect(&mut warnings)).unwrap();
        assert_eq!(warnings, ["Unexpected key(s) present: 1"]);
        has::<fn(&Value, &Schema, &mut Cx<'_>) -> Result<(), ValidationError>>(cfgv::validate_with);
        has::<fn(Value, &Schema) -> Value>(cfgv::apply_defaults_owned);
        let defaulted = cfgv::apply_defaults_owned(value, &schema);
        assert_eq!(defaulted.as_map().map(indexmap::IndexMap::len), Some(2));
        let mut entry = Map::new();
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
