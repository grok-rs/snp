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

/// Decodes lower-case or upper-case hex.
fn unhex(s: &[u8]) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let digit = |b: u8| -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b.wrapping_sub(b'0')),
            b'a'..=b'f' => Some(b.wrapping_sub(b'a').wrapping_add(10)),
            b'A'..=b'F' => Some(b.wrapping_sub(b'A').wrapping_add(10)),
            _ => None,
        }
    };
    let mut out = Vec::new();
    let mut it = s.iter();
    while let (Some(&h), Some(&l)) = (it.next(), it.next()) {
        out.push((digit(h)? << 4) | digit(l)?);
    }
    Some(out)
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
pub(crate) fn decode_repo_meta(bytes: &[u8]) -> Option<RepoMeta> {
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
}
