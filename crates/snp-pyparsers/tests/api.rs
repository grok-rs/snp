//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.11 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-17 (created by WP-00).

#[cfg(test)]
mod api {
    use snp_pyparsers::json::{self, DumpOpts, JsonValue, PyMinor};
    use snp_pyparsers::yamlcheck::{self, Mode};
    use snp_pyparsers::{difflib, ini, toml, xml};

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    #[test]
    fn send_sync_types() {
        send_sync::<PyMinor>();
        send_sync::<JsonValue>();
        send_sync::<DumpOpts>();
        send_sync::<Mode>();
    }

    #[test]
    fn json() {
        has::<[PyMinor; 2]>([PyMinor::P312, PyMinor::P313]);
        has::<JsonValue>(JsonValue::Object(vec![(
            String::from("k"),
            JsonValue::Array(vec![
                JsonValue::Null,
                JsonValue::Bool(true),
                JsonValue::Int("1".into()),
                JsonValue::Float(1.5),
                JsonValue::Str(String::new()),
            ]),
        )]));
        has::<DumpOpts>(DumpOpts {
            indent: String::from("  "),
            ensure_ascii: true,
            sort_keys: true,
            top_keys: Vec::new(),
        });
        has::<fn(&[u8], PyMinor) -> Result<(), String>>(json::check);
        has::<fn(&[u8], PyMinor) -> Result<JsonValue, String>>(json::loads);
        has::<fn(&JsonValue, &DumpOpts) -> String>(json::dumps_pretty);
    }

    #[test]
    fn checkers() {
        has::<[Mode; 3]>([Mode::Safe, Mode::AllowMultipleDocuments, Mode::Unsafe]);
        has::<fn(&str, Mode) -> Result<(), String>>(yamlcheck::check);
        has::<fn(&[u8]) -> Result<(), String>>(toml::check);
        has::<fn(&[u8]) -> Result<(), String>>(xml::check);
        has::<fn(&[&str], &[&str], &str, &str) -> String>(difflib::unified_diff);
        has::<fn(&str) -> Result<Vec<(String, String)>, String>>(ini::aws_credentials);
    }
}
