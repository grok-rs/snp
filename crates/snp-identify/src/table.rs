//! Static string-keyed lookup tables generated from identify.
//!
//! Each table is a sorted `(key, TagSet)` slice plus a precomputed
//! open-addressing index (FNV-1a, Fibonacci hashing, linear probing with a
//! known maximum probe length), so a lookup hashes the key once, compares it
//! against at most `max_probe + 1` candidates and never allocates.
//!
//! The hash functions must stay identical to
//! `xtask/src/tasks/identify_tables/codegen.rs`.

use core::fmt;
use core::iter::FusedIterator;

use crate::tags::TagSet;

const EMPTY_SLOT: u16 = u16::MAX;

/// FNV-1a over `key`, with the offset basis perturbed by `seed`.
fn hash(seed: u64, key: &[u8]) -> u64 {
    hash_from(0xcbf2_9ce4_8422_2325_u64 ^ seed, key)
}

/// FNV-1a continued from the state `h` over `key`: `hash(seed, a ++ b)`
/// equals `hash_from(hash(seed, a), b)`, so a caller that needs the hash of
/// a key and of one of its prefixes hashes the bytes once.
pub(crate) fn hash_from(h: u64, key: &[u8]) -> u64 {
    key.iter().fold(h, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Fibonacci hashing of `h` into `64 - shift` bits.
fn slot(h: u64, shift: u32) -> usize {
    let s = h
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .checked_shr(shift)
        .unwrap_or(0);
    usize::try_from(s).unwrap_or(0)
}

/// A generated perfect-ish hash index from byte strings to positions in a
/// key array.
pub(crate) struct StrIndex {
    seed: u64,
    shift: u32,
    max_probe: u32,
    /// Shortest and longest key in bytes (cheap rejection before hashing).
    min_len: usize,
    max_len: usize,
    slots: &'static [u16],
}

impl StrIndex {
    pub(crate) const fn new(
        seed: u64,
        shift: u32,
        max_probe: u32,
        (min_len, max_len): (usize, usize),
        slots: &'static [u16],
    ) -> Self {
        Self {
            seed,
            shift,
            max_probe,
            min_len,
            max_len,
            slots,
        }
    }

    /// Position of `key`, given `key_at(i)` returning the `i`-th key.
    pub(crate) fn find(
        &self,
        key: &[u8],
        key_at: impl Fn(usize) -> Option<&'static str>,
    ) -> Option<usize> {
        if key.len() < self.min_len || key.len() > self.max_len {
            return None;
        }
        self.find_hashed(key, hash(self.seed, key), key_at)
    }

    /// [`find`](Self::find) for a key whose hash `h` the caller computed
    /// (`h` must be `hash(seed, key)`; any other value can only miss).
    /// The length gates still apply.
    pub(crate) fn find_hashed(
        &self,
        key: &[u8],
        h: u64,
        key_at: impl Fn(usize) -> Option<&'static str>,
    ) -> Option<usize> {
        if key.len() < self.min_len || key.len() > self.max_len {
            return None;
        }
        let mask = self.slots.len().checked_sub(1)?;
        let mut s = slot(h, self.shift) & mask;
        for _ in 0..=self.max_probe {
            let idx = *self.slots.get(s)?;
            if idx == EMPTY_SLOT {
                return None;
            }
            let idx = usize::from(idx);
            if key_at(idx)?.as_bytes() == key {
                return Some(idx);
            }
            s = s.wrapping_add(1) & mask;
        }
        None
    }
}

/// One of identify's lookup tables ([`EXTENSIONS`](crate::extensions::EXTENSIONS),
/// [`NAMES`](crate::extensions::NAMES), …): a static map from `&str` keys to
/// [`TagSet`]s.
///
/// Lookups hash the key once and never allocate. Iteration yields entries
/// sorted by key.
pub struct TagTable {
    entries: &'static [(&'static str, TagSet)],
    index: StrIndex,
}

impl TagTable {
    pub(crate) const fn new(entries: &'static [(&'static str, TagSet)], index: StrIndex) -> Self {
        Self { entries, index }
    }

    /// The tags stored for `key` (exact, case-sensitive match).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<TagSet> {
        self.get_bytes(key.as_bytes())
    }

    /// Like [`get`](TagTable::get) for a key given as bytes (e.g. a
    /// non-UTF-8 file name, which never matches).
    #[must_use]
    pub fn get_bytes(&self, key: &[u8]) -> Option<TagSet> {
        let i = self
            .index
            .find(key, |i| self.entries.get(i).map(|(k, _)| *k))?;
        self.entries.get(i).map(|(_, tags)| *tags)
    }

    /// The hash of `key` for [`get_hashed`](Self::get_hashed).
    pub(crate) fn hash(&self, key: &[u8]) -> u64 {
        hash(self.index.seed, key)
    }

    /// The longest key in bytes: a longer key never matches.
    pub(crate) const fn max_len(&self) -> usize {
        self.index.max_len
    }

    /// [`get_bytes`](Self::get_bytes) for a key whose hash `h` the caller
    /// computed with [`hash`](Self::hash), possibly in steps through
    /// [`hash_from`]; a wrong `h` can only miss.
    pub(crate) fn get_hashed(&self, key: &[u8], h: u64) -> Option<TagSet> {
        let i = self
            .index
            .find_hashed(key, h, |i| self.entries.get(i).map(|(k, _)| *k))?;
        self.entries.get(i).map(|(_, tags)| *tags)
    }

    /// Whether `key` is in the table.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Number of entries.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table has no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All `(key, tags)` entries, sorted by key.
    #[must_use]
    pub const fn entries(&self) -> &'static [(&'static str, TagSet)] {
        self.entries
    }

    /// Iterate over `(key, tags)` entries sorted by key.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&'static str, TagSet)> + ExactSizeIterator + FusedIterator
    {
        self.entries.iter().copied()
    }

    /// Iterate over the keys in sorted order.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn keys(
        &self,
    ) -> impl DoubleEndedIterator<Item = &'static str> + ExactSizeIterator + FusedIterator {
        self.entries.iter().map(|(k, _)| *k)
    }

    /// Iterate over the tag sets in key order.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn values(
        &self,
    ) -> impl DoubleEndedIterator<Item = TagSet> + ExactSizeIterator + FusedIterator {
        self.entries.iter().map(|(_, v)| *v)
    }
}

impl fmt::Debug for TagTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<'a> IntoIterator for &'a TagTable {
    type Item = &'a (&'static str, TagSet);
    type IntoIter = core::slice::Iter<'a, (&'static str, TagSet)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{hash, hash_from};
    use crate::extensions::{EXTENSIONS, EXTENSIONS_NEED_BINARY_CHECK, NAMES};
    use crate::interpreters::INTERPRETERS;

    /// FNV-1a 64 as `xtask/src/tasks/identify_tables/codegen.rs` writes it
    /// (the generated slots depend on it bit for bit).
    fn codegen_hash(seed: u64, key: &[u8]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325_u64 ^ seed;
        for &b in key {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    #[test]
    fn hash_matches_codegen_and_resumes_mid_stream() {
        // FNV-1a 64 reference values (seed 0 is the plain offset basis).
        assert_eq!(hash(0, b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash(0, b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hash(0, b"foobar"), 0x8594_4171_f739_67e8);
        let keys: [&[u8]; 7] = [
            b"",
            b".",
            b"Dockerfile.xenial",
            b"..a.py",
            b"\xff\xfe.py",
            "x.\u{212a}t".as_bytes(),
            b"a-name-longer-than-any-table-key.tar.gz",
        ];
        for seed in [0, 1, 162, 975, 2254, 2894, u64::MAX] {
            for key in keys {
                let whole = hash(seed, key);
                assert_eq!(whole, codegen_hash(seed, key));
                for cut in 0..=key.len() {
                    let (prefix, rest) = key.split_at_checked(cut).unwrap();
                    let mid = hash(seed, prefix);
                    assert_eq!(mid, codegen_hash(seed, prefix));
                    assert_eq!(hash_from(mid, rest), whole, "{seed} {key:?} {cut}");
                }
            }
        }
        for (key, _) in NAMES.iter() {
            let key = key.as_bytes();
            assert_eq!(NAMES.get_hashed(key, NAMES.hash(key)), NAMES.get_bytes(key));
            assert!(key.len() <= NAMES.max_len());
        }
        // A wrong hash can only miss; the length gates still apply.
        assert_eq!(NAMES.get_hashed(b"Makefile", 0), None);
        let long = b"Dockerfile-but-far-too-long";
        assert_eq!(NAMES.get_hashed(long, NAMES.hash(long)), None);
    }

    #[test]
    fn every_key_is_found_through_the_index() {
        for table in [
            &EXTENSIONS,
            &EXTENSIONS_NEED_BINARY_CHECK,
            &NAMES,
            &INTERPRETERS,
        ] {
            assert!(
                table.keys().zip(table.keys().skip(1)).all(|(a, b)| a < b),
                "sorted"
            );
            for (key, tags) in table.iter() {
                assert_eq!(table.get(key), Some(tags), "{key}");
            }
        }
    }

    #[test]
    fn misses() {
        for key in [
            "",
            "PY",
            "py ",
            "xx",
            "a-very-long-key-that-is-not-there",
            "\u{fffd}",
        ] {
            assert_eq!(EXTENSIONS.get(key), None, "{key}");
        }
        assert_eq!(NAMES.get_bytes(b"\xff\xfe"), None);
        assert!(!INTERPRETERS.is_empty());
        assert!(format!("{INTERPRETERS:?}").contains("\"python3\""));
        assert_eq!((&NAMES).into_iter().count(), NAMES.len());
    }
}
