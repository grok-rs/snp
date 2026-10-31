//! `snp config dump` and `snp config schema` (ARCHITECTURE §9.3, 05 §18.2 S9).
//!
//! Owner: WP-21.

use cfgv::{Map, Value};

use crate::overlay::{marker_key, skip_groups_key};
use crate::{Config, messages};

/// The JSON form of a validated value: Python's `json.dumps` view of what `yaml_load` built (dates and date-times
/// in ISO format, bytes and big integers as their Python `repr`, non-finite floats as strings, sets as lists).
fn to_json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Int(i) => J::from(*i),
        Value::Float(f) => {
            serde_json::Number::from_f64(*f).map_or_else(|| J::String(v.to_string()), J::Number)
        }
        Value::String(s) => J::String(s.clone()),
        Value::List(items) | Value::Tuple(items) => J::Array(items.iter().map(to_json).collect()),
        Value::Set(items) => J::Array(items.iter().map(to_json).collect()),
        Value::Map(m) => J::Object(
            m.iter()
                .map(|(k, v)| {
                    (
                        k.try_to_string().unwrap_or_else(|_| k.to_string()),
                        to_json(v),
                    )
                })
                .collect(),
        ),
        _ => J::String(v.try_to_string().unwrap_or_else(|_| v.to_string())),
    }
}

/// The effective config (overlay-marked fields) as YAML, or JSON with `json`.
///
/// YAML is pre-commit's `yaml_dump` of the validated, defaulted config (`snp_yaml::emit::dump`); when a local
/// overlay was applied ([`crate::overlay::apply_root`]), a comment header names the file and every overridden
/// field. JSON carries the same information under the top-level key `_snp_local_overrides`.
pub fn effective_dump(cfg: &Config, json: bool) -> String {
    let mut raw = cfg.raw.clone();
    let marker = raw.as_map_mut().and_then(|m| {
        m.shift_remove(&skip_groups_key());
        m.shift_remove(&marker_key())
    });
    if json {
        let mut out = to_json(&raw);
        if let (Some(marker), serde_json::Value::Object(obj)) = (&marker, &mut out) {
            obj.insert("_snp_local_overrides".to_owned(), to_json(marker));
        }
        let mut s = serde_json::to_string_pretty(&out).unwrap_or_default();
        s.push('\n');
        return s;
    }
    let mut out = String::new();
    if let Some(m) = marker.as_ref().and_then(Value::as_map) {
        let path = m
            .get(&Value::from("path"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let fields: Vec<String> = m
            .get(&Value::from("fields"))
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        out.push_str(&messages::dump_overlay_header(path, &fields));
    }
    let raw = if raw.is_null() {
        Value::Map(Map::new())
    } else {
        raw
    };
    out.push_str(&snp_yaml::emit::dump(&raw));
    out
}

/// The JSON schema of the config, extension keys included (draft 2020-12).
pub fn json_schema() -> &'static str {
    include_str!("config.schema.json")
}
