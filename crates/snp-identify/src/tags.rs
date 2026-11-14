//! Tags, tag sets and identify's tag-group constants.
//!
//! A [`Tag`] is one of the [`TAG_COUNT`] names in
//! [`ALL_TAGS`]; a [`TagSet`] is a `Copy` bitset over them. Neither allocates,
//! and every set operation is a handful of word operations — cheap enough to
//! evaluate pre-commit's `types` / `types_or` / `exclude_types` filters for
//! every file of a large repository.
//!
//! ```
//! use file_identify::{Tag, TagSet};
//!
//! let tags: TagSet = "file, non-executable, python, text".parse()?;
//! assert!(tags.contains(Tag::TEXT));
//! assert!(tags.contains("python"));
//!
//! let types = TagSet::from_names(["python"])?;
//! let exclude = TagSet::from_names(["binary"])?;
//! assert!(tags.matches_types(&types, &TagSet::EMPTY, &exclude));
//!
//! // Iteration (and `Display`) follow tag-name order.
//! assert_eq!(tags.to_string(), "file, non-executable, python, text");
//! # Ok::<(), file_identify::UnknownTagError>(())
//! ```

use core::fmt;
use core::iter::FusedIterator;
use core::ops::{
    BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Not, Sub, SubAssign,
};
use core::str::FromStr;

use crate::generated::{TAG_COUNT, TAG_INDEX, WORDS};

pub use crate::generated::ALL_TAGS;
pub use crate::meta::FileKind;

/// Name of the `directory` tag.
pub const DIRECTORY: &str = "directory";
/// Name of the `symlink` tag.
pub const SYMLINK: &str = "symlink";
/// Name of the `socket` tag.
pub const SOCKET: &str = "socket";
/// Name of the `file` tag.
pub const FILE: &str = "file";
/// Name of the `executable` tag.
pub const EXECUTABLE: &str = "executable";
/// Name of the `non-executable` tag.
pub const NON_EXECUTABLE: &str = "non-executable";
/// Name of the `text` tag.
pub const TEXT: &str = "text";
/// Name of the `binary` tag.
pub const BINARY: &str = "binary";

/// `identify.TYPE_TAGS`: `directory`, `file`, `symlink`, `socket`.
pub const TYPE_TAGS: TagSet = TagSet::EMPTY
    .with(Tag::DIRECTORY)
    .with(Tag::FILE)
    .with(Tag::SYMLINK)
    .with(Tag::SOCKET);

/// `identify.MODE_TAGS`: `executable`, `non-executable`.
pub const MODE_TAGS: TagSet = TagSet::EMPTY
    .with(Tag::EXECUTABLE)
    .with(Tag::NON_EXECUTABLE);

/// `identify.ENCODING_TAGS`: `binary`, `text`.
pub const ENCODING_TAGS: TagSet = TagSet::EMPTY.with(Tag::BINARY).with(Tag::TEXT);

/// Whether `tag` is one of [`TYPE_TAGS`].
#[must_use]
pub fn is_type_tag(tag: &str) -> bool {
    matches!(tag, DIRECTORY | FILE | SYMLINK | SOCKET)
}

/// Whether `tag` is one of [`MODE_TAGS`].
#[must_use]
pub fn is_mode_tag(tag: &str) -> bool {
    matches!(tag, EXECUTABLE | NON_EXECUTABLE)
}

/// Whether `tag` is one of [`ENCODING_TAGS`].
#[must_use]
pub fn is_encoding_tag(tag: &str) -> bool {
    matches!(tag, BINARY | TEXT)
}

/// One identify tag, such as `python`, `text` or `executable`.
///
/// A `Tag` is a 2-byte handle: its [`id`](Tag::id) is the tag's position in
/// the sorted [`ALL_TAGS`] array, so comparing tags compares their names.
/// Only the eight file-system tags have named constants ([`Tag::FILE`],
/// [`Tag::TEXT`], …); look any other tag up by name:
///
/// ```
/// use file_identify::Tag;
///
/// let python: Tag = "python".parse()?;
/// assert_eq!(python.as_str(), "python");
/// assert_eq!(Tag::from_name("python"), Some(python));
/// assert_eq!(Tag::from_name("pythn"), None);
/// assert!(Tag::FILE < Tag::TEXT); // name order
/// # Ok::<(), file_identify::UnknownTagError>(())
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag(u16);

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the named tag constants are generated into generated.rs"
)]
impl Tag {
    /// Construct from a generated id. Only used by `generated.rs`, whose ids
    /// are `< TAG_COUNT` by construction.
    pub(crate) const fn from_id_const(id: u16) -> Self {
        Self(id)
    }

    /// The tag with the given [`id`](Tag::id), if `id < TAG_COUNT`.
    #[must_use]
    pub fn from_id(id: usize) -> Option<Self> {
        if id < TAG_COUNT {
            u16::try_from(id).ok().map(Self)
        } else {
            None
        }
    }

    /// Look a tag up by its exact (case-sensitive) name.
    ///
    /// This is a hash lookup with no allocation.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        TAG_INDEX
            .find(name.as_bytes(), |i| ALL_TAGS.get(i).copied())
            .and_then(Self::from_id)
    }

    /// The tag's position in [`ALL_TAGS`] (`0..TAG_COUNT`).
    #[must_use]
    #[expect(
        clippy::as_conversions,
        reason = "u16 -> usize is lossless (From is not const)"
    )]
    pub const fn id(self) -> usize {
        self.0 as usize
    }

    /// The tag's name, e.g. `"python"`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        ALL_TAGS.get(self.id()).copied().unwrap_or_default()
    }

    /// Whether this is one of [`TYPE_TAGS`].
    #[must_use]
    pub const fn is_type_tag(self) -> bool {
        TYPE_TAGS.contains_tag(self)
    }

    /// Whether this is one of [`MODE_TAGS`].
    #[must_use]
    pub const fn is_mode_tag(self) -> bool {
        MODE_TAGS.contains_tag(self)
    }

    /// Whether this is one of [`ENCODING_TAGS`].
    #[must_use]
    pub const fn is_encoding_tag(self) -> bool {
        ENCODING_TAGS.contains_tag(self)
    }

    /// Iterate over every known tag in name order.
    #[must_use]
    pub const fn all() -> Iter {
        TagSet::ALL.iter()
    }

    /// Word index and bit mask of this tag inside a [`TagSet`].
    #[expect(
        clippy::as_conversions,
        reason = "u16 -> usize is lossless (From is not const)"
    )]
    const fn bit(self) -> (usize, u64) {
        ((self.0 / 64) as usize, 1u64 << (self.0 % 64))
    }
}

impl fmt::Debug for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Tag").field(&self.as_str()).finish()
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for Tag {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for Tag {
    type Err = UnknownTagError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_name(s).ok_or_else(|| UnknownTagError::new(vec![s.to_owned()]))
    }
}

impl TryFrom<&str> for Tag {
    type Error = UnknownTagError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl PartialEq<str> for Tag {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Tag {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<Tag> for str {
    fn eq(&self, other: &Tag) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<Tag> for &str {
    fn eq(&self, other: &Tag) -> bool {
        *self == other.as_str()
    }
}

/// Error returned when parsing names that are not in [`ALL_TAGS`].
///
/// The `Display` text is `unknown tag "x"` (or `unknown tags "x", "y"`);
/// callers that need identify/pre-commit's exact wording should build it from
/// [`names`](UnknownTagError::names).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownTagError {
    names: Vec<String>,
}

impl UnknownTagError {
    fn new(names: Vec<String>) -> Self {
        Self { names }
    }

    /// The unrecognized names, in input order (never empty).
    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

impl fmt::Display for UnknownTagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.names.len() == 1 {
            "unknown tag "
        } else {
            "unknown tags "
        })?;
        for (i, name) in self.names.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{name:?}")?;
        }
        Ok(())
    }
}

impl std::error::Error for UnknownTagError {}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Tag {}
    impl Sealed for &super::Tag {}
    impl Sealed for &str {}
    impl Sealed for String {}
    impl Sealed for &String {}
}

/// A [`Tag`] or a tag name; accepted by [`TagSet::contains`].
///
/// This trait is sealed. Looking up a name costs one hash lookup; unknown
/// names are never contained in any set.
pub trait IntoTag: sealed::Sealed {
    /// The tag this value denotes, if any.
    fn into_tag(self) -> Option<Tag>;
}

impl IntoTag for Tag {
    fn into_tag(self) -> Option<Tag> {
        Some(self)
    }
}

impl IntoTag for &Tag {
    fn into_tag(self) -> Option<Tag> {
        Some(*self)
    }
}

impl IntoTag for &str {
    fn into_tag(self) -> Option<Tag> {
        Tag::from_name(self)
    }
}

impl IntoTag for String {
    fn into_tag(self) -> Option<Tag> {
        Tag::from_name(&self)
    }
}

impl IntoTag for &String {
    fn into_tag(self) -> Option<Tag> {
        Tag::from_name(self)
    }
}

/// A set of [`Tag`]s: a `Copy` bitset with one bit per known tag.
///
/// All operations are allocation-free; the binary operators `|`, `&`, `-`,
/// `^` and `!` (complement relative to all known tags) are available as well
/// as the named methods. Iteration, [`Debug`](fmt::Debug) and
/// [`Display`](fmt::Display) list tags in name order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TagSet {
    words: [u64; WORDS],
}

// All indexing below is `words[i]` with `i < WORDS`, guarded by the loop
// condition or an explicit check, so it cannot panic. Const fns cannot use
// iterators, hence the manual loops.
#[expect(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    reason = "const fn loops: every index is `i < WORDS` (loop condition or explicit check), every counter \
              and shift is bounded by WORDS * 64 and TAG_COUNT, and u16/u32 -> usize casts are lossless"
)]
impl TagSet {
    /// The empty set.
    pub const EMPTY: Self = Self { words: [0; WORDS] };

    /// Every known tag (`identify.ALL_TAGS`).
    pub const ALL: Self = {
        let mut words = [0u64; WORDS];
        let mut i = 0;
        while i < WORDS {
            let start = i * 64;
            words[i] = if TAG_COUNT >= start + 64 {
                u64::MAX
            } else if TAG_COUNT > start {
                (1u64 << (TAG_COUNT - start)) - 1
            } else {
                0
            };
            i += 1;
        }
        Self { words }
    };

    /// The empty set (same as [`TagSet::EMPTY`]).
    #[must_use]
    pub const fn new() -> Self {
        Self::EMPTY
    }

    /// Build a set from tag ids; ids `>= TAG_COUNT` are ignored. Used by the
    /// generated tables.
    pub(crate) const fn from_ids(ids: &[u16]) -> Self {
        let mut set = Self::EMPTY;
        let mut i = 0;
        while i < ids.len() {
            if (ids[i] as usize) < TAG_COUNT {
                set = set.with(Tag(ids[i]));
            }
            i += 1;
        }
        set
    }

    /// The set containing only `tag`.
    #[must_use]
    pub const fn from_tag(tag: Tag) -> Self {
        Self::EMPTY.with(tag)
    }

    /// Build a set from tag names, failing with every unknown name.
    ///
    /// # Errors
    ///
    /// Returns [`UnknownTagError`] listing all names not in [`ALL_TAGS`].
    pub fn from_names<I, S>(names: I) -> Result<Self, UnknownTagError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set = Self::EMPTY;
        let mut unknown = Vec::new();
        for name in names {
            let name = name.as_ref();
            match Tag::from_name(name) {
                Some(tag) => {
                    set.insert(tag);
                }
                None => unknown.push(name.to_owned()),
            }
        }
        if unknown.is_empty() {
            Ok(set)
        } else {
            Err(UnknownTagError::new(unknown))
        }
    }

    /// This set plus `tag`.
    #[must_use]
    pub const fn with(self, tag: Tag) -> Self {
        let (w, bit) = tag.bit();
        let mut words = self.words;
        if w < WORDS {
            words[w] |= bit;
        }
        Self { words }
    }

    /// This set minus `tag`.
    #[must_use]
    pub const fn without(self, tag: Tag) -> Self {
        let (w, bit) = tag.bit();
        let mut words = self.words;
        if w < WORDS {
            words[w] &= !bit;
        }
        Self { words }
    }

    /// Whether `tag` is in the set (`const` version of
    /// [`contains`](TagSet::contains) for [`Tag`] values).
    #[must_use]
    pub const fn contains_tag(&self, tag: Tag) -> bool {
        let (w, bit) = tag.bit();
        w < WORDS && self.words[w] & bit != 0
    }

    /// Whether the set contains `tag`, given as a [`Tag`] or a tag name.
    ///
    /// ```
    /// use file_identify::{Tag, tags_from_filename};
    ///
    /// let tags = tags_from_filename("setup.py");
    /// assert!(tags.contains("python"));
    /// assert!(tags.contains(Tag::TEXT));
    /// assert!(!tags.contains("no-such-tag"));
    /// ```
    #[must_use]
    pub fn contains(&self, tag: impl IntoTag) -> bool {
        tag.into_tag().is_some_and(|t| self.contains_tag(t))
    }

    /// Add `tag`; returns whether it was newly inserted.
    pub fn insert(&mut self, tag: Tag) -> bool {
        let was = self.contains_tag(tag);
        *self = self.with(tag);
        !was
    }

    /// Remove `tag`; returns whether it was present.
    pub fn remove(&mut self, tag: Tag) -> bool {
        let was = self.contains_tag(tag);
        *self = self.without(tag);
        was
    }

    /// Remove every tag.
    pub fn clear(&mut self) {
        *self = Self::EMPTY;
    }

    /// Number of tags in the set.
    #[must_use]
    pub const fn len(&self) -> usize {
        let mut n = 0;
        let mut i = 0;
        while i < WORDS {
            n += self.words[i].count_ones() as usize;
            i += 1;
        }
        n
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        let mut i = 0;
        while i < WORDS {
            if self.words[i] != 0 {
                return false;
            }
            i += 1;
        }
        true
    }

    /// `self ∪ other`.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        let mut words = self.words;
        let mut i = 0;
        while i < WORDS {
            words[i] |= other.words[i];
            i += 1;
        }
        Self { words }
    }

    /// `self ∩ other`.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        let mut words = self.words;
        let mut i = 0;
        while i < WORDS {
            words[i] &= other.words[i];
            i += 1;
        }
        Self { words }
    }

    /// `self \ other`.
    #[must_use]
    pub const fn difference(self, other: Self) -> Self {
        let mut words = self.words;
        let mut i = 0;
        while i < WORDS {
            words[i] &= !other.words[i];
            i += 1;
        }
        Self { words }
    }

    /// Tags in exactly one of `self` and `other`.
    #[must_use]
    pub const fn symmetric_difference(self, other: Self) -> Self {
        let mut words = self.words;
        let mut i = 0;
        while i < WORDS {
            words[i] ^= other.words[i];
            i += 1;
        }
        Self { words }
    }

    /// Every known tag that is not in `self`.
    #[must_use]
    pub const fn complement(self) -> Self {
        Self::ALL.difference(self)
    }

    /// Whether every tag of `self` is in `other`.
    #[must_use]
    pub const fn is_subset(&self, other: &Self) -> bool {
        let mut i = 0;
        while i < WORDS {
            if self.words[i] & !other.words[i] != 0 {
                return false;
            }
            i += 1;
        }
        true
    }

    /// Whether every tag of `other` is in `self` (Python's `self >= other`).
    #[must_use]
    pub const fn is_superset(&self, other: &Self) -> bool {
        other.is_subset(self)
    }

    /// Whether `self` and `other` share no tag.
    #[must_use]
    pub const fn is_disjoint(&self, other: &Self) -> bool {
        !self.intersects(other)
    }

    /// Whether `self` and `other` share at least one tag.
    #[must_use]
    pub const fn intersects(&self, other: &Self) -> bool {
        let mut i = 0;
        while i < WORDS {
            if self.words[i] & other.words[i] != 0 {
                return true;
            }
            i += 1;
        }
        false
    }

    /// pre-commit's file-type filter (`Classifier.by_types`) applied to the
    /// tags of one file:
    ///
    /// `self ⊇ types && (types_or = ∅ || self ∩ types_or ≠ ∅) && self ∩ exclude_types = ∅`
    ///
    /// ```
    /// use file_identify::TagSet;
    ///
    /// let file: TagSet = "file, non-executable, python, text".parse()?;
    /// let none = TagSet::EMPTY;
    /// let types: TagSet = "file".parse()?;
    /// let types_or: TagSet = "python, pyi".parse()?;
    /// let exclude: TagSet = "binary".parse()?;
    /// assert!(file.matches_types(&types, &types_or, &exclude));
    /// assert!(!file.matches_types(&types, &none, &"python".parse()?));
    /// # Ok::<(), file_identify::UnknownTagError>(())
    /// ```
    #[must_use]
    pub const fn matches_types(&self, types: &Self, types_or: &Self, exclude_types: &Self) -> bool {
        self.is_superset(types)
            && (types_or.is_empty() || self.intersects(types_or))
            && self.is_disjoint(exclude_types)
    }

    /// Whether the set contains `text` or `binary` (identify's invariant for
    /// a classified regular file).
    #[must_use]
    pub const fn has_encoding(&self) -> bool {
        self.intersects(&ENCODING_TAGS)
    }

    /// Iterate over the tags in name order.
    #[must_use]
    pub const fn iter(&self) -> Iter {
        Iter {
            words: self.words,
            front: 0,
            back: WORDS,
        }
    }

    /// Iterate over the tag names in sorted order.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn names(&self) -> impl DoubleEndedIterator<Item = &'static str> + ExactSizeIterator {
        self.iter().map(Tag::as_str)
    }

    /// The first tag in name order.
    #[must_use]
    pub fn first(&self) -> Option<Tag> {
        self.iter().next()
    }

    /// The last tag in name order.
    #[must_use]
    pub fn last(&self) -> Option<Tag> {
        self.iter().next_back()
    }
}

impl fmt::Debug for TagSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.names()).finish()
    }
}

/// Tag names in name order separated by `", "`, e.g.
/// `file, non-executable, python, text` (the empty set prints nothing).
/// [`FromStr`] accepts the same format.
impl fmt::Display for TagSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, name) in self.names().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(name)?;
        }
        Ok(())
    }
}

/// Parse comma-separated tag names (surrounding whitespace is ignored, empty
/// items are skipped), the format produced by [`Display`](fmt::Display).
impl FromStr for TagSet {
    type Err = UnknownTagError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_names(s.split(',').map(str::trim).filter(|n| !n.is_empty()))
    }
}

impl TryFrom<&[&str]> for TagSet {
    type Error = UnknownTagError;

    fn try_from(names: &[&str]) -> Result<Self, Self::Error> {
        Self::from_names(names)
    }
}

impl From<Tag> for TagSet {
    fn from(tag: Tag) -> Self {
        Self::from_tag(tag)
    }
}

impl<const N: usize> From<[Tag; N]> for TagSet {
    fn from(tags: [Tag; N]) -> Self {
        tags.into_iter().collect()
    }
}

impl FromIterator<Tag> for TagSet {
    fn from_iter<I: IntoIterator<Item = Tag>>(iter: I) -> Self {
        let mut set = Self::EMPTY;
        set.extend(iter);
        set
    }
}

impl Extend<Tag> for TagSet {
    fn extend<I: IntoIterator<Item = Tag>>(&mut self, iter: I) {
        for tag in iter {
            self.insert(tag);
        }
    }
}

impl IntoIterator for TagSet {
    type Item = Tag;
    type IntoIter = Iter;

    fn into_iter(self) -> Iter {
        self.iter()
    }
}

impl IntoIterator for &TagSet {
    type Item = Tag;
    type IntoIter = Iter;

    fn into_iter(self) -> Iter {
        self.iter()
    }
}

macro_rules! set_op {
    ($trait:ident, $method:ident, $assign_trait:ident, $assign_method:ident, $op:ident) => {
        impl $trait for TagSet {
            type Output = TagSet;

            fn $method(self, rhs: TagSet) -> TagSet {
                self.$op(rhs)
            }
        }

        impl $trait<Tag> for TagSet {
            type Output = TagSet;

            fn $method(self, rhs: Tag) -> TagSet {
                self.$op(TagSet::from_tag(rhs))
            }
        }

        impl $assign_trait for TagSet {
            fn $assign_method(&mut self, rhs: TagSet) {
                *self = self.$op(rhs);
            }
        }

        impl $assign_trait<Tag> for TagSet {
            fn $assign_method(&mut self, rhs: Tag) {
                *self = self.$op(TagSet::from_tag(rhs));
            }
        }
    };
}

set_op!(BitOr, bitor, BitOrAssign, bitor_assign, union);
set_op!(BitAnd, bitand, BitAndAssign, bitand_assign, intersection);
set_op!(
    BitXor,
    bitxor,
    BitXorAssign,
    bitxor_assign,
    symmetric_difference
);
set_op!(Sub, sub, SubAssign, sub_assign, difference);

impl Not for TagSet {
    type Output = Self;

    fn not(self) -> Self {
        self.complement()
    }
}

/// Iterator over the tags of a [`TagSet`] in name order (see
/// [`TagSet::iter`]). Owns a copy of the set.
#[derive(Clone, Debug)]
pub struct Iter {
    words: [u64; WORDS],
    /// Index of the lowest word that may still have bits.
    front: usize,
    /// One past the highest word that may still have bits.
    back: usize,
}

impl Iter {
    fn tag(word: usize, bit: u32) -> Option<Tag> {
        let id = word
            .checked_mul(64)?
            .checked_add(usize::try_from(bit).ok()?)?;
        Tag::from_id(id)
    }
}

impl Iterator for Iter {
    type Item = Tag;

    fn next(&mut self) -> Option<Tag> {
        while self.front < self.back {
            let w = self.words.get_mut(self.front)?;
            if *w != 0 {
                let bit = w.trailing_zeros();
                // `*w != 0`: clears the lowest set bit.
                *w &= w.wrapping_sub(1);
                return Self::tag(self.front, bit);
            }
            // `front < back <= WORDS`.
            self.front = self.front.saturating_add(1);
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self
            .words
            .get(self.front..self.back)
            .unwrap_or_default()
            .iter()
            .map(|w| usize::try_from(w.count_ones()).unwrap_or(0))
            .sum();
        (n, Some(n))
    }
}

impl DoubleEndedIterator for Iter {
    fn next_back(&mut self) -> Option<Tag> {
        while self.back > self.front {
            // `back > front >= 0`.
            let idx = self.back.saturating_sub(1);
            let w = self.words.get_mut(idx)?;
            if *w != 0 {
                // `*w != 0`: `leading_zeros() <= 63`.
                let bit = 63_u32.saturating_sub(w.leading_zeros());
                *w &= !(1u64 << bit);
                return Self::tag(idx, bit);
            }
            self.back = idx;
        }
        None
    }
}

impl ExactSizeIterator for Iter {}

impl FusedIterator for Iter {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(clippy::disallowed_methods, reason = "a constant non-zero window size")]
    fn all_tags_sorted_unique_and_indexed() {
        assert_eq!(ALL_TAGS.len(), TAG_COUNT);
        assert!(ALL_TAGS.windows(2).all(|w| w[0] < w[1]));
        for (i, name) in ALL_TAGS.iter().enumerate() {
            let tag = Tag::from_name(name).expect("indexed");
            assert_eq!(tag.id(), i);
            assert_eq!(tag.as_str(), *name);
        }
        assert_eq!(TagSet::ALL.len(), TAG_COUNT);
        assert_eq!(Tag::all().count(), TAG_COUNT);
    }

    #[test]
    fn base_tag_constants() {
        for (tag, name) in [
            (Tag::DIRECTORY, DIRECTORY),
            (Tag::SYMLINK, SYMLINK),
            (Tag::SOCKET, SOCKET),
            (Tag::FILE, FILE),
            (Tag::EXECUTABLE, EXECUTABLE),
            (Tag::NON_EXECUTABLE, NON_EXECUTABLE),
            (Tag::TEXT, TEXT),
            (Tag::BINARY, BINARY),
        ] {
            assert_eq!(tag.as_str(), name);
        }
        assert_eq!(TYPE_TAGS.len(), 4);
        assert_eq!(MODE_TAGS.len(), 2);
        assert_eq!(ENCODING_TAGS.len(), 2);
        assert!(TYPE_TAGS.is_disjoint(&MODE_TAGS));
        assert!(MODE_TAGS.is_disjoint(&ENCODING_TAGS));
        assert!(Tag::FILE.is_type_tag());
        assert!(Tag::TEXT.is_encoding_tag());
        assert!(Tag::EXECUTABLE.is_mode_tag());
        assert!(is_type_tag("socket") && is_mode_tag("executable") && is_encoding_tag("binary"));
        assert!(!is_type_tag("python"));
    }

    #[test]
    fn unknown_names() {
        assert_eq!(Tag::from_name(""), None);
        assert_eq!(Tag::from_name("Python"), None);
        assert_eq!(Tag::from_id(TAG_COUNT), None);
        let err = "python, nope, nada".parse::<TagSet>().expect_err("unknown");
        assert_eq!(err.names(), ["nope", "nada"]);
        assert_eq!(err.to_string(), r#"unknown tags "nope", "nada""#);
        let err = "zzz".parse::<Tag>().expect_err("unknown");
        assert_eq!(err.to_string(), r#"unknown tag "zzz""#);
    }

    #[test]
    fn set_operations() {
        let a: TagSet = "python, text, file".parse().expect("valid");
        let b: TagSet = "text, binary".parse().expect("valid");
        assert_eq!((a | b).to_string(), "binary, file, python, text");
        assert_eq!((a & b).to_string(), "text");
        assert_eq!((a - b).to_string(), "file, python");
        assert_eq!((a ^ b).to_string(), "binary, file, python");
        assert_eq!((!a).len(), TAG_COUNT - 3);
        assert!((a | b).is_superset(&a));
        assert!(a.is_subset(&(a | b)));
        assert!(a.intersects(&b));
        assert!(!a.is_disjoint(&b));
        let mut c = a;
        assert!(!c.insert(Tag::TEXT));
        assert!(c.remove(Tag::TEXT));
        assert!(!c.contains(Tag::TEXT));
        c |= Tag::BINARY;
        assert!(c.contains("binary"));
        c.clear();
        assert!(c.is_empty());
        assert_eq!(TagSet::from([Tag::TEXT, Tag::FILE]).len(), 2);
        assert_eq!(format!("{:?}", TagSet::from(Tag::TEXT)), r#"{"text"}"#);
        assert_eq!(format!("{:?}", Tag::TEXT), r#"Tag("text")"#);
        assert_eq!(TagSet::EMPTY.to_string(), "");
        assert_eq!("".parse::<TagSet>(), Ok(TagSet::EMPTY));
    }

    #[test]
    fn iteration_order_and_double_ended() {
        let set = TagSet::ALL;
        let forward: Vec<_> = set.names().collect();
        assert_eq!(forward, ALL_TAGS);
        let mut backward: Vec<_> = set.names().rev().collect();
        backward.reverse();
        assert_eq!(backward, ALL_TAGS);
        let mut it = set.iter();
        assert_eq!(it.len(), TAG_COUNT);
        assert_eq!(it.next().map(Tag::id), Some(0));
        assert_eq!(it.next_back().map(Tag::id), Some(TAG_COUNT - 1));
        assert_eq!(it.len(), TAG_COUNT - 2);
        assert_eq!(it.by_ref().count(), TAG_COUNT - 2);
        assert_eq!(it.next(), None);
        assert_eq!(it.next_back(), None);
    }

    #[test]
    fn send_sync_copy() {
        fn assert_traits<T: Send + Sync + Copy + 'static>() {}
        assert_traits::<Tag>();
        assert_traits::<TagSet>();
        assert_eq!(size_of::<Tag>(), 2);
        assert_eq!(size_of::<TagSet>(), WORDS * 8);
    }
}
