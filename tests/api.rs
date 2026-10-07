//! Tests of the public API added or changed in 0.3: warning context, custom
//! items, introspection, shared schemas, owned defaults, Python value
//! semantics and the error types.

use std::collections::HashSet;
use std::sync::Arc;

use cfgv::{
    BigInt, CheckFn, ConditionValue, Cx, Date, DateError, DateTime, LoadError, Map, MapItem,
    Schema, Set, TzOffset, ValidationError, Validator, Value, ValueType, apply_defaults,
    apply_defaults_owned, py, remove_defaults, remove_defaults_owned, validate, validate_with,
    value_map,
};

const fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn public_types_are_send_and_sync() {
    assert_send_sync::<Value>();
    assert_send_sync::<Map>();
    assert_send_sync::<Set>();
    assert_send_sync::<BigInt>();
    assert_send_sync::<Date>();
    assert_send_sync::<DateTime>();
    assert_send_sync::<TzOffset>();
    assert_send_sync::<DateError>();
    assert_send_sync::<Schema>();
    assert_send_sync::<Validator>();
    assert_send_sync::<CheckFn>();
    assert_send_sync::<ConditionValue>();
    assert_send_sync::<ValidationError>();
    assert_send_sync::<LoadError>();
    assert_send_sync::<py::Utf8DecodeError>();
    assert_send_sync::<Arc<dyn MapItem>>();
}

fn warn_on(key: &'static str) -> Validator {
    Validator::custom(WarnIfPresent(Value::from(key)))
}

/// Emits `saw {key}` when the key is present.
#[derive(Debug)]
struct WarnIfPresent(Value);

impl MapItem for WarnIfPresent {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        if dct.contains_key(&self.0) {
            cx.warn(format!("saw {}", self.0));
        }
        Ok(())
    }
}

fn warning_schema() -> Schema {
    let inner = Schema::map(
        "Inner",
        None,
        vec![
            warn_on("x"),
            Validator::required("x", CheckFn::check_int()),
            warn_on("y"),
        ],
    );
    Schema::map(
        "Outer",
        None,
        vec![
            warn_on("a"),
            Validator::optional_recurse("inner", inner, value_map! {}),
            warn_on("b"),
        ],
    )
}

#[test]
fn warnings_are_collected_in_validation_order() {
    let schema = warning_schema();
    let v = value_map! {"b" => 1, "a" => 1, "inner" => value_map! {"y" => 1, "x" => 2}};
    let mut warnings = Vec::new();
    validate_with(&v, &schema, &mut Cx::collect(&mut warnings)).unwrap();
    assert_eq!(warnings, ["saw a", "saw x", "saw y", "saw b"]);
}

#[test]
fn warnings_before_the_first_error_are_kept() {
    let schema = warning_schema();
    let v = value_map! {"a" => 1, "b" => 1, "inner" => value_map! {"x" => "no", "y" => 1}};
    let mut seen = Vec::new();
    let mut sink = |w: String| seen.push(w);
    let err = validate_with(&v, &schema, &mut Cx::new(&mut sink)).unwrap_err();
    assert_eq!(
        err.trace_parts(),
        [
            "At Outer()",
            "At key: inner",
            "At Inner()",
            "At key: x",
            "Expected int got str"
        ]
    );
    assert_eq!(seen, ["saw a", "saw x"]);
    // `validate` discards warnings.
    assert!(validate(&value_map! {"a" => 1}, &schema).is_ok());
    let mut cx = Cx::discard();
    cx.warn("dropped");
    assert_eq!(format!("{cx:?}"), "Cx { sink: \"discard\" }");
    assert_eq!(format!("{:?}", Cx::default()), "Cx { sink: \"discard\" }");
}

#[test]
fn warn_additional_keys_callback_receives_context() {
    let schema = Schema::map(
        "Config",
        None,
        vec![Validator::warn_additional_keys(
            vec!["repos".into()],
            |extra, keys, dct, cx| {
                let extra: Vec<String> = extra.iter().map(ToString::to_string).collect();
                cx.warn(format!(
                    "Unexpected key(s) present at root: {} ({} allowed, {} total)",
                    extra.join(", "),
                    keys.len(),
                    dct.len()
                ));
            },
        )],
    );
    let mut warnings = Vec::new();
    let v = value_map! {"zz" => 1, "repos" => 1, "aa" => 1, 10 => 1, 9 => 1};
    validate_with(&v, &schema, &mut Cx::collect(&mut warnings)).unwrap();
    assert_eq!(
        warnings,
        ["Unexpected key(s) present at root: 9, 10, aa, zz (1 allowed, 5 total)"]
    );
}

/// pre-commit's `NotAllowed`, extending a built-in like a Python subclass.
#[derive(Debug)]
struct NotAllowed(Validator);

impl MapItem for NotAllowed {
    fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        match self.0.key() {
            Some(key) if dct.contains_key(key) => Err(ValidationError::new(format!(
                "{} cannot be overridden",
                key.repr()
            ))),
            _ => Ok(()),
        }
    }

    fn key(&self) -> Option<&Value> {
        self.0.key()
    }

    fn check_fn(&self) -> Option<&CheckFn> {
        self.0.check_fn()
    }
}

#[test]
fn custom_items_and_introspection() {
    let manifest = Schema::map(
        "Hook",
        Some("id".into()),
        vec![
            Validator::required("id", CheckFn::check_string()),
            Validator::required("entry", CheckFn::check_string()),
            Validator::optional(
                "args",
                CheckFn::check_array(CheckFn::check_string()),
                Vec::<Value>::new(),
            ),
        ],
    );
    let items = manifest.items().unwrap();
    assert_eq!(items.len(), 3);
    let keys: Vec<String> = items
        .iter()
        .filter_map(Validator::key)
        .map(ToString::to_string)
        .collect();
    assert_eq!(keys, ["id", "entry", "args"]);
    assert!(items[0].check_fn().is_some());
    assert_eq!(items[2].default_value(), Some(&Value::List(Vec::new())));

    // Derive a meta-hook schema from the manifest items, like pre-commit.
    let meta_items: Vec<Validator> = items
        .iter()
        .map(|item| match item.key().and_then(Value::as_str) {
            Some("entry") => Validator::custom(NotAllowed(item.clone())),
            _ => item.clone(),
        })
        .collect();
    let meta = Schema::map("Hook", Some("id".into()), meta_items);
    let err = validate(&value_map! {"id" => "identity", "entry" => "x"}, &meta).unwrap_err();
    assert_eq!(
        err.to_string(),
        "\n==> At Hook(id='identity')\n=====> 'entry' cannot be overridden"
    );
    let custom = &meta.items().unwrap()[1];
    assert_eq!(custom.key(), Some(&Value::from("entry")));
    assert!(custom.check_fn().is_some());
    assert!(format!("{custom:?}").starts_with("Custom(NotAllowed("));
    assert_eq!(
        apply_defaults(&value_map! {"id" => "identity"}, &meta).repr(),
        "{'id': 'identity', 'args': []}"
    );
}

#[test]
fn sub_schemas_are_shared() {
    let hook = Arc::new(Schema::map(
        "Hook",
        Some("id".into()),
        vec![Validator::required("id", CheckFn::check_string())],
    ));
    let local = Validator::required_recurse("hooks", Schema::array(Arc::clone(&hook)));
    let remote = Validator::optional_recurse("hook", Arc::clone(&hook), value_map! {});
    assert!(Arc::ptr_eq(remote.schema().unwrap(), &hook));
    let Some(Schema::Array { of, .. }) = local.schema().map(AsRef::as_ref) else {
        panic!("expected an array schema");
    };
    assert!(Arc::ptr_eq(of, &hook));
    let config = Schema::map("Config", None, vec![local, remote]);
    let cloned = config.clone();
    assert!(Arc::ptr_eq(
        config.items().unwrap(),
        cloned.items().unwrap()
    ));
    assert_eq!(config.object_name(), Some("Config"));
    assert_eq!(Schema::array(config).object_name(), None);
}

#[test]
fn remove_defaults_preserves_key_order() {
    let schema = Schema::map(
        "M",
        None,
        vec![
            Validator::optional("a", CheckFn::check_any(), 0),
            Validator::optional("b", CheckFn::check_any(), 0),
        ],
    );
    let v = value_map! {"a" => 0, "b" => 1, "c" => 2, "d" => 3};
    assert_eq!(
        remove_defaults(&v, &schema).repr(),
        "{'b': 1, 'c': 2, 'd': 3}"
    );
    let v = value_map! {"d" => 3, "b" => 0, "a" => 1, "c" => 2};
    assert_eq!(
        remove_defaults_owned(v, &schema).repr(),
        "{'d': 3, 'a': 1, 'c': 2}"
    );
}

#[test]
fn defaults_never_replace_mismatched_data() {
    let map_schema = Schema::map(
        "M",
        None,
        vec![Validator::optional("a", CheckFn::check_any(), 0)],
    );
    let list = Value::List(vec![Value::from(1)]);
    assert_eq!(apply_defaults(&list, &map_schema), list);
    assert_eq!(remove_defaults(&list, &map_schema), list);
    let array = Schema::array(map_schema.clone());
    let map = value_map! {"a" => 1};
    assert_eq!(apply_defaults(&map, &array), map);
    let kv = Schema::key_value_map("KV", CheckFn::check_string(), map_schema);
    assert_eq!(apply_defaults_owned(Value::Null, &kv), Value::Null);
    // Tuples become lists, like Python's list comprehension.
    let tuple = Value::Tuple(vec![value_map! {}]);
    assert_eq!(apply_defaults_owned(tuple, &array).repr(), "[{'a': 0}]");
}

#[test]
fn python_numeric_semantics() {
    let mut keys: HashSet<Value> = HashSet::new();
    keys.insert(Value::from(1));
    assert!(keys.contains(&Value::Bool(true)));
    assert!(keys.contains(&Value::Float(1.0)));
    assert!(keys.contains(&Value::BigInt(BigInt::from(1))));
    let a = value_map! {"x" => 1, "y" => value_map! {"z" => 2.0}};
    let b = value_map! {"y" => value_map! {"z" => 2}, "x" => true};
    assert_eq!(a, b);
    let mut maps = HashSet::new();
    maps.insert(a);
    assert!(maps.contains(&b));

    // `check_int` accepts booleans and big integers, like isinstance.
    assert!(CheckFn::check_int().check(&Value::Bool(true)).is_ok());
    assert!(CheckFn::check_int().check(&Value::from(u64::MAX)).is_ok());
    assert!(CheckFn::check_int().check(&Value::Float(1.0)).is_err());
    // `datetime` is a subclass of `date`.
    let dt = DateTime::new(Date::new(2020, 1, 1).unwrap(), 0, 0, 0, 0, None).unwrap();
    assert!(ValueType::Date.matches(&Value::from(dt)));
    assert!(!ValueType::DateTime.matches(&Value::from(dt.date())));
    assert_eq!(
        CheckFn::check_type(ValueType::Map, None)
            .check(&Value::from(1))
            .unwrap_err()
            .message(),
        "Expected dict got int"
    );
    // Choices are sorted numerically and matched with Python equality.
    let one_of = CheckFn::check_one_of([10, 2, 1]);
    assert!(one_of.check(&Value::Float(2.0)).is_ok());
    assert_eq!(
        one_of.check(&Value::from(3)).unwrap_err().message(),
        "Expected one of 1, 2, 10 but got: 3"
    );
    // Conditions use Python equality as well.
    assert!(ConditionValue::Exact(Value::from(1)).matches(Some(&Value::Bool(true))));
    assert!(ConditionValue::In(vec![Value::from(0)]).matches(Some(&Value::Float(-0.0))));
}

#[test]
fn ensure_absent_uses_repr_and_tuple_syntax() {
    let schema = |cond| {
        Schema::map(
            "Repository",
            Some("repo".into()),
            vec![Validator::conditional_with_absent(
                "rev",
                CheckFn::check_string(),
                "repo",
                cond,
                true,
            )],
        )
    };
    let v = value_map! {"repo" => "local", "rev" => 1.0};
    let msg = |cond| {
        validate(&v, &schema(cond))
            .unwrap_err()
            .message()
            .to_owned()
    };
    assert_eq!(
        msg(ConditionValue::NotIn(vec!["local".into(), "meta".into()])),
        "Expected rev to be absent when repo is any of ('local', 'meta'), found rev: 1.0"
    );
    assert_eq!(
        msg(ConditionValue::In(vec!["x".into()])),
        "Expected rev to be absent when repo is not any of ('x',), found rev: 1.0"
    );
    assert_eq!(
        msg(ConditionValue::Not("local".into())),
        "Expected rev to be absent when repo is 'local', found rev: 1.0"
    );
    assert_eq!(
        msg(ConditionValue::Exact("it's".into())),
        "Expected rev to be absent when repo is not \"it's\", found rev: 1.0"
    );
}

#[test]
fn regex_checks() {
    let python_like = CheckFn::check_regex_with(|p| !p.contains("(?<!") || p.ends_with("bar"));
    assert!(python_like.check(&Value::from("(?<!foo)bar")).is_ok());
    assert_eq!(
        python_like.check(&Value::from(1)).unwrap_err().message(),
        "Expected string got int"
    );
    assert_eq!(format!("{python_like:?}"), "Regex(..)");
    #[cfg(feature = "regex")]
    {
        let builtin = CheckFn::check_regex();
        assert!(builtin.check(&Value::from("^a.*$")).is_ok());
        // The regex crate does not support look-behind; Python does.
        assert_eq!(
            builtin
                .check(&Value::from("(?<!foo)bar"))
                .unwrap_err()
                .message(),
            "'(?<!foo)bar' is not a valid python regex"
        );
        assert_eq!(
            builtin.check(&Value::from("foo\\")).unwrap_err().message(),
            "'foo\\\\' is not a valid python regex"
        );
    }
}

#[test]
fn check_array_accepts_tuples() {
    let check = CheckFn::check_array(CheckFn::check_string());
    assert!(check.check(&Value::Tuple(vec!["a".into()])).is_ok());
    assert_eq!(
        check.check(&Value::Set(Set::new())).unwrap_err().message(),
        "Expected array but got 'set'"
    );
}

#[test]
fn validation_error_api() {
    let err = ValidationError::new("leaf")
        .with_context("inner")
        .with_context("outer");
    assert_eq!(err.message(), "leaf");
    assert_eq!(err.contexts().collect::<Vec<_>>(), ["outer", "inner"]);
    assert_eq!(err.contexts().len(), 2);
    assert_eq!(err.contexts().rev().next(), Some("inner"));
    assert_eq!(err.to_string(), "\n==> outer\n==> inner\n=====> leaf");
    assert!(std::error::Error::source(&err).is_none());
    assert_eq!(err.clone(), err);

    let load: LoadError = err.clone().into();
    assert_eq!(load.to_string(), err.to_string());
    assert!(std::error::Error::source(&load).is_none());
    let io = LoadError::Io {
        path: "missing.yaml".into(),
        source: std::io::Error::from_raw_os_error(2),
    };
    #[cfg(unix)]
    assert_eq!(
        io.to_string(),
        "[Errno 2] No such file or directory: 'missing.yaml'"
    );
    assert!(std::error::Error::source(&io).is_some());
    let custom = LoadError::Io {
        path: "it's.yaml".into(),
        source: std::io::Error::other("boom"),
    };
    assert_eq!(custom.to_string(), "boom: \"it's.yaml\"");
}

#[test]
fn value_conversions_and_accessors() {
    assert_eq!(Value::from(u64::MAX).type_name(), "int");
    assert!(matches!(Value::from(u64::MAX), Value::BigInt(_)));
    assert_eq!(Value::from(5_u64), Value::Int(5));
    assert_eq!(Value::from(i128::MIN).repr(), i128::MIN.to_string());
    assert_eq!(Value::from(BigInt::from(7)), Value::Int(7));
    assert_eq!(Value::BigInt(BigInt::from(7)).as_int(), Some(7));
    assert_eq!(Value::from(None::<i32>), Value::Null);
    assert_eq!(Value::from(Some("x")).as_str(), Some("x"));
    assert_eq!(Value::from(1.5_f32).as_float(), Some(1.5));
    assert_eq!(Value::from(&b"ab"[..]).as_bytes(), Some(&b"ab"[..]));
    assert_eq!(
        Value::from(std::borrow::Cow::Borrowed("c"))
            .into_string()
            .as_deref(),
        Some("c")
    );
    let list: Value = (1..=3).map(Value::from).collect();
    assert_eq!(list.as_list().map(<[Value]>::len), Some(3));
    assert_eq!(list.as_array().map(<[Value]>::len), Some(3));
    assert_eq!(Value::Tuple(Vec::new()).as_list(), None);
    let map: Value = [
        (Value::from("k"), Value::from(1)),
        (Value::from("k"), Value::from(2)),
    ]
    .into_iter()
    .collect();
    assert_eq!(map.repr(), "{'k': 2}");
    let mut map = map;
    map.as_map_mut().unwrap().insert("j".into(), Value::Null);
    assert_eq!(map.as_map().unwrap().len(), 2);
    assert!(Value::default().is_null());
    assert_eq!(value_map! {}.repr(), "{}");
    let date = Date::new(2020, 2, 29).unwrap();
    assert_eq!((date.year(), date.month(), date.day()), (2020, 2, 29));
    assert_eq!(Value::from(date).as_date(), Some(date));
    assert_eq!(Value::Set(Set::new()).as_set().map(Set::len), Some(0));
    assert_eq!(
        Value::from(DateTime::new(date, 1, 2, 3, 4, Some(TzOffset::UTC)).unwrap())
            .as_datetime()
            .and_then(DateTime::tz),
        Some(TzOffset::UTC)
    );
}
