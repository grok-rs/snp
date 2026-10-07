//! Decoding of the JSON test vectors produced by
//! `scripts/gen_parity_vectors.py` (see that script for the encoding).

#![allow(dead_code, unreachable_pub)]

use std::collections::HashMap;
use std::sync::Arc;

use cfgv::{
    BigInt, CheckFn, ConditionValue, Date, DateTime, Map, Schema, TzOffset, ValidationError,
    Validator, Value, ValueType,
};
use serde_json::Value as Json;

/// Loads a vector file from `tests/vectors/`.
pub fn load_vectors(name: &str) -> Json {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("vectors")
        .join(name);
    let text = std::fs::read_to_string(&path).expect("vector file is readable");
    serde_json::from_str(&text).expect("vector file is valid JSON")
}

fn field<'a>(j: &'a Json, key: &str) -> &'a Json {
    j.get(key)
        .unwrap_or_else(|| panic!("missing field {key:?} in {j}"))
}

fn str_field<'a>(j: &'a Json, key: &str) -> &'a str {
    field(j, key)
        .as_str()
        .unwrap_or_else(|| panic!("{key:?} is not a string in {j}"))
}

fn array<'a>(j: &'a Json, key: &str) -> &'a [Json] {
    field(j, key)
        .as_array()
        .unwrap_or_else(|| panic!("{key:?} is not an array in {j}"))
}

fn uint(j: &Json) -> u32 {
    j.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or_else(|| panic!("not a small integer: {j}"))
}

pub fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
        .collect()
}

/// Decodes a tagged value.
pub fn value(j: &Json) -> Value {
    match str_field(j, "t") {
        "none" => Value::Null,
        "bool" => Value::Bool(field(j, "v").as_bool().expect("bool")),
        "int" => {
            let text = str_field(j, "v");
            Value::from(text.parse::<BigInt>().expect("decimal integer"))
        }
        "float" => {
            let bits = u64::from_str_radix(str_field(j, "bits"), 16).expect("hex bits");
            Value::Float(f64::from_bits(bits))
        }
        "str" => Value::String(str_field(j, "v").to_owned()),
        "bytes" => Value::Bytes(hex_bytes(str_field(j, "hex"))),
        "list" => Value::List(array(j, "v").iter().map(value).collect()),
        "tuple" => Value::Tuple(array(j, "v").iter().map(value).collect()),
        "set" => Value::Set(array(j, "v").iter().map(value).collect()),
        "dict" => {
            let mut map = Map::new();
            for pair in array(j, "v") {
                let pair = pair.as_array().expect("pair");
                map.insert(value(&pair[0]), value(&pair[1]));
            }
            Value::Map(map)
        }
        "date" => {
            let v = array(j, "v");
            let year = i64::from(uint(&v[0]));
            Value::Date(Date::new(year, uint(&v[1]), uint(&v[2])).expect("valid date"))
        }
        "datetime" => {
            let v = array(j, "v");
            let date = Date::new(i64::from(uint(&v[0])), uint(&v[1]), uint(&v[2])).expect("date");
            let tz = field(j, "tz")
                .as_i64()
                .map(|s| TzOffset::from_seconds(s).expect("valid offset"));
            Value::DateTime(
                DateTime::new(date, uint(&v[3]), uint(&v[4]), uint(&v[5]), uint(&v[6]), tz)
                    .expect("valid datetime"),
            )
        }
        other => panic!("unknown value tag {other:?}"),
    }
}

/// Builds cfgv schemas from the JSON specs, mirroring the generator's
/// `Spec` class.
pub struct SpecBuilder {
    pub regex_valid: Arc<HashMap<String, bool>>,
}

impl SpecBuilder {
    pub fn new(vectors: &Json) -> Self {
        let regex_valid = vectors
            .get("regex_valid")
            .and_then(Json::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_bool().expect("bool verdict")))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            regex_valid: Arc::new(regex_valid),
        }
    }

    pub fn check(&self, c: &Json) -> CheckFn {
        match str_field(c, "kind") {
            "any" => CheckFn::check_any(),
            "bool" => CheckFn::check_bool(),
            "int" => CheckFn::check_int(),
            "bytes" => CheckFn::check_bytes(),
            "string" => CheckFn::check_string(),
            "text" => CheckFn::check_text(),
            "type" => {
                let tp = match str_field(c, "type") {
                    "bool" => ValueType::Bool,
                    "int" => ValueType::Int,
                    "float" => ValueType::Float,
                    "str" => ValueType::String,
                    "bytes" => ValueType::Bytes,
                    "list" => ValueType::List,
                    "tuple" => ValueType::Tuple,
                    "dict" => ValueType::Map,
                    "set" => ValueType::Set,
                    "NoneType" => ValueType::Null,
                    "date" => ValueType::Date,
                    "datetime" => ValueType::DateTime,
                    other => panic!("unknown type {other:?}"),
                };
                let name = c.get("name").and_then(Json::as_str).map(|name| match name {
                    "mapping" => "mapping",
                    other => panic!("add {other:?} to the static type names"),
                });
                CheckFn::check_type(tp, name)
            }
            "one_of" => CheckFn::check_one_of(array(c, "values").iter().map(value)),
            "regex" => {
                let verdicts = Arc::clone(&self.regex_valid);
                CheckFn::check_regex_with(move |pattern| {
                    *verdicts
                        .get(pattern)
                        .unwrap_or_else(|| panic!("no verdict for {pattern:?}"))
                })
            }
            "array" => CheckFn::check_array(self.check(field(c, "inner"))),
            "and" => CheckFn::check_and(array(c, "fns").iter().map(|f| self.check(f))),
            "fail" => {
                let msg = str_field(c, "msg").to_owned();
                CheckFn::custom(move |_| Err(ValidationError::new(msg.clone())))
            }
            other => panic!("unknown check {other:?}"),
        }
    }

    fn condition(&self, c: &Json) -> ConditionValue {
        let values = || array(c, "values").iter().map(value).collect();
        match str_field(c, "kind") {
            "exact" => ConditionValue::Exact(value(field(c, "value"))),
            "not" => ConditionValue::Not(value(field(c, "value"))),
            "in" => ConditionValue::In(values()),
            "not_in" => ConditionValue::NotIn(values()),
            other => panic!("unknown condition {other:?}"),
        }
    }

    fn item(&self, c: &Json) -> Validator {
        let key = || value(field(c, "key"));
        let check = || self.check(field(c, "check"));
        let default = || value(field(c, "default"));
        let schema = || self.schema(field(c, "schema"));
        let condition_key = || value(field(c, "condition_key"));
        let condition = || self.condition(field(c, "condition"));
        let ensure_absent = || field(c, "ensure_absent").as_bool().expect("bool");
        let keys = || array(c, "keys").iter().map(value).collect::<Vec<_>>();
        match str_field(c, "kind") {
            "required" => Validator::required(key(), check()),
            "required_recurse" => Validator::required_recurse(key(), schema()),
            "optional" => Validator::optional(key(), check(), default()),
            "optional_recurse" => Validator::optional_recurse(key(), schema(), default()),
            "optional_no_default" => Validator::optional_no_default(key(), check()),
            "conditional" => Validator::conditional_with_absent(
                key(),
                check(),
                condition_key(),
                condition(),
                ensure_absent(),
            ),
            "conditional_optional" => Validator::conditional_optional_with_absent(
                key(),
                check(),
                default(),
                condition_key(),
                condition(),
                ensure_absent(),
            ),
            "conditional_recurse" => Validator::conditional_recurse_with_absent(
                key(),
                schema(),
                condition_key(),
                condition(),
                ensure_absent(),
            ),
            "no_additional_keys" => Validator::no_additional_keys(keys()),
            "warn_additional_keys" => {
                Validator::warn_additional_keys(keys(), |extra, keys, dct, cx| {
                    cx.warn(format!(
                        "extra: {} | keys: {} | size: {}",
                        join_str(extra),
                        join_str(keys),
                        dct.len()
                    ));
                })
            }
            other => panic!("unknown item {other:?}"),
        }
    }

    pub fn schema(&self, s: &Json) -> Schema {
        match str_field(s, "kind") {
            "map" => {
                let id_key = field(s, "id_key");
                let id_key = (!id_key.is_null()).then(|| value(id_key));
                let items: Vec<Validator> =
                    array(s, "items").iter().map(|i| self.item(i)).collect();
                Schema::map(str_field(s, "name"), id_key, items)
            }
            "array" => {
                let of = self.schema(field(s, "of"));
                if field(s, "allow_empty").as_bool().expect("bool") {
                    Schema::array(of)
                } else {
                    Schema::array_nonempty(of)
                }
            }
            "kvmap" => Schema::key_value_map(
                str_field(s, "name"),
                self.check(field(s, "check_key")),
                self.schema(field(s, "value_schema")),
            ),
            other => panic!("unknown schema {other:?}"),
        }
    }
}

pub fn join_str(values: &[Value]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Collects mismatches and fails with a readable report at the end.
#[derive(Default)]
pub struct Mismatches {
    items: Vec<String>,
    checked: usize,
}

impl Mismatches {
    pub fn compare(&mut self, what: impl FnOnce() -> String, got: &str, expected: &str) {
        self.checked += 1;
        if got != expected {
            self.items.push(format!(
                "{}\n    got:      {got:?}\n    expected: {expected:?}",
                what()
            ));
        }
    }

    pub fn finish(self, label: &str) {
        assert!(self.checked > 0, "{label}: nothing was checked");
        if !self.items.is_empty() {
            let shown: Vec<&str> = self.items.iter().take(25).map(String::as_str).collect();
            panic!(
                "{label}: {} of {} comparisons differ from Python:\n{}",
                self.items.len(),
                self.checked,
                shown.join("\n")
            );
        }
    }
}
