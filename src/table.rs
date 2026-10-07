//! Static string-keyed lookup tables generated from identify.
//!
//! Each table is a sorted `(key, TagSet)` slice plus a precomputed
//! open-addressing index (FNV-1a, Fibonacci hashing, linear probing with a
//! known maximum probe length), so a lookup hashes the key once, compares it
//! against at most `max_probe + 1` candidates and never allocates.
//!
//! The hash functions must stay identical to `xtask/src/codegen.rs`.

use core::fmt;
use core::iter::FusedIterator;

use crate::tags::TagSet;

const EMPTY_SLOT: u16 = u16::MAX;

/// FNV-1a over `key`, with the offset basis perturbed by `seed`.
fn hash(seed: u64, key: &[u8]) -> u64 {
    key.iter().fold(0xcbf2_9ce4_8422_2325_u64 ^ seed, |h, &b| {
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
        let mask = self.slots.len().checked_sub(1)?;
        let mut s = slot(hash(self.seed, key), self.shift) & mask;
        for _ in 0..=self.max_probe {
            let idx = *self.slots.get(s)?;
            if idx == EMPTY_SLOT {
                return None;
            }
            let idx = usize::from(idx);
            if key_at(idx)?.as_bytes() == key {
                return Some(idx);
            }
            s = (s + 1) & mask;
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
    use crate::extensions::{EXTENSIONS, EXTENSIONS_NEED_BINARY_CHECK, NAMES};
    use crate::interpreters::INTERPRETERS;

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
