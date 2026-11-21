//! The hasher of every [`Map`] and [`Set`](super::Set) and the precomputed-hash keys of
//! schemas (ADR-111).

use std::fmt;
use std::hash::{BuildHasher, DefaultHasher, RandomState};
use std::ops::Deref;
use std::sync::OnceLock;

use indexmap::map::raw_entry_v1::{RawEntryApiV1, RawEntryMut};

use super::{Map, Value};

/// The process's one hashing key: std's SipHash-1-3 with a random 128-bit
/// key drawn once per process (the `stamp.rs` precedent; ADR-111). It is
/// written once and never observable: no hash is printed, persisted or
/// used for ordering ([`Map`] iterates in insertion order).
fn seed() -> &'static RandomState {
    static SEED: OnceLock<RandomState> = OnceLock::new();
    SEED.get_or_init(RandomState::new)
}

/// The [`BuildHasher`] of [`Map`] and [`Set`](super::Set): zero-sized, every instance
/// hashes with the same per-process random key (keyed SipHash-1-3, as
/// `RandomState`; ADR-111).
///
/// Because every map shares the key, the hash of a schema's own key can be
/// computed once ([`HashedKey`]) and reused for every map it is looked up
/// in. Hash values depend on the process and are never observable.
///
/// ```
/// use cfgv::{Map, Value, ValueState};
///
/// let mut m = Map::with_capacity_and_hasher(1, ValueState);
/// m.insert(Value::from("id"), Value::Int(1));
/// assert_eq!(m, Map::from_iter([(Value::from("id"), Value::Int(1))]));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValueState;

impl BuildHasher for ValueState {
    type Hasher = DefaultHasher;

    fn build_hasher(&self) -> DefaultHasher {
        seed().build_hasher()
    }
}

/// A key with its hash precomputed, for the fixed keys of a schema
/// (`"id"`, `"files"`, ...) that are looked up in every validated map.
///
/// The hash is taken once, at run time, with the process's [`ValueState`]
/// key, and every lookup reuses it. A lookup through a `HashedKey` finds
/// exactly what the same lookup with the plain [`Value`] finds.
///
/// ```
/// use cfgv::{HashedKey, Value, value_map};
///
/// let key = HashedKey::new("id");
/// let map = value_map! {"id" => "x"};
/// assert_eq!(key.get(map.as_map().unwrap()), Some(&Value::from("x")));
/// assert_eq!(key.to_string(), "id");
/// ```
#[derive(Clone)]
pub struct HashedKey {
    key: Value,
    hash: u64,
}

impl HashedKey {
    /// Hashes `key` with the process's [`ValueState`].
    #[must_use]
    pub fn new(key: impl Into<Value>) -> Self {
        let key = key.into();
        let hash = ValueState.hash_one(&key);
        Self { key, hash }
    }

    /// The key.
    #[must_use]
    pub const fn value(&self) -> &Value {
        &self.key
    }

    /// The key, consuming `self`.
    #[must_use]
    pub fn into_value(self) -> Value {
        self.key
    }

    /// The precomputed hash, checked against the map's own hash of the key
    /// in debug builds (the tests): a mismatch would read as a missing key.
    #[expect(
        clippy::disallowed_macros,
        reason = "debug-only check that the precomputed hash is the map's own (ADR-111 H1)"
    )]
    fn hash_for(&self, map: &Map) -> u64 {
        debug_assert_eq!(self.hash, map.hasher().hash_one(&self.key));
        self.hash
    }

    /// `map.get(key)`.
    #[must_use]
    pub fn get<'m>(&self, map: &'m Map) -> Option<&'m Value> {
        map.raw_entry_v1()
            .from_key_hashed_nocheck(self.hash_for(map), &self.key)
            .map(|(_, v)| v)
    }

    /// `map.get_full(key)` without the key: the index and the value.
    #[must_use]
    pub fn get_full<'m>(&self, map: &'m Map) -> Option<(usize, &'m Value)> {
        let hash = self.hash_for(map);
        map.raw_entry_v1()
            .from_hash_full(hash, |k| *k == self.key)
            .map(|(index, _, v)| (index, v))
    }

    /// `map.contains_key(key)`.
    #[must_use]
    pub fn contained_in(&self, map: &Map) -> bool {
        self.get(map).is_some()
    }

    /// `map.get_mut(key)`.
    #[must_use]
    pub fn get_mut<'m>(&self, map: &'m mut Map) -> Option<&'m mut Value> {
        let hash = self.hash_for(map);
        match map
            .raw_entry_mut_v1()
            .from_key_hashed_nocheck(hash, &self.key)
        {
            RawEntryMut::Occupied(slot) => Some(slot.into_mut()),
            RawEntryMut::Vacant(_) => None,
        }
    }

    /// `map.get_full_mut(key)` without the key: the index and the value.
    #[must_use]
    pub fn get_full_mut<'m>(&self, map: &'m mut Map) -> Option<(usize, &'m mut Value)> {
        let hash = self.hash_for(map);
        match map
            .raw_entry_mut_v1()
            .from_key_hashed_nocheck(hash, &self.key)
        {
            RawEntryMut::Occupied(slot) => {
                let index = slot.index();
                Some((index, slot.into_mut()))
            }
            RawEntryMut::Vacant(_) => None,
        }
    }

    /// `map.setdefault(key, default)` in one probe: the key and the default
    /// are cloned only when the key is missing (then inserted at the end,
    /// like Python). Returns the value now under the key.
    pub fn set_default<'m>(&self, map: &'m mut Map, default: &Value) -> &'m mut Value {
        let hash = self.hash_for(map);
        match map
            .raw_entry_mut_v1()
            .from_key_hashed_nocheck(hash, &self.key)
        {
            RawEntryMut::Occupied(slot) => slot.into_mut(),
            // `hash` is the map's own hash of the key, and a clone hashes alike.
            RawEntryMut::Vacant(slot) => {
                slot.insert_hashed_nocheck(hash, self.key.clone(), default.clone())
                    .1
            }
        }
    }
}

impl Deref for HashedKey {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.key
    }
}

impl AsRef<Value> for HashedKey {
    fn as_ref(&self) -> &Value {
        &self.key
    }
}

impl PartialEq for HashedKey {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for HashedKey {}

impl fmt::Debug for HashedKey {
    /// The key's `Debug` (the hash is never shown).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.key, f)
    }
}

impl fmt::Display for HashedKey {
    /// The key's `Display` (Python `str()`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.key, f)
    }
}

impl From<Value> for HashedKey {
    fn from(key: Value) -> Self {
        Self::new(key)
    }
}

impl From<&str> for HashedKey {
    fn from(key: &str) -> Self {
        Self::new(key)
    }
}

impl From<String> for HashedKey {
    fn from(key: String) -> Self {
        Self::new(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builder_is_zero_sized_and_shared() {
        assert_eq!(size_of::<ValueState>(), 0);
        let v = Value::from("files");
        assert_eq!(ValueState.hash_one(&v), ValueState.hash_one(&v));
        assert_eq!(
            Map::default().hasher().hash_one(&v),
            HashedKey::new("files").hash
        );
    }

    #[test]
    fn set_default_inserts_at_the_end_and_keeps_existing_values() {
        let mut m = Map::default();
        m.insert(Value::from("a"), Value::Int(1));
        let k = HashedKey::new("b");
        *k.set_default(&mut m, &Value::Int(2)) = Value::Int(3);
        assert_eq!(k.set_default(&mut m, &Value::Int(9)), &Value::Int(3));
        let keys: Vec<_> = m.keys().cloned().collect();
        assert_eq!(keys, [Value::from("a"), Value::from("b")]);
    }

    #[test]
    fn full_lookups_report_the_index() {
        let mut m = Map::default();
        m.insert(Value::from("a"), Value::Int(1));
        m.insert(Value::Int(1), Value::Int(2));
        // `True == 1`: the hashed lookup follows Python equality like `get_full`.
        let k = HashedKey::new(Value::Bool(true));
        assert_eq!(k.get_full(&m), Some((1, &Value::Int(2))));
        assert_eq!(k.get_full_mut(&mut m).map(|(i, _)| i), Some(1));
        assert!(HashedKey::new("a").contained_in(&m));
        assert!(!HashedKey::new("z").contained_in(&m));
    }
}
