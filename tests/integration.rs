use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cfgv::*;

// Helper to extract the error trace parts for assertion
fn assert_trace(err: &ValidationError, expected: &[&str]) {
    let parts = err.trace_parts();
    assert_eq!(
        parts, expected,
        "Error trace mismatch.\nGot:      {parts:?}\nExpected: {expected:?}"
    );
}

// ---- ValidationError tests ----

#[test]
fn test_validation_error_simple_str() {
    let err = ValidationError::new("error msg");
    let s = format!("{err}");
    assert_eq!(s, "\n=====> error msg");
}

#[test]
fn test_validation_error_nested() {
    let inner = ValidationError::new("error msg");
    let mid = inner.with_context("At line 1");
    let outer = mid.with_context("In file foo");
    let s = format!("{outer}");
    assert_eq!(s, "\n==> In file foo\n==> At line 1\n=====> error msg");
}

// ---- check_one_of tests ----

#[test]
fn test_check_one_of() {
    let check = CheckFn::check_one_of(vec![Value::Int(1), Value::Int(2)]);
    let err = check.check(&Value::Int(3)).unwrap_err();
    assert_eq!(err.message(), "Expected one of 1, 2 but got: 3");
}

#[test]
fn test_check_one_of_ok() {
    let check = CheckFn::check_one_of(vec![Value::Int(1), Value::Int(2)]);
    check.check(&Value::Int(2)).unwrap();
}

// ---- check_regex tests ----

#[test]
fn test_check_regex() {
    let check = CheckFn::check_regex();
    let err = check.check(&Value::String("(".into())).unwrap_err();
    assert_eq!(err.message(), "'(' is not a valid python regex");
}

#[test]
fn test_check_regex_ok() {
    let check = CheckFn::check_regex();
    check.check(&Value::String("^$".into())).unwrap();
}

// ---- check_array tests ----

#[test]
fn test_check_array_failed_inner_check() {
    let check = CheckFn::check_array(CheckFn::check_bool());
    let val = Value::List(vec![Value::Bool(true), Value::Bool(false), Value::Int(5)]);
    let err = check.check(&val).unwrap_err();
    assert_trace(&err, &["At index 2", "Expected bool got int"]);
}

#[test]
fn test_check_array_ok() {
    let check = CheckFn::check_array(CheckFn::check_bool());
    let val = Value::List(vec![Value::Bool(true), Value::Bool(false)]);
    check.check(&val).unwrap();
}

// ---- check_and tests ----

#[test]
fn test_check_and() {
    let check = CheckFn::check_and(vec![
        CheckFn::Type(ValueType::String, None),
        CheckFn::check_regex(),
    ]);
    let err = check.check(&Value::Bool(true)).unwrap_err();
    assert_eq!(err.message(), "Expected str got bool");

    let err = check.check(&Value::String("(".into())).unwrap_err();
    assert_eq!(err.message(), "'(' is not a valid python regex");
}

#[test]
fn test_check_and_ok() {
    let check = CheckFn::check_and(vec![
        CheckFn::Type(ValueType::String, None),
        CheckFn::check_regex(),
    ]);
    check.check(&Value::String("^$".into())).unwrap();
}

// ---- ConditionValue (Not, NotIn, In) tests ----

#[test]
fn test_not() {
    let cond = ConditionValue::Not(Value::from("foo"));
    assert!(cond.matches(Some(&Value::from("bar"))));
    assert!(!cond.matches(Some(&Value::from("foo"))));
    assert!(!cond.matches(None)); // MISSING
}

#[test]
fn test_not_in() {
    let cond = ConditionValue::NotIn(vec![Value::from("baz"), Value::from("foo")]);
    assert!(cond.matches(Some(&Value::from("bar"))));
    assert!(!cond.matches(Some(&Value::from("foo"))));
    assert!(!cond.matches(None)); // MISSING
}

#[test]
fn test_in() {
    let cond = ConditionValue::In(vec![Value::from("baz"), Value::from("foo")]);
    assert!(!cond.matches(Some(&Value::from("bar"))));
    assert!(cond.matches(Some(&Value::from("foo"))));
    assert!(cond.matches(Some(&Value::from("baz"))));
    assert!(!cond.matches(None)); // MISSING
}

// ---- Schema helpers ----

fn trivial_array_schema() -> Schema {
    Schema::array(Schema::map("foo", Some(Value::from("id")), vec![]))
}

fn trivial_array_schema_nonempty() -> Schema {
    Schema::array_nonempty(Schema::map("foo", Some(Value::from("id")), vec![]))
}

fn map_required() -> Schema {
    Schema::map(
        "foo",
        Some(Value::from("key")),
        vec![Validator::required("key", CheckFn::check_bool())],
    )
}

fn map_optional() -> Schema {
    Schema::map(
        "foo",
        Some(Value::from("key")),
        vec![Validator::optional("key", CheckFn::check_bool(), false)],
    )
}

fn map_no_default() -> Schema {
    Schema::map(
        "foo",
        Some(Value::from("key")),
        vec![Validator::optional_no_default("key", CheckFn::check_bool())],
    )
}

fn map_no_id_key() -> Schema {
    Schema::map(
        "foo",
        None,
        vec![Validator::required("key", CheckFn::check_bool())],
    )
}

// ---- Array schema tests ----

#[test]
fn test_validate_top_level_array_not_an_array() {
    let err = validate(&value_map! {}, &trivial_array_schema()).unwrap_err();
    assert_eq!(err.message(), "Expected array but got 'dict'");
}

#[test]
fn test_validate_top_level_array_no_objects() {
    let err = validate(&Value::List(vec![]), &trivial_array_schema_nonempty()).unwrap_err();
    assert_eq!(err.message(), "Expected at least 1 'foo'");
}

#[test]
fn test_trivial_array_schema_ok_empty() {
    validate(&Value::List(vec![]), &trivial_array_schema()).unwrap();
}

#[test]
fn test_ok_both_types() {
    // tuple in Python = list in Rust, so we just test list
    validate(&Value::List(vec![value_map! {}]), &trivial_array_schema()).unwrap();
}

// ---- Map schema tests ----

#[test]
fn test_map_wrong_type() {
    let err = validate(&Value::List(vec![]), &map_required()).unwrap_err();
    assert_eq!(err.message(), "Expected a foo map but got a list");
}

#[test]
fn test_required_missing_key() {
    let err = validate(&value_map! {}, &map_required()).unwrap_err();
    assert_trace(&err, &["At foo(key=MISSING)", "Missing required key: key"]);
}

#[test]
fn test_map_value_wrong_type_required() {
    let err = validate(&value_map! { "key" => 5i64 }, &map_required()).unwrap_err();
    assert_trace(
        &err,
        &["At foo(key=5)", "At key: key", "Expected bool got int"],
    );
}

#[test]
fn test_map_value_wrong_type_optional() {
    let err = validate(&value_map! { "key" => 5i64 }, &map_optional()).unwrap_err();
    assert_trace(
        &err,
        &["At foo(key=5)", "At key: key", "Expected bool got int"],
    );
}

#[test]
fn test_map_value_wrong_type_no_default() {
    let err = validate(&value_map! { "key" => 5i64 }, &map_no_default()).unwrap_err();
    assert_trace(
        &err,
        &["At foo(key=5)", "At key: key", "Expected bool got int"],
    );
}

#[test]
fn test_map_value_correct_type() {
    validate(&value_map! { "key" => true }, &map_required()).unwrap();
    validate(&value_map! { "key" => true }, &map_optional()).unwrap();
    validate(&value_map! { "key" => true }, &map_no_default()).unwrap();
}

#[test]
fn test_optional_key_missing() {
    validate(&value_map! {}, &map_optional()).unwrap();
    validate(&value_map! {}, &map_no_default()).unwrap();
}

#[test]
fn test_error_message_no_id_key() {
    let err = validate(&value_map! { "key" => 5i64 }, &map_no_id_key()).unwrap_err();
    assert_trace(&err, &["At foo()", "At key: key", "Expected bool got int"]);
}

// ---- Conditional tests ----

fn map_conditional() -> Schema {
    Schema::map(
        "foo",
        Some(Value::from("key")),
        vec![Validator::conditional(
            "key2",
            CheckFn::check_bool(),
            "key",
            ConditionValue::Exact(Value::Bool(true)),
        )],
    )
}

fn map_conditional_not() -> Schema {
    Schema::map(
        "foo",
        Some(Value::from("key")),
        vec![Validator::conditional(
            "key2",
            CheckFn::check_bool(),
            "key",
            ConditionValue::Not(Value::Bool(false)),
        )],
    )
}

fn map_conditional_absent() -> Schema {
    Schema::map(
        "foo",
        Some(Value::from("key")),
        vec![Validator::conditional_with_absent(
            "key2",
            CheckFn::check_bool(),
            "key",
            ConditionValue::Exact(Value::Bool(true)),
            true,
        )],
    )
}

fn map_conditional_absent_not() -> Schema {
    Schema::map(
        "foo",
        Some(Value::from("key")),
        vec![Validator::conditional_with_absent(
            "key2",
            CheckFn::check_bool(),
            "key",
            ConditionValue::Not(Value::Bool(true)),
            true,
        )],
    )
}

fn map_conditional_absent_not_in() -> Schema {
    Schema::map(
        "foo",
        Some(Value::from("key")),
        vec![Validator::conditional_with_absent(
            "key2",
            CheckFn::check_bool(),
            "key",
            ConditionValue::NotIn(vec![Value::Int(1), Value::Int(2)]),
            true,
        )],
    )
}

fn map_conditional_absent_in() -> Schema {
    Schema::map(
        "foo",
        Some(Value::from("key")),
        vec![Validator::conditional_with_absent(
            "key2",
            CheckFn::check_bool(),
            "key",
            ConditionValue::In(vec![Value::Int(1), Value::Int(2)]),
            true,
        )],
    )
}

#[test]
fn test_ok_conditional_schemas() {
    for schema in [map_conditional(), map_conditional_not()] {
        // Conditional check passes, key2 is checked and passes
        validate(&value_map! { "key" => true, "key2" => true }, &schema).unwrap();
        // Conditional check fails, key2 is not checked
        validate(&value_map! { "key" => false, "key2" => "ohai" }, &schema).unwrap();
    }
}

#[test]
fn test_not_ok_conditional_schemas() {
    for schema in [map_conditional(), map_conditional_not()] {
        let err = validate(&value_map! { "key" => true, "key2" => 5i64 }, &schema).unwrap_err();
        assert_trace(
            &err,
            &["At foo(key=True)", "At key: key2", "Expected bool got int"],
        );
    }
}

#[test]
fn test_ensure_absent_conditional() {
    let err = validate(
        &value_map! { "key" => false, "key2" => true },
        &map_conditional_absent(),
    )
    .unwrap_err();
    assert_trace(
        &err,
        &[
            "At foo(key=False)",
            "Expected key2 to be absent when key is not True, found key2: True",
        ],
    );
}

#[test]
fn test_ensure_absent_conditional_not() {
    let err = validate(
        &value_map! { "key" => true, "key2" => true },
        &map_conditional_absent_not(),
    )
    .unwrap_err();
    assert_trace(
        &err,
        &[
            "At foo(key=True)",
            "Expected key2 to be absent when key is True, found key2: True",
        ],
    );
}

#[test]
fn test_ensure_absent_conditional_not_in() {
    let err = validate(
        &value_map! { "key" => 1i64, "key2" => true },
        &map_conditional_absent_not_in(),
    )
    .unwrap_err();
    assert_trace(
        &err,
        &[
            "At foo(key=1)",
            "Expected key2 to be absent when key is any of (1, 2), found key2: True",
        ],
    );
}

#[test]
fn test_ensure_absent_conditional_in() {
    let err = validate(
        &value_map! { "key" => 3i64, "key2" => true },
        &map_conditional_absent_in(),
    )
    .unwrap_err();
    assert_trace(
        &err,
        &[
            "At foo(key=3)",
            "Expected key2 to be absent when key is not any of (1, 2), found key2: True",
        ],
    );
}

#[test]
fn test_no_error_conditional_absent() {
    validate(&value_map! {}, &map_conditional_absent()).unwrap();
    validate(&value_map! {}, &map_conditional_absent_not()).unwrap();
    validate(&value_map! { "key2" => true }, &map_conditional_absent()).unwrap();
    validate(
        &value_map! { "key2" => true },
        &map_conditional_absent_not(),
    )
    .unwrap();
}

// ---- apply_defaults tests ----

#[test]
fn test_apply_defaults_copies_object() {
    let val = value_map! {};
    let ret = apply_defaults(&val, &map_optional());
    assert_ne!(val, ret); // different because default was applied
}

#[test]
fn test_apply_defaults_sets_default() {
    let ret = apply_defaults(&value_map! {}, &map_optional());
    assert_eq!(ret, value_map! { "key" => false });
}

#[test]
fn test_apply_defaults_does_not_change_non_default() {
    let ret = apply_defaults(&value_map! { "key" => true }, &map_optional());
    assert_eq!(ret, value_map! { "key" => true });
}

#[test]
fn test_apply_defaults_does_nothing_on_non_optional() {
    let ret = apply_defaults(&value_map! {}, &map_required());
    assert_eq!(ret, value_map! {});
}

#[test]
fn test_apply_defaults_map_in_list() {
    let ret = apply_defaults(
        &Value::List(vec![value_map! {}]),
        &Schema::array(map_optional()),
    );
    assert_eq!(ret, Value::List(vec![value_map! { "key" => false }]));
}

// ---- remove_defaults tests ----

#[test]
fn test_remove_defaults_removes_defaults() {
    let ret = remove_defaults(&value_map! { "key" => false }, &map_optional());
    assert_eq!(ret, value_map! {});
}

#[test]
fn test_remove_defaults_nothing_to_remove() {
    let ret = remove_defaults(&value_map! {}, &map_optional());
    assert_eq!(ret, value_map! {});
}

#[test]
fn test_remove_defaults_does_not_change_non_default() {
    let ret = remove_defaults(&value_map! { "key" => true }, &map_optional());
    assert_eq!(ret, value_map! { "key" => true });
}

#[test]
fn test_remove_defaults_map_in_list() {
    let ret = remove_defaults(
        &Value::List(vec![value_map! { "key" => false }]),
        &Schema::array(map_optional()),
    );
    assert_eq!(ret, Value::List(vec![value_map! {}]));
}

#[test]
fn test_remove_defaults_does_nothing_on_non_optional() {
    let ret = remove_defaults(&value_map! { "key" => true }, &map_required());
    assert_eq!(ret, value_map! { "key" => true });
}

// ---- Nested schema tests ----

fn nested_schema_required() -> Schema {
    Schema::map(
        "Repository",
        Some(Value::from("repo")),
        vec![
            Validator::required("repo", CheckFn::Any),
            Validator::required_recurse("hooks", Schema::array(map_required())),
        ],
    )
}

fn nested_schema_optional() -> Schema {
    Schema::map(
        "Repository",
        Some(Value::from("repo")),
        vec![
            Validator::required("repo", CheckFn::Any),
            Validator::required_recurse("hooks", Schema::array(map_optional())),
        ],
    )
}

#[test]
fn test_validate_failure_nested() {
    let val = value_map! {
        "repo" => 1i64,
        "hooks" => Value::List(vec![value_map! {}])
    };
    let err = validate(&val, &nested_schema_required()).unwrap_err();
    assert_trace(
        &err,
        &[
            "At Repository(repo=1)",
            "At key: hooks",
            "At foo(key=MISSING)",
            "Missing required key: key",
        ],
    );
}

#[test]
fn test_apply_defaults_nested() {
    let val = value_map! {
        "repo" => "repo1",
        "hooks" => Value::List(vec![value_map! {}])
    };
    let ret = apply_defaults(&val, &nested_schema_optional());
    assert_eq!(
        ret,
        value_map! {
            "repo" => "repo1",
            "hooks" => Value::List(vec![value_map! { "key" => false }])
        }
    );
}

#[test]
fn test_remove_defaults_nested() {
    let val = value_map! {
        "repo" => "repo1",
        "hooks" => Value::List(vec![value_map! { "key" => false }])
    };
    let ret = remove_defaults(&val, &nested_schema_optional());
    assert_eq!(
        ret,
        value_map! {
            "repo" => "repo1",
            "hooks" => Value::List(vec![value_map! {}])
        }
    );
}

// ---- OptionalRecurse tests ----

fn optional_nested_schema() -> Schema {
    let link = Schema::map(
        "Link",
        Some(Value::from("key")),
        vec![Validator::required("key", CheckFn::check_bool())],
    );
    Schema::map(
        "Config",
        None,
        vec![Validator::optional_recurse(
            "links",
            Schema::array(link),
            Value::List(vec![]),
        )],
    )
}

#[test]
fn test_validate_failure_optional_recurse() {
    let val = value_map! {
        "links" => Value::List(vec![value_map! {}])
    };
    let err = validate(&val, &optional_nested_schema()).unwrap_err();
    assert_trace(
        &err,
        &[
            "At Config()",
            "At key: links",
            "At Link(key=MISSING)",
            "Missing required key: key",
        ],
    );
}

#[test]
fn test_optional_recurse_ok_missing() {
    validate(&value_map! {}, &optional_nested_schema()).unwrap();
}

#[test]
fn test_apply_defaults_optional_recurse_missing() {
    let ret = apply_defaults(&value_map! {}, &optional_nested_schema());
    assert_eq!(ret, value_map! { "links" => Value::List(vec![]) });
}

#[test]
fn test_apply_defaults_optional_recurse_already_present() {
    let val = value_map! {
        "links" => Value::List(vec![value_map! { "key" => true }])
    };
    let ret = apply_defaults(&val, &optional_nested_schema());
    assert_eq!(
        ret,
        value_map! {
            "links" => Value::List(vec![value_map! { "key" => true }])
        }
    );
}

#[test]
fn test_remove_defaults_optional_recurse_not_present() {
    assert_eq!(
        remove_defaults(&value_map! {}, &optional_nested_schema()),
        value_map! {}
    );
}

#[test]
fn test_remove_defaults_optional_recurse_present_at_default() {
    let val = value_map! { "links" => Value::List(vec![]) };
    assert_eq!(
        remove_defaults(&val, &optional_nested_schema()),
        value_map! {}
    );
}

#[test]
fn test_remove_defaults_optional_recurse_non_default() {
    let val = value_map! {
        "links" => Value::List(vec![value_map! { "key" => true }])
    };
    let ret = remove_defaults(&val, &optional_nested_schema());
    assert_eq!(
        ret,
        value_map! {
            "links" => Value::List(vec![value_map! { "key" => true }])
        }
    );
}

// ---- Optional nested Optional tests ----

fn optional_nested_optional_schema() -> Schema {
    let builder_opts = Schema::map(
        "BuilderOpts",
        None,
        vec![Validator::optional("noop", CheckFn::check_bool(), true)],
    );
    Schema::map(
        "Config",
        None,
        vec![Validator::optional_recurse(
            "builder",
            builder_opts,
            value_map! {},
        )],
    )
}

#[test]
fn test_optional_optional_apply_defaults() {
    let ret = apply_defaults(&value_map! {}, &optional_nested_optional_schema());
    assert_eq!(
        ret,
        value_map! { "builder" => value_map! { "noop" => true } }
    );
}

#[test]
fn test_optional_optional_remove_defaults() {
    let val = value_map! { "builder" => value_map! { "noop" => true } };
    let ret = remove_defaults(&val, &optional_nested_optional_schema());
    assert_eq!(ret, value_map! {});
}

// ---- ConditionalRecurse tests ----

fn conditional_nested_schema() -> Schema {
    let params1 = Schema::map(
        "Params1",
        None,
        vec![Validator::required("p1", CheckFn::check_bool())],
    );
    let params2 = Schema::map(
        "Params2",
        None,
        vec![Validator::required("p2", CheckFn::check_bool())],
    );
    Schema::map(
        "Config",
        None,
        vec![
            Validator::required("type", CheckFn::Any),
            Validator::conditional_recurse(
                "params",
                params1,
                "type",
                ConditionValue::Exact(Value::from("type1")),
            ),
            Validator::conditional_recurse(
                "params",
                params2,
                "type",
                ConditionValue::Exact(Value::from("type2")),
            ),
        ],
    )
}

#[test]
fn test_conditional_recurse_ok() {
    let vals = [
        value_map! { "type" => "type3" },
        value_map! { "type" => "type1", "params" => value_map! { "p1" => true } },
        value_map! { "type" => "type2", "params" => value_map! { "p2" => true } },
    ];
    for val in &vals {
        validate(val, &conditional_nested_schema()).unwrap();
    }
}

#[test]
fn test_conditional_recurse_error() {
    let val = value_map! {
        "type" => "type1",
        "params" => value_map! { "p2" => true }
    };
    let err = validate(&val, &conditional_nested_schema()).unwrap_err();
    assert_trace(
        &err,
        &[
            "At Config()",
            "At key: params",
            "At Params1()",
            "Missing required key: p1",
        ],
    );
}

// ---- ConditionalRecurse apply/remove defaults ----

fn conditional_recurse_schema() -> Schema {
    Schema::map(
        "Map",
        None,
        vec![
            Validator::required("t", CheckFn::check_bool()),
            Validator::conditional_recurse(
                "v",
                Schema::map(
                    "Inner",
                    Some(Value::from("k")),
                    vec![Validator::optional("k", CheckFn::check_bool(), true)],
                ),
                "t",
                ConditionValue::Exact(Value::Bool(true)),
            ),
            Validator::conditional_recurse(
                "v",
                Schema::map(
                    "Inner",
                    Some(Value::from("k")),
                    vec![Validator::optional("k", CheckFn::check_bool(), false)],
                ),
                "t",
                ConditionValue::Exact(Value::Bool(false)),
            ),
        ],
    )
}

#[test]
fn test_conditional_recurse_apply_defaults() {
    for tvalue in [true, false] {
        let val = value_map! { "t" => tvalue, "v" => value_map! {} };
        let ret = apply_defaults(&val, &conditional_recurse_schema());
        assert_eq!(
            ret,
            value_map! { "t" => tvalue, "v" => value_map! { "k" => tvalue } }
        );

        let val = value_map! { "t" => tvalue, "v" => value_map! { "k" => !tvalue } };
        let ret = apply_defaults(&val, &conditional_recurse_schema());
        assert_eq!(
            ret,
            value_map! { "t" => tvalue, "v" => value_map! { "k" => !tvalue } }
        );
    }
}

#[test]
fn test_conditional_recurse_remove_defaults() {
    for tvalue in [true, false] {
        let val = value_map! { "t" => tvalue, "v" => value_map! { "k" => tvalue } };
        let ret = remove_defaults(&val, &conditional_recurse_schema());
        assert_eq!(ret, value_map! { "t" => tvalue, "v" => value_map! {} });

        let val = value_map! { "t" => tvalue, "v" => value_map! { "k" => !tvalue } };
        let ret = remove_defaults(&val, &conditional_recurse_schema());
        assert_eq!(
            ret,
            value_map! { "t" => tvalue, "v" => value_map! { "k" => !tvalue } }
        );
    }
}

// ---- ConditionalOptional tests ----

fn conditional_optional_schema() -> Schema {
    Schema::map(
        "Map",
        None,
        vec![
            Validator::required("t", CheckFn::check_bool()),
            Validator::conditional_optional(
                "v",
                CheckFn::check_bool(),
                true,
                "t",
                ConditionValue::Exact(Value::Bool(true)),
            ),
            Validator::conditional_optional(
                "v",
                CheckFn::check_bool(),
                false,
                "t",
                ConditionValue::Exact(Value::Bool(false)),
            ),
        ],
    )
}

#[test]
fn test_conditional_optional_check() {
    for tvalue in [true, false] {
        let err = validate(
            &value_map! { "t" => tvalue, "v" => 2i64 },
            &conditional_optional_schema(),
        )
        .unwrap_err();
        assert_trace(&err, &["At Map()", "At key: v", "Expected bool got int"]);

        validate(
            &value_map! { "t" => tvalue, "v" => tvalue },
            &conditional_optional_schema(),
        )
        .unwrap();
    }
}

#[test]
fn test_conditional_optional_apply_default() {
    for tvalue in [true, false] {
        let ret = apply_defaults(
            &value_map! { "t" => tvalue },
            &conditional_optional_schema(),
        );
        assert_eq!(ret, value_map! { "t" => tvalue, "v" => tvalue });
    }
}

#[test]
fn test_conditional_optional_remove_default() {
    for tvalue in [true, false] {
        let ret = remove_defaults(
            &value_map! { "t" => tvalue, "v" => tvalue },
            &conditional_optional_schema(),
        );
        assert_eq!(ret, value_map! { "t" => tvalue });

        let ret = remove_defaults(
            &value_map! { "t" => tvalue, "v" => !tvalue },
            &conditional_optional_schema(),
        );
        assert_eq!(ret, value_map! { "t" => tvalue, "v" => !tvalue });
    }
}

// ---- NoAdditionalKeys tests ----

fn no_additional_keys_schema() -> Schema {
    Schema::map(
        "Map",
        None,
        vec![
            Validator::required(true, CheckFn::check_bool()),
            Validator::no_additional_keys(vec![Value::Bool(true)]),
        ],
    )
}

#[test]
fn test_no_additional_keys() {
    let err = validate(
        &value_map! { true => true, false => false },
        &no_additional_keys_schema(),
    )
    .unwrap_err();
    assert_trace(
        &err,
        &[
            "At Map()",
            "Additional keys found: False.  Only these keys are allowed: True",
        ],
    );

    validate(&value_map! { true => true }, &no_additional_keys_schema()).unwrap();
}

// ---- WarnAdditionalKeys tests ----

#[test]
fn test_warn_additional_keys_when_has_extra_keys() {
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = called.clone();

    let schema = Schema::map(
        "Map",
        None,
        vec![
            Validator::required(true, CheckFn::check_bool()),
            Validator::warn_additional_keys(vec![Value::Bool(true)], move |_extra, _keys, _dct, _cx| {
                called_clone.store(true, Ordering::SeqCst);
            }),
        ],
    );

    validate(&value_map! { true => true, false => false }, &schema).unwrap();
    assert!(called.load(Ordering::SeqCst));
}

#[test]
fn test_warn_additional_keys_when_no_extra_keys() {
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = called.clone();

    let schema = Schema::map(
        "Map",
        None,
        vec![
            Validator::required(true, CheckFn::check_bool()),
            Validator::warn_additional_keys(vec![Value::Bool(true)], move |_extra, _keys, _dct, _cx| {
                called_clone.store(true, Ordering::SeqCst);
            }),
        ],
    );

    validate(&value_map! { true => true }, &schema).unwrap();
    assert!(!called.load(Ordering::SeqCst));
}

// ---- KeyValueMap tests ----

fn key_value_map_schema() -> Schema {
    Schema::key_value_map(
        "Container",
        CheckFn::check_string(),
        Schema::map(
            "Object",
            Some(Value::from("name")),
            vec![
                Validator::required("name", CheckFn::check_string()),
                Validator::optional("setting", CheckFn::check_bool(), false),
            ],
        ),
    )
}

fn key_value_map_ints_schema() -> Schema {
    Schema::key_value_map(
        "Container",
        CheckFn::check_int(),
        Schema::array(Schema::map(
            "Object",
            Some(Value::from("nane")),
            vec![Validator::required("name", CheckFn::check_string())],
        )),
    )
}

#[test]
fn test_key_value_map_schema_ok() {
    validate(
        &value_map! {
            "hello" => value_map! { "name" => "hello" },
            "world" => value_map! { "name" => "world" }
        },
        &key_value_map_schema(),
    )
    .unwrap();

    validate(
        &value_map! {
            1i64 => Value::List(vec![value_map! { "name" => "hello" }]),
            2i64 => Value::List(vec![value_map! { "name" => "world" }])
        },
        &key_value_map_ints_schema(),
    )
    .unwrap();
}

#[test]
fn test_key_value_map_apply_defaults() {
    let orig = value_map! { "hello" => value_map! { "name" => "hello" } };
    let ret = apply_defaults(&orig, &key_value_map_schema());
    assert_eq!(
        orig,
        value_map! { "hello" => value_map! { "name" => "hello" } }
    );
    assert_eq!(
        ret,
        value_map! { "hello" => value_map! { "name" => "hello", "setting" => false } }
    );
}

#[test]
fn test_key_value_map_remove_defaults() {
    let orig = value_map! {
        "hello" => value_map! { "name" => "hello", "setting" => false }
    };
    let ret = remove_defaults(&orig, &key_value_map_schema());
    assert_eq!(
        orig,
        value_map! { "hello" => value_map! { "name" => "hello", "setting" => false } }
    );
    assert_eq!(
        ret,
        value_map! { "hello" => value_map! { "name" => "hello" } }
    );
}

#[test]
fn test_key_value_map_not_a_map() {
    let err = validate(&Value::List(vec![]), &key_value_map_schema()).unwrap_err();
    assert_trace(&err, &["Expected a Container map but got a list"]);
}

#[test]
fn test_key_value_map_wrong_key_type() {
    let val = value_map! {
        1i64 => value_map! { "name" => "hello" }
    };
    let err = validate(&val, &key_value_map_schema()).unwrap_err();
    assert_trace(
        &err,
        &["At Container()", "For key: 1", "Expected string got int"],
    );
}

#[test]
fn test_key_value_map_error_in_child_schema() {
    let val = value_map! {
        "hello" => value_map! { "name" => 1i64 }
    };
    let err = validate(&val, &key_value_map_schema()).unwrap_err();
    assert_trace(
        &err,
        &[
            "At Container()",
            "At key: hello",
            "At Object(name=1)",
            "At key: name",
            "Expected string got int",
        ],
    );
}

// ---- load_from_filename tests ----

fn json_to_value(v: serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Array(arr) => Value::List(arr.into_iter().map(json_to_value).collect()),
        serde_json::Value::Object(obj) => {
            let mut map = indexmap::IndexMap::new();
            for (k, v) in obj {
                map.insert(Value::String(k), json_to_value(v));
            }
            Value::Map(map)
        }
    }
}

fn json_load_strategy(contents: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let json_val: serde_json::Value = serde_json::from_str(contents)?;
    Ok(json_to_value(json_val))
}

#[test]
fn test_load_from_filename_file_does_not_exist() {
    let err = load_from_filename(
        std::path::Path::new("does_not_exist"),
        &map_required(),
        json_load_strategy,
        None,
    )
    .unwrap_err();
    assert_eq!(err.message(), "does_not_exist is not a file");
}

#[test]
fn test_load_from_filename_not_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("f");
    std::fs::create_dir(&sub).unwrap();
    let err = load_from_filename(&sub, &map_required(), json_load_strategy, None).unwrap_err();
    assert!(err.message().ends_with("is not a file"));
}

#[test]
fn test_load_from_filename_fails_load_strategy() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("foo.notjson");
    std::fs::write(&f, "totes not json").unwrap();
    let err = load_from_filename(&f, &map_required(), json_load_strategy, None).unwrap_err();
    // Should have "File ..." context
    assert!(err.contexts().next().unwrap().starts_with("File "));
}

#[test]
fn test_load_from_filename_validation_error() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("foo.json");
    std::fs::write(&f, "{}").unwrap();
    let err = load_from_filename(&f, &map_required(), json_load_strategy, None).unwrap_err();
    let parts = err.trace_parts();
    assert!(parts[0].starts_with("File "));
    assert_eq!(parts[1], "At foo(key=MISSING)");
    assert_eq!(parts[2], "Missing required key: key");
}

#[test]
fn test_load_from_filename_applies_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("foo.json");
    std::fs::write(&f, "{}").unwrap();
    let ret = load_from_filename(&f, &map_optional(), json_load_strategy, None).unwrap();
    assert_eq!(ret, value_map! { "key" => false });
}

#[test]
fn test_load_from_filename_custom_display_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("cfg.json");
    let err =
        load_from_filename(&f, &map_required(), json_load_strategy, Some("cfg.json")).unwrap_err();
    assert_trace(&err, &["cfg.json is not a file"]);
}

#[test]
fn test_load_from_filename_custom_display_error() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("cfg.json");
    std::fs::write(&f, "{}").unwrap();
    let err =
        load_from_filename(&f, &map_required(), json_load_strategy, Some("cfg.json")).unwrap_err();
    assert_trace(
        &err,
        &[
            "File cfg.json",
            "At foo(key=MISSING)",
            "Missing required key: key",
        ],
    );
}
