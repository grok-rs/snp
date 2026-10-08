//! Decoding of the JSON test vectors produced by
//! `scripts/gen_parity_vectors.py` (see that script for the encoding).

#![allow(dead_code, unreachable_pub)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use cfgv::{
    BigInt, CheckFn, ConditionValue, Cx, Date, DateTime, Map, MapItem, RegexPattern, Schema,
    TzOffset, ValidationError, Validator, Value, ValueType, validate_context,
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
        "int" => match j.get("hex").and_then(Json::as_str) {
            Some(hex) => Value::from(BigInt::from_str_radix(hex, 16).expect("hex integer")),
            None => Value::from(
                str_field(j, "v")
                    .parse::<BigInt>()
                    .expect("decimal integer"),
            ),
        },
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

/// What Python's `re.compile` did with a pattern: compiled it, raised
/// `re.error`, or raised another exception (`"{type}: {message}"`).
#[derive(Debug, Clone)]
pub enum RegexVerdict {
    Valid(bool),
    Raised(String, String),
}

/// Builds cfgv schemas from the JSON specs, mirroring the generator's
/// `Spec` class.
pub struct SpecBuilder {
    /// Keyed by the `str` pattern, or `b:{hex}` for a `bytes` pattern.
    pub regex_valid: Arc<HashMap<String, RegexVerdict>>,
}

impl SpecBuilder {
    pub fn new(vectors: &Json) -> Self {
        let verdict = |v: &Json| match v {
            Json::Bool(b) => RegexVerdict::Valid(*b),
            Json::String(raised) => {
                let (tp, msg) = raised.split_once(": ").expect("type: message");
                RegexVerdict::Raised(tp.to_owned(), msg.to_owned())
            }
            other => panic!("bad regex verdict {other}"),
        };
        let regex_valid = vectors
            .get("regex_valid")
            .and_then(Json::as_object)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), verdict(v))).collect())
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
                    "" => "",
                    other => panic!("add {other:?} to the static type names"),
                });
                CheckFn::check_type(tp, name)
            }
            "one_of" => CheckFn::check_one_of(array(c, "values").iter().map(value)),
            "regex" => {
                let verdicts = Arc::clone(&self.regex_valid);
                CheckFn::check_regex_try_with(move |pattern| {
                    let key = match pattern {
                        RegexPattern::Str(s) => s.to_owned(),
                        RegexPattern::Bytes(b) => b.iter().fold("b:".to_owned(), |mut key, x| {
                            let _ = write!(key, "{x:02x}");
                            key
                        }),
                        _ => panic!("unknown pattern kind"),
                    };
                    match verdicts.get(&key) {
                        Some(RegexVerdict::Valid(valid)) => Ok(*valid),
                        Some(RegexVerdict::Raised(tp, msg)) => {
                            Err(ValidationError::uncaught(tp.clone(), msg.clone()))
                        }
                        None => panic!("no verdict for {key:?}"),
                    }
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

    fn condition(c: &Json) -> ConditionValue {
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
        let condition = || Self::condition(field(c, "condition"));
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
            "pc_stages_migration" => Validator::custom(PcStagesMigration { key: key() }),
            "pc_deprecated_stages_warning" => {
                Validator::custom(PcDeprecatedStagesWarning { key: key() })
            }
            "pc_language_migration_required" => Validator::custom(PcLanguageMigrationRequired {
                key: key(),
                check_fn: check(),
            }),
            "pc_warn_mutable_rev" => Validator::custom(PcWarnMutableRev {
                inner: Validator::conditional_with_absent(
                    key(),
                    CheckFn::check_string(),
                    condition_key(),
                    condition(),
                    true,
                ),
            }),
            "pc_sensible_regex" => Validator::custom(PcSensibleRegex {
                inner: Validator::optional_no_default(key(), CheckFn::check_string()),
            }),
            "pc_not_allowed" => Validator::custom(PcNotAllowed { key: key() }),
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

// --- pre-commit's custom map items (pre_commit/clientlib.py 4.6.2) ---------
//
// Mirrors of the Python classes in scripts/gen_parity_vectors.py.

const PC_STAGES: [&str; 4] = ["commit-msg", "manual", "pre-commit", "pre-push"];

fn transform_stage(stage: &Value) -> Value {
    match stage.as_str() {
        Some("commit") => Value::from("pre-commit"),
        Some("push") => Value::from("pre-push"),
        _ => stage.clone(),
    }
}

fn transform_stages(stages: &Value) -> Value {
    stages
        .as_array()
        .unwrap_or_default()
        .iter()
        .map(transform_stage)
        .collect()
}

fn translate_language(name: &Value) -> Value {
    match name.as_str() {
        Some("system") => Value::from("unsupported"),
        Some("script") => Value::from("unsupported_script"),
        _ => name.clone(),
    }
}

/// `StagesMigration(key, [])`.
#[derive(Debug)]
pub struct PcStagesMigration {
    pub key: Value,
}

impl MapItem for PcStagesMigration {
    fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        let Some(val) = dct.get(&self.key) else {
            return Ok(());
        };
        validate_context(
            || format!("At key: {}", self.key),
            || {
                CheckFn::check_array(CheckFn::check_any()).check(val)?;
                CheckFn::check_array(CheckFn::check_one_of(PC_STAGES)).check(&transform_stages(val))
            },
        )
    }

    fn apply_default(&self, dct: &mut Map) {
        let slot = dct
            .entry(self.key.clone())
            .or_insert_with(|| Value::List(Vec::new()));
        *slot = transform_stages(slot);
    }

    fn key(&self) -> Option<&Value> {
        Some(&self.key)
    }
}

/// `DeprecatedStagesWarning(key)`.
#[derive(Debug)]
pub struct PcDeprecatedStagesWarning {
    pub key: Value,
}

impl MapItem for PcDeprecatedStagesWarning {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        let Some(val) = dct.get(&self.key) else {
            return Ok(());
        };
        CheckFn::check_array(CheckFn::check_any()).check(val)?;
        let legacy: Vec<Value> = val
            .as_array()
            .unwrap_or_default()
            .iter()
            .filter(|s| matches!(s.as_str(), Some("commit" | "push")))
            .cloned()
            .collect();
        if !legacy.is_empty() {
            let id = dct
                .get(&Value::from("id"))
                .map(ToString::to_string)
                .unwrap_or_default();
            cx.warn(format!(
                "hook id `{id}` uses deprecated stage names ({}) which will be removed in a \
                 future version.  run: `pre-commit migrate-config` to automatically fix this.",
                join_str(&legacy)
            ));
        }
        Ok(())
    }
}

/// `LanguageMigrationRequired(key, check_fn)`.
#[derive(Debug)]
pub struct PcLanguageMigrationRequired {
    pub key: Value,
    pub check_fn: CheckFn,
}

impl MapItem for PcLanguageMigrationRequired {
    fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        let Some(val) = dct.get(&self.key) else {
            return Err(ValidationError::new(format!(
                "Missing required key: {}",
                self.key
            )));
        };
        validate_context(
            || format!("At key: {}", self.key),
            || self.check_fn.check(&translate_language(val)),
        )
    }

    fn apply_default(&self, dct: &mut Map) {
        if let Some(slot) = dct.get_mut(&self.key) {
            *slot = translate_language(slot);
        }
    }

    fn key(&self) -> Option<&Value> {
        Some(&self.key)
    }

    fn check_fn(&self) -> Option<&CheckFn> {
        Some(&self.check_fn)
    }
}

/// `WarnMutableRev(Conditional)`: a built-in validator extended like a
/// Python subclass.
#[derive(Debug)]
pub struct PcWarnMutableRev {
    pub inner: Validator,
}

impl MapItem for PcWarnMutableRev {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        self.inner.check(dct, cx)?;
        let Some(key) = self.inner.key() else {
            return Ok(());
        };
        if let Some(rev) = dct.get(key).and_then(Value::as_str) {
            // `re.match(r'^[a-fA-F0-9]+$', rev)`: `$` also matches before a
            // trailing newline.
            let body = rev.strip_suffix('\n').unwrap_or(rev);
            let hex = !body.is_empty() && body.bytes().all(|b| b.is_ascii_hexdigit());
            if !rev.contains('.') && !hex {
                let repo = dct
                    .get(&Value::from("repo"))
                    .map(Value::repr)
                    .unwrap_or_default();
                cx.warn(format!(
                    "The {} field of repo {repo} appears to be a mutable reference (moving tag / \
                     branch).  Mutable references are never updated after first install and \
                     are not supported.  See \
                     https://pre-commit.com/#using-the-latest-version-for-a-repository for more \
                     details.  Hint: `pre-commit autoupdate` often fixes this.",
                    key.repr()
                ));
            }
        }
        Ok(())
    }

    fn apply_default(&self, dct: &mut Map) {
        self.inner.apply_default(dct);
    }

    fn key(&self) -> Option<&Value> {
        self.inner.key()
    }

    fn check_fn(&self) -> Option<&CheckFn> {
        self.inner.check_fn()
    }
}

/// `OptionalSensibleRegexAtHook(OptionalNoDefault)`.
#[derive(Debug)]
pub struct PcSensibleRegex {
    pub inner: Validator,
}

impl MapItem for PcSensibleRegex {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        self.inner.check(dct, cx)?;
        let Some(key) = self.inner.key() else {
            return Ok(());
        };
        let pattern = dct.get(key).and_then(Value::as_str).unwrap_or_default();
        let id = dct
            .get(&Value::from("id"))
            .map_or_else(|| "None".to_owned(), Value::repr);
        if pattern.contains("/*") {
            cx.warn(format!(
                "The {} field in hook {id} is a regex, not a glob -- matching '/*' probably \
                 isn't what you want here",
                key.repr()
            ));
        }
        for fwd_slash_re in [r"[\\/]", r"[\/]", r"[/\\]"] {
            if pattern.contains(fwd_slash_re) {
                cx.warn(format!(
                    "pre-commit normalizes slashes in the {} field in hook {id} to forward \
                     slashes, so you can use / instead of {fwd_slash_re}",
                    key.repr()
                ));
            }
        }
        Ok(())
    }

    fn key(&self) -> Option<&Value> {
        self.inner.key()
    }

    fn check_fn(&self) -> Option<&CheckFn> {
        self.inner.check_fn()
    }
}

/// `NotAllowed(OptionalNoDefault)`.
#[derive(Debug)]
pub struct PcNotAllowed {
    pub key: Value,
}

impl MapItem for PcNotAllowed {
    fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        if dct.contains_key(&self.key) {
            return Err(ValidationError::new(format!(
                "{} cannot be overridden",
                self.key.repr()
            )));
        }
        Ok(())
    }

    fn key(&self) -> Option<&Value> {
        Some(&self.key)
    }
}
