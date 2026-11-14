//! Parity with Python identify: vectors in `tests/data/identify-vectors.json`
//! were produced by running identify itself (see
//! `xtask/parity/gen_vectors.py`).

#[cfg(test)]
mod parity {
    use std::collections::BTreeMap;
    use std::io::{self, Read};

    use file_identify::extensions::{EXTENSIONS, EXTENSIONS_NEED_BINARY_CHECK, NAMES};
    use file_identify::interpreters::INTERPRETERS;
    use file_identify::{
        ALL_TAGS, Classification, IDENTIFY_VERSION, Meta, TagSet, TagTable, classify, is_text,
        is_text_bytes, parse_shebang, parse_shebang_bytes, shlex_split, tags_from_filename,
        tags_from_interpreter,
    };
    use serde_json::Value;

    fn vectors() -> &'static Value {
        static DATA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        DATA.get_or_init(|| {
            let text = include_str!("data/identify-vectors.json");
            serde_json::from_str(text).expect("valid vector file")
        })
    }

    fn section(name: &str) -> &'static [Value] {
        vectors()[name]
            .as_array()
            .unwrap_or_else(|| panic!("missing section {name}"))
    }

    fn strings(v: &Value) -> Vec<String> {
        v.as_array()
            .expect("array")
            .iter()
            .map(|s| s.as_str().expect("string").to_owned())
            .collect()
    }

    fn names(tags: TagSet) -> Vec<String> {
        tags.names().map(str::to_owned).collect()
    }

    fn unhex(s: &str) -> Vec<u8> {
        let (pairs, rest) = s.as_bytes().as_chunks::<2>();
        assert!(rest.is_empty(), "odd-length hex {s:?}");
        pairs
            .iter()
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("hex"), 16).expect("hex")
            })
            .collect()
    }

    /// A reader that returns at most one byte per `read` call.
    struct OneByte<'a>(&'a [u8]);

    impl Read for OneByte<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match (self.0.split_first(), buf.first_mut()) {
                (Some((&b, rest)), Some(slot)) => {
                    *slot = b;
                    self.0 = rest;
                    Ok(1)
                }
                _ => Ok(0),
            }
        }
    }

    #[test]
    fn version_matches() {
        assert_eq!(vectors()["identify_version"], IDENTIFY_VERSION);
    }

    #[test]
    fn all_tags_match() {
        let expected = strings(&vectors()["all_tags"]);
        assert_eq!(expected.len(), 323);
        assert_eq!(ALL_TAGS.to_vec(), expected);
        assert_eq!(TagSet::ALL.len(), expected.len());
    }

    #[test]
    fn tables_match() {
        let tables = &vectors()["tables"];
        let check = |name: &str, table: &TagTable| {
            let expected: BTreeMap<String, Vec<String>> = tables[name]
                .as_object()
                .expect("object")
                .iter()
                .map(|(k, v)| (k.clone(), strings(v)))
                .collect();
            let actual: BTreeMap<String, Vec<String>> = table
                .iter()
                .map(|(k, v)| (k.to_owned(), names(v)))
                .collect();
            assert_eq!(actual, expected, "{name}");
        };
        check("EXTENSIONS", &EXTENSIONS);
        check(
            "EXTENSIONS_NEED_BINARY_CHECK",
            &EXTENSIONS_NEED_BINARY_CHECK,
        );
        check("NAMES", &NAMES);
        check("INTERPRETERS", &INTERPRETERS);
    }

    #[test]
    fn tags_from_filename_matches() {
        let cases = section("tags_from_filename");
        assert!(cases.len() > 3000);
        for case in cases {
            let name = case[0].as_str().expect("name");
            let posix = strings(&case[1]);
            let nt = case.get(2).map_or_else(|| posix.clone(), strings);
            let expected = if cfg!(windows) { nt } else { posix };
            assert_eq!(names(tags_from_filename(name)), expected, "{name:?}");
        }
    }

    #[test]
    fn tags_from_interpreter_matches() {
        for case in section("tags_from_interpreter") {
            let interp = case[0].as_str().expect("interpreter");
            assert_eq!(
                names(tags_from_interpreter(interp)),
                strings(&case[1]),
                "{interp:?}"
            );
        }
    }

    #[test]
    fn parse_shebang_matches() {
        let unknown_exec = match classify(Meta::regular(true), "unknown-name") {
            Classification::NeedsContent(p) => p,
            Classification::Done(_) => panic!("unknown executable needs content"),
        };
        for case in section("parse_shebang") {
            let input = unhex(case[0].as_str().expect("hex"));
            let expected = strings(&case[1]);
            let shown = String::from_utf8_lossy(&input);
            assert_eq!(parse_shebang_bytes(&input), expected, "{shown:?}");
            assert_eq!(
                parse_shebang(&input[..]).expect("in-memory"),
                expected,
                "{shown:?}"
            );
            assert_eq!(
                parse_shebang(OneByte(&input)).expect("in-memory"),
                expected,
                "one-byte reads {shown:?}"
            );
            // Through the classification path (head buffer + rest of file).
            let interp_tags = expected
                .first()
                .map_or(TagSet::EMPTY, |i| tags_from_interpreter(i));
            let encoding = if is_text_bytes(&input) {
                "text"
            } else {
                "binary"
            };
            let want = unknown_exec.tags() | interp_tags | encoding.parse::<TagSet>().expect("tag");
            assert_eq!(unknown_exec.finish_bytes(&input), want, "{shown:?}");
            assert_eq!(
                unknown_exec.finish(OneByte(&input)).expect("in-memory"),
                want,
                "{shown:?}"
            );
        }
    }

    #[test]
    fn shlex_split_matches() {
        for case in section("shlex_split") {
            let input = case[0].as_str().expect("input");
            match &case[1] {
                Value::Array(_) => assert_eq!(
                    shlex_split(input).ok(),
                    Some(strings(&case[1])),
                    "{input:?}"
                ),
                Value::Object(err) => {
                    let msg = err["error"].as_str().expect("message");
                    assert_eq!(
                        shlex_split(input).err().map(|e| e.to_string()).as_deref(),
                        Some(msg),
                        "{input:?}"
                    );
                }
                other => panic!("unexpected {other}"),
            }
        }
    }

    #[test]
    fn is_text_matches() {
        for case in section("is_text") {
            let input = unhex(case[0].as_str().expect("hex"));
            let expected = case[1].as_bool().expect("bool");
            assert_eq!(is_text_bytes(&input), expected, "{input:?}");
            assert_eq!(is_text(&input[..]).expect("in-memory"), expected);
            assert_eq!(is_text(OneByte(&input)).expect("in-memory"), expected);
        }
    }

    #[cfg(unix)]
    mod on_disk {
        use super::*;
        use file_identify::{
            ExecPolicy, IdentifyError, file_is_text, parse_shebang_from_file,
            parse_shebang_from_file_with, tags_from_meta, tags_from_metadata, tags_from_path,
        };
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        /// Whether this process bypasses permission checks (root); the vectors
        /// were generated unprivileged.
        fn privileged(dir: &std::path::Path) -> bool {
            let probe = dir.join("probe");
            fs::write(&probe, "x").expect("write");
            fs::set_permissions(&probe, fs::Permissions::from_mode(0o000)).expect("chmod");
            fs::File::open(&probe).is_ok()
        }

        fn error_kind(e: &IdentifyError) -> &'static str {
            match e {
                IdentifyError::PathNotFound { .. } => "ValueError",
                IdentifyError::IoError { source }
                    if source.kind() == io::ErrorKind::PermissionDenied =>
                {
                    "PermissionError"
                }
                _ => "OSError",
            }
        }

        #[test]
        fn tags_from_path_matches() {
            let contents = vectors()["path_contents"].as_object().expect("contents");
            let dir = tempfile::tempdir().expect("tempdir");
            let privileged = privileged(dir.path());
            let cases = section("tags_from_path");
            for (i, case) in cases.iter().enumerate() {
                let name = case["name"].as_str().expect("name");
                let mode = u32::try_from(case["mode"].as_u64().expect("mode")).expect("u32");
                if privileged && mode & 0o500 != 0o500 {
                    continue;
                }
                let content = unhex(
                    contents[case["content"].as_str().expect("key")]
                        .as_str()
                        .expect("hex"),
                );
                let sub = dir.path().join(i.to_string());
                fs::create_dir_all(&sub).expect("mkdir");
                let path = sub.join(name);
                fs::write(&path, &content).expect("write");
                fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");
                let ctx = format!("{name} mode {mode:o} content {:?}", case["content"]);

                let actual = tags_from_path(&path);
                match &case["tags"] {
                    Value::Array(_) => {
                        let expected = strings(&case["tags"]);
                        assert_eq!(
                            actual.as_ref().map(|t| names(*t)).ok(),
                            Some(expected.clone()),
                            "{ctx}"
                        );
                        // Same result from the caller's lstat metadata ...
                        let md = fs::symlink_metadata(&path).expect("lstat");
                        assert_eq!(
                            tags_from_metadata(&path, &md, ExecPolicy::PythonCompat)
                                .map(names)
                                .ok(),
                            Some(expected.clone()),
                            "{ctx}"
                        );
                        // ... from a pre-computed Meta (no lstat) ...
                        let meta = Meta::from_metadata(&md, ExecPolicy::PythonCompat);
                        assert_eq!(
                            tags_from_meta(&path, meta).map(names).ok(),
                            Some(expected.clone()),
                            "{ctx}"
                        );
                        // ... and from the pure classification + a reader.
                        let tags = match classify(meta, name) {
                            Classification::Done(t) => t,
                            Classification::NeedsContent(p) => p
                                .finish(fs::File::open(&path).expect("open"))
                                .expect("read"),
                        };
                        assert_eq!(names(tags), expected, "{ctx}");
                    }
                    Value::Object(err) => {
                        let e = actual.expect_err(&ctx);
                        assert_eq!(error_kind(&e), err["error"], "{ctx}");
                    }
                    other => panic!("unexpected {other}"),
                }

                match &case["shebang"] {
                    Value::Array(_) => {
                        assert_eq!(
                            parse_shebang_from_file(&path).ok(),
                            Some(strings(&case["shebang"])),
                            "{ctx}"
                        );
                        assert_eq!(
                            parse_shebang_from_file_with(&path, ExecPolicy::PythonCompat).ok(),
                            Some(strings(&case["shebang"])),
                            "{ctx}"
                        );
                    }
                    Value::Object(err) => {
                        let e = parse_shebang_from_file(&path).expect_err(&ctx);
                        assert_eq!(error_kind(&e), err["error"], "{ctx}");
                    }
                    other => panic!("unexpected {other}"),
                }

                if let Some(expected) = case["file_is_text"].as_bool() {
                    assert_eq!(file_is_text(&path).ok(), Some(expected), "{ctx}");
                }
            }
        }

        #[test]
        fn special_entries_match() {
            let dir = tempfile::tempdir().expect("tempdir");
            let d = dir.path();
            fs::create_dir_all(d.join("adir")).expect("mkdir");
            fs::create_dir_all(d.join("adir.py")).expect("mkdir");
            fs::write(d.join("target.py"), "x").expect("write");
            std::os::unix::fs::symlink("target.py", d.join("link.py")).expect("symlink");
            std::os::unix::fs::symlink("missing", d.join("broken")).expect("symlink");
            for case in section("tags_from_path_special") {
                let name = case[0].as_str().expect("name");
                let path = d.join(name);
                match &case[1] {
                    Value::Array(_) => assert_eq!(
                        tags_from_path(&path).map(names).ok(),
                        Some(strings(&case[1])),
                        "{name}"
                    ),
                    Value::Object(err) => {
                        let e = tags_from_path(&path).expect_err(name);
                        assert_eq!(error_kind(&e), err["error"]);
                        #[expect(
                            clippy::literal_string_with_formatting_args,
                            reason = "`{path}` is the vectors' placeholder, not a format argument"
                        )]
                        let message = err["message"]
                            .as_str()
                            .expect("message")
                            .replace("{path}", &path.display().to_string());
                        assert_eq!(e.to_string(), message);
                    }
                    other => panic!("unexpected {other}"),
                }
            }
        }
    }
}
