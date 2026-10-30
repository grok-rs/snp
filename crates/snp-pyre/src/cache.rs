//! Compile cache keyed by (source, [`Options`]) (01 Addendum §A.6.12; CPython's `re._cache`, unbounded).
//!
//! Owner: WP-13.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use indexmap::{Equivalent, IndexMap};

use crate::{Options, PyReError, PyRegex};

/// A cached compile result (errors are cached too: an invalid pattern is reported once per run, §A.4.5).
type Entry = Result<Arc<PyRegex>, PyReError>;

/// The owned cache key.
#[derive(Debug, PartialEq, Eq)]
struct Key {
    src: Box<str>,
    opts: Options,
}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        KeyRef {
            src: &self.src,
            opts: self.opts,
        }
        .hash(state);
    }
}

/// A borrowed lookup key (no allocation on a hit).
#[derive(Debug, Hash)]
struct KeyRef<'a> {
    src: &'a str,
    opts: Options,
}

impl Equivalent<Key> for KeyRef<'_> {
    fn equivalent(&self, key: &Key) -> bool {
        *self.src == *key.src && self.opts == key.opts
    }
}

/// Process-wide compile cache: one compile per distinct (source, [`Options`]) for the run, shared by the
/// filters, the meta hooks and pygrep (`Send + Sync`).
#[derive(Debug, Default)]
pub struct RegexCache {
    map: Mutex<IndexMap<Key, Entry>>,
}

impl RegexCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached compile of `src` under `opts`, compiling on a miss (outside the lock; when two threads miss
    /// together the first stored result wins, so every caller sees the same `Arc`).
    pub fn get(&self, src: &str, opts: Options) -> Result<Arc<PyRegex>, PyReError> {
        let probe = KeyRef { src, opts };
        if let Some(hit) = snp_core::sync::lock(&self.map).get(&probe) {
            return hit.clone();
        }
        let compiled = PyRegex::compile(src, opts).map(Arc::new);
        let mut map = snp_core::sync::lock(&self.map);
        map.entry(Key {
            src: src.into(),
            opts,
        })
        .or_insert(compiled)
        .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mode;

    #[test]
    fn caches_results_and_errors() {
        let cache = RegexCache::new();
        let a = cache.get("a.*b", Options::CONFIG).unwrap();
        let b = cache.get("a.*b", Options::CONFIG).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let bytes = Options {
            mode: Mode::Bytes,
            ..Options::CONFIG
        };
        let c = cache.get("a.*b", bytes).unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
        assert!(cache.get("(", Options::CONFIG).is_err());
        assert_eq!(
            cache.get("(", Options::CONFIG).unwrap_err(),
            cache.get("(", Options::CONFIG).unwrap_err()
        );
    }
}
