//! Byte-for-byte parity with Python cfgv 3.5.0 on CPython 3.12.
//!
//! The expected values in `tests/vectors/*.json` were produced by running
//! the real Python cfgv and CPython (`scripts/gen_parity_vectors.py`):
//!
//! * `repr.json`: `repr()`, `str()` and `type().__name__` of ~2200 values of
//!   every kind (curated and random strings over the whole Unicode range,
//!   random float bit patterns, big integers, bytes, containers, dates and
//!   time-zone aware date-times), plus the `str.isprintable()` table;
//! * `utf8.json`: `bytes.decode('utf-8')` results and `UnicodeDecodeError`
//!   texts;
//! * `messages.json`: `str(ValidationError)`, `apply_defaults` and
//!   `remove_defaults` results and warning callbacks for ~3400 schema cases
//!   covering every message format in `cfgv.py`, plus `load_from_filename`
//!   cases (missing file, directory, decoding errors, universal newlines,
//!   loader errors, nested validation errors, display names).

#[cfg(test)]
pub mod common;
#[cfg(test)]
mod parity {
    use super::common;

    use std::path::Path;

    use cfgv::{
        BigInt, Cx, LoadError, Map, ParseBigIntError, ValidationError, Value, apply_defaults,
        load_from_filename, py, remove_defaults, validate_with,
    };
    use common::{Mismatches, SpecBuilder, hex_bytes, load_vectors, value};
    use serde_json::Value as Json;

    fn cases<'a>(vectors: &'a Json, key: &str) -> &'a [Json] {
        vectors
            .get(key)
            .and_then(Json::as_array)
            .map_or(&[], Vec::as_slice)
    }

    fn text<'a>(j: &'a Json, key: &str) -> &'a str {
        j.get(key)
            .and_then(Json::as_str)
            .unwrap_or_else(|| panic!("{key:?} in {j}"))
    }

    #[test]
    fn repr_str_and_type_name_match_python() {
        let vectors = load_vectors("repr.json");
        let mut m = Mismatches::default();
        for case in cases(&vectors, "cases") {
            let v = value(&case["value"]);
            let what = || format!("{}", case["value"]);
            m.compare(what, &v.repr(), text(case, "repr"));
            m.compare(what, &v.to_string(), text(case, "str"));
            m.compare(what, v.type_name(), text(case, "type"));
            if let Value::String(s) = &v {
                m.compare(what, &py::repr_str(s), text(case, "repr"));
            }
            if let Value::Float(f) = v {
                m.compare(what, &py::repr_float(f), text(case, "repr"));
            }
            if let Value::Bytes(b) = &v {
                m.compare(what, &py::repr_bytes(b), text(case, "repr"));
            }
        }
        m.finish("repr");
    }

    #[test]
    fn isprintable_matches_python_for_every_code_point() {
        let vectors = load_vectors("repr.json");
        let ranges: Vec<(u32, u32)> = cases(&vectors, "nonprintable")
            .iter()
            .map(|r| {
                let lo = r[0]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .expect("u32");
                let hi = r[1]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .expect("u32");
                (lo, hi)
            })
            .collect();
        let mut mismatches = Vec::new();
        let mut next_range = ranges.iter().peekable();
        for c in (0..=0x10_ffff).filter_map(char::from_u32) {
            let cp = u32::from(c);
            while next_range.next_if(|&&(_, hi)| hi < cp).is_some() {}
            let python_printable = next_range.peek().is_none_or(|&&(lo, _)| cp < lo);
            if py::is_printable(c) != python_printable {
                mismatches.push(cp);
            }
        }
        assert!(
            mismatches.is_empty(),
            "isprintable differs for {mismatches:x?}"
        );
        assert_eq!(py::UNICODE_VERSION, vectors["meta"]["unicode"]);
    }

    #[test]
    fn utf8_decode_errors_match_python() {
        let vectors = load_vectors("utf8.json");
        let mut m = Mismatches::default();
        for case in cases(&vectors, "cases") {
            let bytes = hex_bytes(text(case, "hex"));
            let what = || format!("bytes {}", text(case, "hex"));
            let result = &case["result"];
            match py::decode_utf8(&bytes) {
                Ok(s) => m.compare(what, s, result["ok"].as_str().unwrap_or("<python failed>")),
                Err(e) => m.compare(
                    what,
                    &e.to_string(),
                    result["error"].as_str().unwrap_or("<python succeeded>"),
                ),
            }
        }
        m.finish("utf8");
    }

    #[test]
    fn validation_messages_defaults_and_warnings_match_python() {
        let vectors = load_vectors("messages.json");
        let builder = SpecBuilder::new(&vectors);
        let schemas: Vec<cfgv::Schema> = cases(&vectors, "schemas")
            .iter()
            .map(|s| builder.schema(s))
            .collect();
        let mut m = Mismatches::default();
        for case in cases(&vectors, "cases") {
            let name = text(case, "name");
            let index = case["schema"]
                .as_u64()
                .and_then(|i| usize::try_from(i).ok());
            let schema = &schemas[index.expect("schema index")];
            let v = value(&case["value"]);
            let mut warnings = Vec::new();
            let result = validate_with(&v, schema, &mut Cx::collect(&mut warnings));
            let expected = &case["result"];
            match result {
                // An exception other than ValidationError escaped Python cfgv.
                Err(err) if err.is_uncaught() => m.compare(
                    || format!("{name}: uncaught"),
                    &err.to_string(),
                    expected["uncaught"]
                        .as_str()
                        .unwrap_or("<python did not raise this exception>"),
                ),
                Err(err) => m.compare(
                    || format!("{name}: error"),
                    &err.to_string(),
                    expected["error"]
                        .as_str()
                        .unwrap_or("<python did not raise a ValidationError>"),
                ),
                Ok(()) => {
                    let applied = expected["applied"]
                        .as_str()
                        .unwrap_or("<python rejected the value>");
                    m.compare(
                        || format!("{name}: applied"),
                        &repr_or_raise(&apply_defaults(&v, schema)),
                        applied,
                    );
                    let removed = expected["removed"]
                        .as_str()
                        .unwrap_or("<python rejected the value>");
                    m.compare(
                        || format!("{name}: removed"),
                        &repr_or_raise(&remove_defaults(&v, schema)),
                        removed,
                    );
                }
            }
            let expected_warnings: Vec<&str> = cases(case, "warnings")
                .iter()
                .map(|w| w.as_str().expect("warning text"))
                .collect();
            m.compare(
                || format!("{name}: warnings"),
                &format!("{warnings:?}"),
                &format!("{expected_warnings:?}"),
            );
        }
        m.finish("messages");
    }

    /// The generator's `repr_or_raise`: `repr()`, or `!{type}: {message}` when
    /// CPython raises.
    fn repr_or_raise(v: &Value) -> String {
        v.try_repr()
            .unwrap_or_else(|e| format!("!{}", ValidationError::from(e)))
    }

    #[test]
    fn int_digit_limit_matches_python() {
        let vectors = load_vectors("repr.json");
        let limit = &vectors["int_limit"];
        let mut m = Mismatches::default();
        for case in cases(limit, "parse") {
            let input = text(case, "text");
            let base = case["base"]
                .as_u64()
                .and_then(|b| u32::try_from(b).ok())
                .expect("base");
            let what = || {
                format!(
                    "int({}... ({} chars), {base})",
                    input.get(..12).unwrap_or(input),
                    input.len()
                )
            };
            let result = &case["result"];
            match (BigInt::from_str_radix(input, base), result["hex"].as_str()) {
                (Ok(got), Some(hex)) => m.compare(what, &format!("{got:x}"), hex),
                (Ok(got), None) => m.compare(what, &format!("{got:x}"), "<python failed>"),
                (Err(err @ ParseBigIntError::ExceedsMaxStrDigits { .. }), _) => m.compare(
                    what,
                    &err.to_string(),
                    result["error"].as_str().unwrap_or("<python succeeded>"),
                ),
                // Other parse errors have Rust texts; Python must fail too.
                (Err(_), expected) => m.compare(
                    what,
                    "<failed>",
                    expected.map_or("<failed>", |_| "<python succeeded>"),
                ),
            }
        }
        for case in cases(limit, "str") {
            let hex = text(case, "hex");
            let big = BigInt::from_str_radix(hex, 16).expect("hex");
            let what = || format!("str(0x{}...)", hex.get(..12).unwrap_or(hex));
            let result = &case["result"];
            match big.to_decimal_string() {
                Ok(text) => m.compare(
                    what,
                    &text,
                    result["str"].as_str().unwrap_or("<python raised>"),
                ),
                Err(err) => m.compare(
                    what,
                    &err.to_string(),
                    result["error"].as_str().unwrap_or("<python succeeded>"),
                ),
            }
            m.compare(
                || format!("{} exceeds", what()),
                &big.exceeds_max_str_digits().to_string(),
                &result.get("error").is_some().to_string(),
            );
            let value = Value::from(big);
            match value.try_repr() {
                Ok(repr) => m.compare(
                    what,
                    &repr,
                    result["str"].as_str().unwrap_or("<python raised>"),
                ),
                Err(err) => m.compare(
                    what,
                    &err.to_string(),
                    result["error"].as_str().unwrap_or("<python succeeded>"),
                ),
            }
        }
        m.finish("int digit limit");
    }

    /// The generator's `kv_loader`: `key=value` lines.
    fn kv_loader(contents: &str) -> Result<Value, String> {
        let mut map = Map::default();
        for line in contents.split('\n') {
            if line.is_empty() {
                continue;
            }
            if line == "NESTED" {
                return Err(ValidationError::new("nested failure").to_string());
            }
            let Some((k, v)) = line.split_once('=') else {
                return Err(format!("bad line: {}", py::repr_str(line)));
            };
            map.insert(k.into(), v.into());
        }
        Ok(Value::Map(map))
    }

    #[test]
    fn load_from_filename_matches_python() {
        let vectors = load_vectors("messages.json");
        let builder = SpecBuilder::new(&vectors);
        let tmp = tempfile::tempdir().expect("temp dir");
        let tmp_str = tmp.path().to_str().expect("UTF-8 temp dir").to_owned();
        std::fs::create_dir_all(tmp.path().join("a_directory")).expect("mkdir");
        let mut m = Mismatches::default();
        for case in cases(&vectors, "load") {
            let name = text(case, "name");
            let index = case["schema"]
                .as_u64()
                .and_then(|i| usize::try_from(i).ok());
            let schema_json = &cases(&vectors, "schemas")[index.expect("schema index")];
            let schema = builder.schema(schema_json);
            // Forward slashes on every platform, like the Python generator.
            let path_string = format!("{tmp_str}/{}", text(case, "file"));
            let path = Path::new(&path_string);
            if let Some(hex) = case["hex"].as_str() {
                std::fs::write(path, hex_bytes(hex)).expect("write case file");
            }
            let display = case["display"].as_str();
            let expected = &case["result"];
            match load_from_filename(path, &schema, kv_loader, display) {
                Ok(v) => m.compare(
                    || format!("{name}: applied"),
                    &v.repr(),
                    expected["applied"].as_str().unwrap_or("<python failed>"),
                ),
                Err(LoadError::Invalid(err)) => m.compare(
                    || format!("{name}: error"),
                    &err.to_string().replace(&tmp_str, "$TMP"),
                    expected["error"].as_str().unwrap_or("<python succeeded>"),
                ),
                Err(other) => panic!("{name}: unexpected {other}"),
            }
        }
        m.finish("load_from_filename");
    }

    #[test]
    fn load_from_filename_io_error_is_not_a_validation_error() {
        // Python lets the OSError raised by open() escape cfgv.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tmp = tempfile::tempdir().expect("temp dir");
            let path = tmp.path().join("secret.cfg");
            std::fs::write(&path, "name=x\n").expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
            if std::fs::read(&path).is_ok() {
                return; // running as root: permissions are not enforced
            }
            let schema = cfgv::Schema::map("Config", None, Vec::new());
            let err = load_from_filename(&path, &schema, kv_loader, None).expect_err("unreadable");
            let LoadError::Io { path: failed, .. } = &err else {
                panic!("expected an I/O error, got {err:?}");
            };
            assert_eq!(failed, &path);
            assert_eq!(
                err.to_string(),
                format!(
                    "[Errno 13] Permission denied: {}",
                    py::repr_str(&path.to_string_lossy())
                )
            );
        }
    }
}
