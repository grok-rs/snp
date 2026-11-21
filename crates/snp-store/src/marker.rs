//! The store's own small files: the `.snp-repo.json` commit marker, the `.snp-tools.json` sidecar,
//! `store-v1.json` and the `configs-v1` entries (ARCHITECTURE §5.2).
//!
//! These replace pre-commit's SQLite rows (`store.py:62-235`). Every parser here takes untrusted bytes (a corrupt,
//! truncated or hand-edited store) and returns `None` instead of failing: a malformed marker means "not there"
//! (self-healing, D-CRASH-17), never a crash.
//!
//! Owner: WP-22.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::keys::{RepoId, RepoKind, RepoMeta, ToolRef, hex};
use crate::maintenance::protect::unhex;

/// Upper bound of every store metadata file snp reads (1 MiB; real files are a few hundred bytes).
pub(crate) const META_CAP: u64 = 1024 * 1024;

/// A path as JSON: a string when it is valid Unicode, else `{"hex": "<bytes>"}` (Unix paths may be any bytes).
pub(crate) fn path_to_json(p: &Path) -> Value {
    match p.to_str() {
        Some(s) => Value::String(s.to_owned()),
        None => json!({ "hex": hex(p.as_os_str().as_encoded_bytes()) }),
    }
}

/// Inverse of [`path_to_json`].
pub(crate) fn path_from_json(v: &Value) -> Option<PathBuf> {
    match v {
        Value::String(s) => Some(PathBuf::from(s)),
        Value::Object(m) => {
            let bytes = unhex(m.get("hex")?.as_str()?.as_bytes())?;
            path_from_bytes(bytes)
        }
        _ => None,
    }
}

/// A path from raw bytes: the bytes on Unix; valid UTF-8 elsewhere.
#[cfg_attr(
    unix,
    expect(
        clippy::unnecessary_wraps,
        reason = "only Windows can reject the bytes"
    )
)]
pub(crate) fn path_from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes).ok().map(PathBuf::from)
    }
}

/// The bytes of a path for an on-disk record: raw on Unix, UTF-8 (lossy) elsewhere.
pub(crate) fn path_bytes(p: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        p.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        p.to_string_lossy().into_owned().into_bytes()
    }
}

/// Seconds since the epoch (0 when the clock is before it).
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `.snp-repo.json`: `{format, kind, repo_type, repo, rev, deps, object_format, created, snp_version}`.
pub(crate) fn encode_repo_meta(meta: &RepoMeta) -> Vec<u8> {
    let mut m = Map::new();
    m.insert("format".into(), json!(1));
    match &meta.kind {
        RepoKind::Remote { id, rev } => {
            m.insert("kind".into(), json!("remote"));
            match id {
                RepoId::Url(u) => {
                    m.insert("repo_type".into(), json!("url"));
                    m.insert("repo".into(), json!(&**u));
                }
                RepoId::LocalPath(p) => {
                    m.insert("repo_type".into(), json!("path"));
                    m.insert("repo".into(), path_to_json(p));
                }
            }
            m.insert("rev".into(), json!(&**rev));
        }
        RepoKind::Local => {
            m.insert("kind".into(), json!("local"));
            m.insert(
                "template_version".into(),
                json!(snp_core::profile::LOCAL_TEMPLATE_VERSION),
            );
        }
    }
    let deps: Vec<&str> = meta.deps.iter().map(|d| &**d).collect();
    m.insert("deps".into(), json!(deps));
    m.insert("object_format".into(), Value::Null);
    m.insert("created".into(), json!(now_secs()));
    m.insert("snp_version".into(), json!(snp_core::profile::SNP_VERSION));
    let mut out = serde_json::to_vec(&Value::Object(m)).unwrap_or_default();
    out.push(b'\n');
    out
}

/// Parses `.snp-repo.json`; `None` for anything malformed.
///
/// Typed, without a `serde_json::Value` tree for the whole object, and accepting exactly what
/// `serde_json::from_slice::<Value>` followed by the key lookups accepts (`decode_repo_meta_oracle`, the test
/// oracle): the input must be one JSON object (same parser, same nesting limit, every value validated as `Value`
/// would be, trailing characters refused); a duplicate key keeps its last value, as `Value`'s map does; the kept
/// values are type-checked after the whole object is read. `repo` still goes through [`path_from_json`] for a
/// local-path repository.
pub(crate) fn decode_repo_meta(bytes: &[u8]) -> Option<RepoMeta> {
    let f: meta_de::Fields<'_> = serde_json::from_slice(bytes).ok()?;
    let meta_de::Slot::U64(1) = f.format? else {
        return None;
    };
    let meta_de::Slot::List(Some(deps)) = f.deps? else {
        return None;
    };
    let kind = match f.kind?.as_str()? {
        "local" => RepoKind::Local,
        "remote" => {
            let rev = Arc::from(f.rev?.as_str()?);
            let id = match f.repo_type?.as_str()? {
                "url" => RepoId::Url(Arc::from(f.repo?.as_str()?)),
                "path" => RepoId::LocalPath(match f.repo? {
                    meta_de::Slot::Str(s) => path_from_json(&Value::String(s.into_owned()))?,
                    meta_de::Slot::Object(v) => path_from_json(&v)?,
                    _ => return None,
                }),
                _ => return None,
            };
            RepoKind::Remote { id, rev }
        }
        _ => return None,
    };
    Some(RepoMeta {
        kind,
        deps: Arc::from(deps),
    })
}

/// The typed `.snp-repo.json` decoding of [`decode_repo_meta`].
mod meta_de {
    use std::borrow::Cow;
    use std::fmt;
    use std::sync::Arc;

    use serde::de::value::MapAccessDeserializer;
    use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
    use serde_json::Value;

    /// One value, as far as the decoder looks at it. Every value is read through `deserialize_any`, like
    /// `Value`'s own visitor, so the parser accepts and refuses the same inputs.
    #[derive(Debug)]
    pub(super) enum Slot<'de> {
        /// A string (borrowed from the input unless it has escapes).
        Str(Cow<'de, str>),
        /// A number `Value::as_u64` gives.
        U64(u64),
        /// An array: its items when every one is a string.
        List(Option<Vec<Arc<str>>>),
        /// An object, as `Value` (a local-path `repo`'s `{"hex": …}`).
        Object(Value),
        /// Anything else: null, a boolean, a negative or fractional number.
        Other,
    }

    impl Slot<'_> {
        /// `Value::as_str`.
        pub(super) fn as_str(&self) -> Option<&str> {
            match self {
                Self::Str(s) => Some(s),
                _ => None,
            }
        }
    }

    struct SlotVisitor;

    impl<'de> Visitor<'de> for SlotVisitor {
        type Value = Slot<'de>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("any JSON value")
        }

        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Slot<'de>, E> {
            Ok(Slot::Other)
        }

        // `Number::from(i64)` is a positive integer when non-negative: `as_u64` gives it.
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Slot<'de>, E> {
            Ok(u64::try_from(v).map_or(Slot::Other, Slot::U64))
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Slot<'de>, E> {
            Ok(Slot::U64(v))
        }

        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Slot<'de>, E> {
            Ok(Slot::Other)
        }

        fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Slot<'de>, E> {
            Ok(Slot::Str(Cow::Borrowed(v)))
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Slot<'de>, E> {
            Ok(Slot::Str(Cow::Owned(v.to_owned())))
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Slot<'de>, E> {
            Ok(Slot::Str(Cow::Owned(v)))
        }

        fn visit_unit<E: de::Error>(self) -> Result<Slot<'de>, E> {
            Ok(Slot::Other)
        }

        fn visit_none<E: de::Error>(self) -> Result<Slot<'de>, E> {
            Ok(Slot::Other)
        }

        fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Slot<'de>, D::Error> {
            Slot::deserialize(deserializer)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Slot<'de>, A::Error> {
            let mut items = Some(Vec::new());
            while let Some(item) = seq.next_element::<Slot<'de>>()? {
                match (item, items.as_mut()) {
                    (Slot::Str(s), Some(v)) => v.push(Arc::from(&*s)),
                    _ => items = None,
                }
            }
            Ok(Slot::List(items))
        }

        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Slot<'de>, A::Error> {
            Value::deserialize(MapAccessDeserializer::new(map)).map(Slot::Object)
        }
    }

    impl<'de> Deserialize<'de> for Slot<'de> {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_any(SlotVisitor)
        }
    }

    /// A top-level key: the ones the decoder reads, or another (whose value is still read and validated).
    enum Key {
        Format,
        Kind,
        RepoType,
        Repo,
        Rev,
        Deps,
        Other,
    }

    struct KeyVisitor;

    impl Visitor<'_> for KeyVisitor {
        type Value = Key;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object key")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Key, E> {
            Ok(match v {
                "format" => Key::Format,
                "kind" => Key::Kind,
                "repo_type" => Key::RepoType,
                "repo" => Key::Repo,
                "rev" => Key::Rev,
                "deps" => Key::Deps,
                _ => Key::Other,
            })
        }
    }

    impl<'de> Deserialize<'de> for Key {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            // `Value`'s map visitor reads its keys with `deserialize_str` too.
            deserializer.deserialize_str(KeyVisitor)
        }
    }

    /// The last value of each key the decoder reads.
    #[derive(Debug, Default)]
    pub(super) struct Fields<'de> {
        pub(super) format: Option<Slot<'de>>,
        pub(super) kind: Option<Slot<'de>>,
        pub(super) repo_type: Option<Slot<'de>>,
        pub(super) repo: Option<Slot<'de>>,
        pub(super) rev: Option<Slot<'de>>,
        pub(super) deps: Option<Slot<'de>>,
    }

    struct FieldsVisitor;

    impl<'de> Visitor<'de> for FieldsVisitor {
        type Value = Fields<'de>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a JSON object")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields<'de>, A::Error> {
            let mut f = Fields::default();
            while let Some(key) = map.next_key::<Key>()? {
                let value = map.next_value::<Slot<'de>>()?;
                let slot = match key {
                    Key::Format => &mut f.format,
                    Key::Kind => &mut f.kind,
                    Key::RepoType => &mut f.repo_type,
                    Key::Repo => &mut f.repo,
                    Key::Rev => &mut f.rev,
                    Key::Deps => &mut f.deps,
                    Key::Other => continue,
                };
                *slot = Some(value);
            }
            Ok(f)
        }
    }

    impl<'de> Deserialize<'de> for Fields<'de> {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            // `deserialize_any` like `Value` (any other top-level value is refused by the visitor)
            deserializer.deserialize_any(FieldsVisitor)
        }
    }
}

/// The `serde_json::Value` decoder before [`decode_repo_meta`] (its test oracle).
#[cfg(test)]
fn decode_repo_meta_oracle(bytes: &[u8]) -> Option<RepoMeta> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let m = v.as_object()?;
    if m.get("format")?.as_u64()? != 1 {
        return None;
    }
    let deps = m
        .get("deps")?
        .as_array()?
        .iter()
        .map(|d| d.as_str().map(Arc::<str>::from))
        .collect::<Option<Vec<_>>>()?;
    let kind = match m.get("kind")?.as_str()? {
        "local" => RepoKind::Local,
        "remote" => {
            let rev = Arc::from(m.get("rev")?.as_str()?);
            let id = match m.get("repo_type")?.as_str()? {
                "url" => RepoId::Url(Arc::from(m.get("repo")?.as_str()?)),
                "path" => RepoId::LocalPath(path_from_json(m.get("repo")?)?),
                _ => return None,
            };
            RepoKind::Remote { id, rev }
        }
        _ => return None,
    };
    Some(RepoMeta {
        kind,
        deps: Arc::from(deps),
    })
}

/// `.snp-tools.json`: `{"v": 1, "tools": [{"tool": …, "version": …}, …]}`.
pub(crate) fn encode_tools(tools: &[ToolRef]) -> Vec<u8> {
    let list: Vec<Value> = tools
        .iter()
        .map(|t| json!({ "tool": &*t.tool, "version": &*t.version }))
        .collect();
    let mut out = serde_json::to_vec(&json!({ "v": 1, "tools": list })).unwrap_or_default();
    out.push(b'\n');
    out
}

/// Parses `.snp-tools.json`; `None` when malformed (the caller then keeps every tool: it cannot know what the env
/// references).
pub(crate) fn decode_tools(bytes: &[u8]) -> Option<Vec<ToolRef>> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let m = v.as_object()?;
    if m.get("v")?.as_u64()? != 1 {
        return None;
    }
    m.get("tools")?
        .as_array()?
        .iter()
        .map(|t| {
            let t = t.as_object()?;
            Some(ToolRef {
                tool: Arc::from(t.get("tool")?.as_str()?),
                version: Arc::from(t.get("version")?.as_str()?),
            })
        })
        .collect()
}

/// `store-v1.json`: `{"format": 1, "path": <abs store path>}` (relocation detection, 11 §A.10.4).
pub(crate) fn encode_store_file(path: &Path) -> Vec<u8> {
    let mut out =
        serde_json::to_vec(&json!({ "format": 1, "path": path_to_json(path) })).unwrap_or_default();
    out.push(b'\n');
    out
}

/// The recorded store path of `store-v1.json`; `None` when malformed.
pub(crate) fn decode_store_file(bytes: &[u8]) -> Option<PathBuf> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let m = v.as_object()?;
    if m.get("format")?.as_u64()? != 1 {
        return None;
    }
    path_from_json(m.get("path")?)
}

/// A `configs-v1` entry: config realpath bytes, NUL, project-root bytes.
pub(crate) fn encode_config_entry(cfg: &Path, project_root: &Path) -> Vec<u8> {
    let mut out = path_bytes(cfg);
    out.push(0);
    out.extend_from_slice(&path_bytes(project_root));
    out
}

/// Parses a `configs-v1` entry; `None` unless both halves are non-empty and NUL-free.
pub(crate) fn decode_config_entry(bytes: &[u8]) -> Option<(PathBuf, PathBuf)> {
    let nul = memchr::memchr(0, bytes)?;
    let cfg = bytes.get(..nul)?;
    let root = bytes.get(nul.checked_add(1)?..)?;
    if cfg.is_empty() || root.is_empty() || memchr::memchr(0, root).is_some() {
        return None;
    }
    Some((
        path_from_bytes(cfg.to_vec())?,
        path_from_bytes(root.to_vec())?,
    ))
}

/// Install markers stay opaque to the store, but a file that is not even one JSON value (truncated, garbled) is no
/// marker at all: the environment is reinstalled (ADR-012, X13-ENV-01).
pub(crate) fn is_json_value(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde::de::IgnoredAny>(bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deps(xs: &[&str]) -> Arc<[Arc<str>]> {
        xs.iter().map(|s| Arc::from(*s)).collect()
    }

    #[test]
    fn repo_meta_round_trip() {
        for meta in [
            RepoMeta {
                kind: RepoKind::Remote {
                    id: RepoId::Url(Arc::from("https://github.com/pre-commit/pre-commit-hooks")),
                    rev: Arc::from("v6.0.0"),
                },
                deps: deps(&["b", "a"]),
            },
            RepoMeta {
                kind: RepoKind::Remote {
                    id: RepoId::LocalPath(PathBuf::from("/src/hooks")),
                    rev: Arc::from("HEAD"),
                },
                deps: deps(&[]),
            },
            RepoMeta {
                kind: RepoKind::Local,
                deps: deps(&["six==1.16"]),
            },
        ] {
            let back = decode_repo_meta(&encode_repo_meta(&meta)).unwrap();
            assert_eq!(back.key(), meta.key());
            assert_eq!(back.deps, meta.deps);
        }
        assert!(decode_repo_meta(b"").is_none());
        assert!(decode_repo_meta(b"{}").is_none());
        assert!(decode_repo_meta(br#"{"format":2,"kind":"local","deps":[]}"#).is_none());
        assert!(decode_repo_meta(br#"{"format":1,"kind":"local","deps":[1]}"#).is_none());
        assert!(decode_repo_meta(br#"{"format":1,"kind":"local","deps":[]}"#).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_survive() {
        use std::os::unix::ffi::OsStrExt;
        let p = Path::new(std::ffi::OsStr::from_bytes(b"/src/h\xffooks"));
        assert_eq!(path_from_json(&path_to_json(p)).as_deref(), Some(p));
        let (c, r) = decode_config_entry(&encode_config_entry(p, Path::new("/r"))).unwrap();
        assert_eq!((c.as_path(), r.as_path()), (p, Path::new("/r")));
        assert_eq!(decode_store_file(&encode_store_file(p)).as_deref(), Some(p));
    }

    #[test]
    fn tools_and_entries() {
        let tools = vec![ToolRef {
            tool: Arc::from("node"),
            version: Arc::from("22.11.0"),
        }];
        assert_eq!(decode_tools(&encode_tools(&tools)).unwrap(), tools);
        assert_eq!(decode_tools(&encode_tools(&[])).unwrap(), Vec::new());
        assert!(decode_tools(b"[").is_none());
        assert!(decode_config_entry(b"").is_none());
        assert!(decode_config_entry(b"\0/r").is_none());
        assert!(decode_config_entry(b"/c\0").is_none());
        assert!(decode_config_entry(b"/c\0/r\0x").is_none());
        assert!(is_json_value(b"{\"v\":1}"));
        assert!(!is_json_value(b"{bad"));
        assert!(!is_json_value(b""));
        assert!(!is_json_value(b"\xff"));
    }

    /// Both decoders' verdicts, comparable (`RepoMeta` has no `PartialEq`).
    fn both(bytes: &[u8]) -> (String, String) {
        (
            format!("{:?}", decode_repo_meta(bytes)),
            format!("{:?}", decode_repo_meta_oracle(bytes)),
        )
    }

    /// Keys of generated objects: the read ones, an escaped spelling of one, the written-but-unread ones, others.
    const KEYS: [&str; 12] = [
        "format",
        "kind",
        "repo_type",
        "repo",
        "rev",
        "deps",
        "\\u0066ormat",
        "created",
        "snp_version",
        "object_format",
        "template_version",
        "x",
    ];

    /// Values of generated objects: right and wrong types for every read key, numbers `as_u64` does and does not
    /// give, escapes, a lone surrogate, invalid UTF-8, out-of-range numbers, nesting at the parser's limit.
    fn values() -> Vec<Vec<u8>> {
        let mut v: Vec<Vec<u8>> = [
            "1",
            "1.0",
            "-1",
            "0",
            "-0",
            "2",
            "1e0",
            "18446744073709551615",
            "18446744073709551616",
            "-9223372036854775808",
            "1e400",
            "\"local\"",
            "\"remote\"",
            "\"url\"",
            "\"path\"",
            "\"https://x\"",
            "\"\\u006cocal\"",
            "\"v1\"",
            "\"/src/h\"",
            "[]",
            "[\"a\",\"b\"]",
            "[\"a\",1]",
            "[[\"a\"]]",
            "[\"\\u0062\"]",
            "{\"hex\":\"2f61\"}",
            "{\"hex\":\"zz\"}",
            "{\"hex\":\"2f61\",\"hex\":\"2f62\"}",
            "{\"hex\":1}",
            "{}",
            "null",
            "true",
            "\"\\ud800\"",
            "\"\\x\"",
        ]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
        v.push(b"\"bad\xff\"".to_vec());
        v.push(b"\"tab\there\"".to_vec());
        for depth in [126, 127, 128, 129] {
            let mut deep = vec![b'['; depth];
            deep.extend(std::iter::repeat_n(b']', depth));
            v.push(deep);
        }
        v
    }

    /// A valid marker to start from (local, URL, path, hex path).
    const BASES: [&str; 4] = [
        r#""format":1,"kind":"local","template_version":1,"deps":["six==1.16"],"object_format":null,"created":1,"snp_version":"2.0.1""#,
        r#""format":1,"kind":"remote","repo_type":"url","repo":"https://x","rev":"v1","deps":[],"object_format":null,"created":1,"snp_version":"2.0.1""#,
        r#""format":1,"kind":"remote","repo_type":"path","repo":"/src/h","rev":"HEAD","deps":["a","b"]"#,
        r#""format":1,"kind":"remote","repo_type":"path","repo":{"hex":"2f7372632f68"},"rev":"HEAD","deps":[]"#,
    ];

    fn render(base: Option<usize>, entries: &[(usize, usize)], tail: u8) -> Vec<u8> {
        let values = values();
        let mut out = b"{".to_vec();
        let base = base.and_then(|b| BASES.get(b));
        if let Some(b) = base {
            out.extend_from_slice(b.as_bytes());
        }
        let mut first = base.is_none();
        for &(k, v) in entries {
            if !first {
                out.extend_from_slice(b", ");
            }
            first = false;
            out.push(b'"');
            out.extend_from_slice(KEYS[k % KEYS.len()].as_bytes());
            out.extend_from_slice(b"\": ");
            out.extend_from_slice(&values[v % values.len()]);
        }
        out.push(b'}');
        match tail {
            0 => out.push(b'\n'),
            1 => out.extend_from_slice(b" x"),
            2 => {
                out.pop();
            }
            3 => out.extend_from_slice(b"{}"),
            _ => {}
        }
        out
    }

    /// The typed decoder agrees with the `Value` decoder on the written markers, on hand-picked edge cases and on
    /// the generated objects below.
    #[test]
    fn typed_decoder_matches_the_value_decoder_on_edge_cases() {
        let mut cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"[]".to_vec(),
            b"\"x\"".to_vec(),
            b"null".to_vec(),
            b"1".to_vec(),
            b" {\"format\":1,\"kind\":\"local\",\"deps\":[]} \n".to_vec(),
            b"{\"format\":1,\"kind\":\"local\",\"deps\":[],}".to_vec(),
            b"{\"format\":1,\"format\":2,\"kind\":\"local\",\"deps\":[]}".to_vec(),
            b"{\"format\":2,\"format\":1,\"kind\":\"local\",\"deps\":[]}".to_vec(),
            b"{\"format\":1,\"kind\":\"local\",\"deps\":[1],\"deps\":[]}".to_vec(),
            b"{\"format\":1,\"kind\":\"local\",\"deps\":[],\"deps\":[1]}".to_vec(),
            b"{\"format\":1,\"kind\":\"local\",\"deps\":[],\"x\":\"\\ud800\"}".to_vec(),
            b"{\"format\":1,\"kind\":\"local\",\"deps\":[],\"x\":1e400}".to_vec(),
            b"{\"format\":1,\"kind\":\"local\",\"deps\":[],\"x\":\"\xff\"}".to_vec(),
            b"{\"format\":1,\"kind\":\"local\",\"deps\":[],\"x\":[1,]}".to_vec(),
            b"{\"format\":1,\"kind\":\"local\",\"deps\":[]}{}".to_vec(),
            b"{\"format\":1,\"kind\":\"remote\",\"repo_type\":\"path\",\"repo\":{\"hex\":\"2f61\",\"x\":[]},\"rev\":\"r\",\"deps\":[]}".to_vec(),
            b"{\"format\":1,\"kind\":\"remote\",\"repo_type\":\"path\",\"repo\":[\"/a\"],\"rev\":\"r\",\"deps\":[]}".to_vec(),
            b"{\"format\":1,\"kind\":\"remote\",\"repo_type\":\"url\",\"repo\":{\"hex\":\"2f61\"},\"rev\":\"r\",\"deps\":[]}".to_vec(),
            b"{\"format\":1,\"kind\":\"remote\",\"repo_type\":\"url\",\"repo\":\"u\",\"rev\":1,\"deps\":[]}".to_vec(),
        ];
        for b in 0..BASES.len() {
            cases.push(render(Some(b), &[], 0));
            for k in 0..KEYS.len() {
                for v in 0..values().len() {
                    cases.push(render(Some(b), &[(k, v)], 0));
                }
            }
        }
        for meta in [
            RepoMeta {
                kind: RepoKind::Remote {
                    id: RepoId::Url(Arc::from("https://github.com/pre-commit/pre-commit-hooks")),
                    rev: Arc::from("v6.0.0"),
                },
                deps: deps(&["b", "a"]),
            },
            RepoMeta {
                kind: RepoKind::Remote {
                    id: RepoId::LocalPath(PathBuf::from("/src/hooks")),
                    rev: Arc::from("HEAD"),
                },
                deps: deps(&[]),
            },
            RepoMeta {
                kind: RepoKind::Local,
                deps: deps(&["six==1.16"]),
            },
        ] {
            cases.push(encode_repo_meta(&meta));
        }
        let mut some = 0;
        for c in &cases {
            let (new, old) = both(c);
            assert_eq!(new, old, "{}", String::from_utf8_lossy(c));
            some += usize::from(old != "None");
        }
        assert!(
            some > 100 && some < cases.len() - 100,
            "{some} of {}",
            cases.len()
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(2048))]

        /// Generated objects (from a valid marker or from scratch; duplicate keys; any value under any key;
        /// trailing data, truncation): the typed decoder's verdict is the `Value` decoder's.
        #[test]
        fn typed_decoder_matches_the_value_decoder(
            base in proptest::option::weighted(0.7, 0..BASES.len()),
            entries in proptest::collection::vec((0..KEYS.len(), 0usize..64), 0..6),
            tail in 0u8..8,
        ) {
            let bytes = render(base, &entries, tail);
            let (new, old) = both(&bytes);
            proptest::prop_assert_eq!(new, old, "{}", String::from_utf8_lossy(&bytes));
        }
    }
}
