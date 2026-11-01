//! A small owned YAML tree for the harness's own files (scenarios, `brand_map.yaml`, divergence files), built from
//! snp-yaml's composer (PyYAML `SafeLoader` resolution, ADR-002) so the harness reads YAML exactly as the project's
//! other tools do.
//!
//! The harness never needs Python object semantics (that is snp-config's job): a scalar keeps its text and the tag
//! PyYAML's resolver gave it, so `yes` is a bool for `tty: yes` while a file name `yes` stays a string.
//!
//! Owner: WP-35.

use std::fmt;

use snp_yaml::{Document, Limits, NodeId, NodeKind, Schema};

const NULL_TAG: &str = "tag:yaml.org,2002:null";
const BOOL_TAG: &str = "tag:yaml.org,2002:bool";
const INT_TAG: &str = "tag:yaml.org,2002:int";
const BINARY_TAG: &str = "tag:yaml.org,2002:binary";

/// One YAML node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Yv {
    /// `~`, `null`, an empty value.
    Null,
    /// A scalar: its text and its resolved tag.
    Scalar {
        /// The scalar's text (after YAML unescaping).
        text: String,
        /// The resolved tag (`tag:yaml.org,2002:str`, `…:int`, …).
        tag: String,
    },
    /// `!!binary` bytes.
    Bytes(Vec<u8>),
    /// A sequence.
    Seq(Vec<Self>),
    /// A mapping in document order (keys are rendered to their text).
    Map(Vec<(String, Self)>),
}

/// A YAML loading error: `file: message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlLoadError(pub String);

impl fmt::Display for YamlLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Yv {
    /// Parses one YAML document (PyYAML 1.1 safe resolution; an empty stream is [`Yv::Null`]).
    pub fn parse(text: &str) -> Result<Self, YamlLoadError> {
        let lim = Limits::default();
        let doc = snp_yaml::compose(text, Schema::PyYaml11Safe, &lim)
            .map_err(|e| YamlLoadError(e.to_string()))?;
        match doc.root {
            None => Ok(Self::Null),
            Some(root) => convert(&doc, root, 0),
        }
    }

    /// The scalar text of a string/int/bool/float scalar (`None` for null, bytes and collections).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Scalar { text, .. } => Some(text),
            _ => None,
        }
    }

    /// A scalar as an owned string; null is the empty string (YAML `key:` with no value).
    pub fn to_text(&self) -> Option<String> {
        match self {
            Self::Null => Some(String::new()),
            Self::Scalar { text, .. } => Some(text.clone()),
            _ => None,
        }
    }

    /// File contents: a string scalar's UTF-8 bytes, `!!binary` bytes, null = empty.
    pub fn to_bytes(&self) -> Option<Vec<u8>> {
        match self {
            Self::Bytes(b) => Some(b.clone()),
            other => other.to_text().map(String::into_bytes),
        }
    }

    /// A YAML 1.1 boolean (`true`/`yes`/`on`/… as PyYAML resolves them).
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Scalar { text, tag } if tag == BOOL_TAG => Some(matches!(
                text.as_str(),
                "yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on" | "On" | "ON"
            )),
            _ => None,
        }
    }

    /// A decimal integer scalar.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Scalar { text, tag } if tag == INT_TAG => text.replace('_', "").parse().ok(),
            _ => None,
        }
    }

    /// The pairs of a mapping.
    pub fn as_map(&self) -> Option<&[(String, Self)]> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }

    /// The items of a sequence.
    pub fn as_seq(&self) -> Option<&[Self]> {
        match self {
            Self::Seq(s) => Some(s),
            _ => None,
        }
    }

    /// The value of `key` in a mapping (the last occurrence wins, as in PyYAML).
    pub fn get(&self, key: &str) -> Option<&Self> {
        self.as_map()?
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    /// Whether this is null.
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// A short description of the node kind for error messages.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Scalar { .. } => "scalar",
            Self::Bytes(_) => "binary",
            Self::Seq(_) => "sequence",
            Self::Map(_) => "mapping",
        }
    }
}

/// Maximum nesting the harness accepts in its own files (far above any scenario; snp-yaml bounds the composer).
const MAX_DEPTH: u32 = 256;

fn convert(doc: &Document, id: NodeId, depth: u32) -> Result<Yv, YamlLoadError> {
    if depth > MAX_DEPTH {
        return Err(YamlLoadError(String::from("YAML nesting too deep")));
    }
    let next = depth.saturating_add(1);
    let node = usize::try_from(id.0)
        .ok()
        .and_then(|i| doc.nodes.get(i))
        .ok_or_else(|| YamlLoadError(String::from("dangling YAML node")))?;
    match &node.kind {
        NodeKind::Scalar(text) => {
            if &*node.tag == NULL_TAG {
                Ok(Yv::Null)
            } else if &*node.tag == BINARY_TAG {
                crate::text::base64_decode(text)
                    .map(Yv::Bytes)
                    .ok_or_else(|| YamlLoadError(String::from("invalid !!binary scalar")))
            } else {
                Ok(Yv::Scalar {
                    text: text.to_string(),
                    tag: node.tag.to_string(),
                })
            }
        }
        NodeKind::Seq(items) => {
            let mut out = Vec::new();
            for item in items {
                out.push(convert(doc, *item, next)?);
            }
            Ok(Yv::Seq(out))
        }
        NodeKind::Map(pairs) => {
            let mut out = Vec::new();
            for (k, v) in pairs {
                let key = match convert(doc, *k, next)? {
                    Yv::Null => String::from("null"),
                    Yv::Scalar { text, .. } => text,
                    other => {
                        return Err(YamlLoadError(format!(
                            "unsupported {} mapping key",
                            other.kind()
                        )));
                    }
                };
                out.push((key, convert(doc, *v, next)?));
            }
            Ok(Yv::Map(out))
        }
        _ => Err(YamlLoadError(String::from("unsupported YAML node"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalars_by_tag() {
        let v = Yv::parse("a: yes\nb: 'yes'\nc: 3\nd:\ne: !!binary aGk=\nf: [x, y]\n").unwrap();
        assert_eq!(v.get("a").unwrap().as_bool(), Some(true));
        assert_eq!(v.get("b").unwrap().as_bool(), None);
        assert_eq!(v.get("b").unwrap().as_str(), Some("yes"));
        assert_eq!(v.get("c").unwrap().as_i64(), Some(3));
        assert!(v.get("d").unwrap().is_null());
        assert_eq!(v.get("e").unwrap().to_bytes().unwrap(), b"hi");
        assert_eq!(v.get("f").unwrap().as_seq().unwrap().len(), 2);
        assert!(Yv::parse("").unwrap().is_null());
        assert!(Yv::parse("a: [").is_err());
    }

    #[test]
    fn last_duplicate_key_wins() {
        let v = Yv::parse("a: 1\na: 2\n").unwrap();
        assert_eq!(v.get("a").unwrap().as_i64(), Some(2));
    }
}
