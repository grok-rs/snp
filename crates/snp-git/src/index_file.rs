//! A strict reader of git's index file, versions 2, 3 and 4 (ADR-134): every entry's cached stat data, mode, flags
//! and whether it names the empty blob, for the `DiffOracle`'s index-stat proof. No spawn.
//!
//! The reader is exact or refuses ([`IndexRefusal`]): it consumes exactly the header's entry count (v2/v3 entries
//! padded to 8 bytes, v4 names prefix-compressed), then extensions by size up to exactly the file length minus the
//! trailing hash. A split index (`link`), a sparse index (`sdir`), the fsmonitor extension (`FSMN`) and any unknown
//! extension whose signature starts with a lowercase letter (git dies on those) are refused; `TREE`, `REUC`, `UNTR`,
//! `EOIE`, `IEOT` and unknown uppercase (optional) extensions are skipped by size. Unknown entry flags, the extended
//! flag in a v2 index, a name length that disagrees with its NUL and a v4 prefix longer than the previous name refuse
//! too. The trailing checksum is not verified: git dies on a corrupt index with nothing on stdout, and
//! `index.skipHash` writes zeros. Every allocation is a `try_reserve`; no indexing, no unchecked arithmetic (S2).
//!
//! [`IndexFile::read_keep`] also keeps every entry's object id and the `TREE` extension's bytes (a second `TREE`
//! refuses), for ADR-153's in-process `write-tree` and `diff-index` ([`crate::cache_tree`]).
//!
//! Owner: WP-18.

use std::ffi::CStr;
use std::io::BufRead;
use std::path::Path;

/// `CE_VALID` (assume-unchanged) in the entry flags.
const CE_VALID: u16 = 0x8000;
/// `CE_EXTENDED`: a second 16-bit flags word follows (v3 and later).
const CE_EXTENDED: u16 = 0x4000;
/// The stage bits of the entry flags.
const CE_STAGEMASK: u16 = 0x3000;
/// The name-length bits of the entry flags; this value means "at least 0xFFF, find the NUL".
const CE_NAMEMASK: u16 = 0x0FFF;
/// `CE_SKIP_WORKTREE` in the second flags word.
const CE_SKIP_WORKTREE: u16 = 0x4000;
/// `CE_INTENT_TO_ADD` in the second flags word.
const CE_INTENT_TO_ADD: u16 = 0x2000;
/// The second word's bits git knows (`CE_EXTENDED_FLAGS`); any other bit makes git die.
const KNOWN_FLAGS2: u16 = CE_SKIP_WORKTREE | CE_INTENT_TO_ADD;
/// The fixed part of an entry before the object id: ten 32-bit stat and mode words.
const STAT_WORDS: usize = 40;

/// The empty blob in SHA-1 and SHA-256 repositories, raw.
const EMPTY_BLOB_SHA1: [u8; 20] = [
    0xe6, 0x9d, 0xe2, 0x9b, 0xb2, 0xd1, 0xd6, 0x43, 0x4b, 0x8b, 0x29, 0xae, 0x77, 0x5a, 0xd8, 0xc2,
    0xe4, 0x8c, 0x53, 0x91,
];
const EMPTY_BLOB_SHA256: [u8; 32] = [
    0x47, 0x3a, 0x0f, 0x4c, 0x3b, 0xe8, 0xa9, 0x36, 0x81, 0xa2, 0x67, 0xe3, 0xb1, 0xe9, 0xa7, 0xdc,
    0xda, 0x11, 0x85, 0x43, 0x6f, 0xe1, 0x41, 0xf7, 0x74, 0x91, 0x20, 0xa3, 0x03, 0x72, 0x18, 0x13,
];

/// Why the reader refused an index (every refusal means "no proof", never an error of the run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IndexRefusal {
    /// Not `DIRC`, or a version other than 2, 3 or 4.
    Header,
    /// The object id size is neither 20 nor 32 bytes.
    HashSize,
    /// The file ends inside the header, an entry or an extension, or bytes are left before the trailing hash.
    Truncated,
    /// An entry git would read differently or die on: unknown flags, the extended flag in v2, a name whose length
    /// field disagrees with its NUL, an empty name, a v4 prefix longer than the previous name.
    Entry,
    /// An extension the proof cannot follow (`link`, `sdir`, `FSMN`, or an unknown lowercase one).
    Extension([u8; 4]),
    /// A reservation failed or a size does not fit (S2).
    TooLarge,
    /// The index file could not be read.
    Io,
}

/// One index entry: its cached stat data as git wrote it (32-bit fields), mode, flags and name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::partial_pub_fields,
    reason = "the name is a span into its index's arena, read through IndexFile::name"
)]
pub struct IndexEntry {
    /// `ctime` seconds and nanoseconds.
    pub ctime: (u32, u32),
    /// `mtime` seconds and nanoseconds.
    pub mtime: (u32, u32),
    /// The device number, truncated to 32 bits.
    pub dev: u32,
    /// The inode number, truncated to 32 bits.
    pub ino: u32,
    /// The entry mode (`100644`, `100755`, `120000`, `160000`, or whatever was written).
    pub mode: u32,
    /// The owner, truncated to 32 bits.
    pub uid: u32,
    /// The group, truncated to 32 bits.
    pub gid: u32,
    /// The size as git stored it (truncated to 32 bits; `0` also for a racily smudged entry).
    pub size: u32,
    /// The first flags word (assume-valid, extended, stage, name length).
    pub flags: u16,
    /// The second flags word (skip-worktree, intent-to-add); 0 without the extended flag.
    pub flags2: u16,
    /// The object id is the empty blob's.
    pub empty_blob: bool,
    /// The name's start in [`IndexFile::names`].
    name_start: u32,
    /// The name's length (without its NUL).
    name_len: u32,
    /// The object id's start in [`IndexFile::oids`] ([`NOT_KEPT`] unless read with [`IndexFile::read_keep`]).
    oid_start: u32,
}

/// [`IndexEntry::oid_start`] of an index read without its object ids.
const NOT_KEPT: u32 = u32::MAX;

impl IndexEntry {
    /// The merge stage (0 for a merged entry).
    pub fn stage(&self) -> u16 {
        (self.flags & CE_STAGEMASK).checked_shr(12).unwrap_or(0)
    }

    /// Assume-unchanged (`CE_VALID`): git never compares the worktree for it.
    pub fn assume_valid(&self) -> bool {
        self.flags & CE_VALID != 0
    }

    /// Skip-worktree: git never compares the worktree for it.
    pub fn skip_worktree(&self) -> bool {
        self.flags2 & CE_SKIP_WORKTREE != 0
    }

    /// Intent-to-add (`git add -N`).
    pub fn intent_to_add(&self) -> bool {
        self.flags2 & CE_INTENT_TO_ADD != 0
    }
}

/// A parsed index: its version, entries in index order and their names.
#[derive(Debug, Clone, Default)]
#[expect(
    clippy::partial_pub_fields,
    reason = "the names arena is read through IndexFile::name and IndexFile::name_cstr"
)]
pub struct IndexFile {
    /// The format version (2, 3 or 4).
    pub version: u32,
    /// Every entry, in index order.
    pub entries: Vec<IndexEntry>,
    /// The signatures of the extensions skipped by size, in file order.
    pub extensions: Vec<[u8; 4]>,
    /// Every name followed by a NUL.
    names: Vec<u8>,
    /// ADR-153: every entry's raw object id, back to back (empty unless read with [`IndexFile::read_keep`]).
    oids: Vec<u8>,
    /// ADR-153: the object id length the index was read with (0 for a default value).
    hash_len: usize,
    /// ADR-153: the `TREE` extension's bytes, when read with [`IndexFile::read_keep`] and present.
    tree: Option<Vec<u8>>,
}

impl IndexFile {
    /// Reads and parses the index at `path` (`RepoInfo::index_file`, which honours `GIT_INDEX_FILE`) with object
    /// ids of `hash_len` bytes, streamed through a 64 KiB buffer: only the entries and their names are kept. A file
    /// whose length differs from its `fstat` size by the end refuses.
    pub fn read(path: &Path, hash_len: usize) -> Result<Self, IndexRefusal> {
        Self::read_with(path, hash_len, false)
    }

    /// [`Self::read`] that also keeps every entry's object id ([`Self::oid`]) and the `TREE` extension's bytes
    /// ([`Self::tree_extension`]), for ADR-153's in-process `write-tree` and `diff-index`.
    pub fn read_keep(path: &Path, hash_len: usize) -> Result<Self, IndexRefusal> {
        Self::read_with(path, hash_len, true)
    }

    fn read_with(path: &Path, hash_len: usize, keep: bool) -> Result<Self, IndexRefusal> {
        let f = std::fs::File::open(path).map_err(|_| IndexRefusal::Io)?;
        let len = f.metadata().map_err(|_| IndexRefusal::Io)?.len();
        let len = usize::try_from(len).map_err(|_| IndexRefusal::TooLarge)?;
        Self::parse_from(
            std::io::BufReader::with_capacity(64 * 1024, f),
            len,
            hash_len,
            keep,
        )
    }

    /// Parses an index image (see the module documentation).
    pub fn parse(buf: &[u8], hash_len: usize) -> Result<Self, IndexRefusal> {
        Self::parse_from(buf, buf.len(), hash_len, false)
    }

    /// [`Self::parse`] keeping the object ids and the `TREE` extension, as [`Self::read_keep`] does.
    pub fn parse_keep(buf: &[u8], hash_len: usize) -> Result<Self, IndexRefusal> {
        Self::parse_from(buf, buf.len(), hash_len, true)
    }

    /// Parses the `len` bytes `r` yields (and refuses when it yields more or fewer); `keep` keeps the object ids and
    /// the `TREE` extension.
    fn parse_from<R: BufRead>(
        r: R,
        len: usize,
        hash_len: usize,
        keep: bool,
    ) -> Result<Self, IndexRefusal> {
        let empty_blob: &[u8] = match hash_len {
            20 => &EMPTY_BLOB_SHA1,
            32 => &EMPTY_BLOB_SHA256,
            _ => return Err(IndexRefusal::HashSize),
        };
        let end = len.checked_sub(hash_len).ok_or(IndexRefusal::Truncated)?;
        let mut rd = Reader { r, pos: 0, end };
        if &rd.array::<4>()? != b"DIRC" {
            return Err(IndexRefusal::Header);
        }
        let version = rd.u32()?;
        if !(2..=4).contains(&version) {
            return Err(IndexRefusal::Header);
        }
        let count = usize::try_from(rd.u32()?).map_err(|_| IndexRefusal::TooLarge)?;
        // every entry takes at least the stat words, the id, the flags and one name byte with its NUL
        let min_entry = STAT_WORDS.saturating_add(hash_len).saturating_add(4);
        if count > end.checked_div(min_entry).unwrap_or(0) {
            return Err(IndexRefusal::Truncated);
        }
        let mut entries: Vec<IndexEntry> = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|_| IndexRefusal::TooLarge)?;
        let mut oids: Vec<u8> = Vec::new();
        if keep {
            oids.try_reserve_exact(count.checked_mul(hash_len).ok_or(IndexRefusal::TooLarge)?)
                .map_err(|_| IndexRefusal::TooLarge)?;
        }
        // an upper bound for v2/v3 (each name is followed by at least its NUL); untouched capacity is not resident,
        // and the arena is shrunk to its length at the end
        let mut names: Vec<u8> = Vec::new();
        names
            .try_reserve(end.saturating_sub(rd.pos))
            .map_err(|_| IndexRefusal::TooLarge)?;
        // v4 prefix compression can make the names longer than the file; far beyond any real index is refused
        let names_cap = len.saturating_mul(32).max(1 << 20);
        let mut prev: (usize, usize) = (0, 0);
        // the fixed part of an entry: the stat words, the id and the flags, read in one call
        let mut fixed_buf = [0_u8; STAT_WORDS + 32 + 2];
        let fixed_len = STAT_WORDS.saturating_add(hash_len).saturating_add(2);
        for _ in 0..count {
            let start = rd.pos;
            let fixed = fixed_buf
                .get_mut(..fixed_len)
                .ok_or(IndexRefusal::HashSize)?;
            rd.fill(fixed)?;
            let (stat, rest) = fixed
                .split_first_chunk::<STAT_WORDS>()
                .ok_or(IndexRefusal::Truncated)?;
            let (oid, flags) = rest
                .split_last_chunk::<2>()
                .ok_or(IndexRefusal::Truncated)?;
            let (words, _) = stat.as_chunks::<4>();
            let mut w = words.iter().map(|c| u32::from_be_bytes(*c));
            let mut next = || w.next().ok_or(IndexRefusal::Truncated);
            let ctime = (next()?, next()?);
            let mtime = (next()?, next()?);
            let dev = next()?;
            let ino = next()?;
            let mode = next()?;
            let uid = next()?;
            let gid = next()?;
            let size = next()?;
            let empty = oid == empty_blob;
            let flags = u16::from_be_bytes(*flags);
            let flags2 = if flags & CE_EXTENDED != 0 {
                if version < 3 {
                    return Err(IndexRefusal::Entry);
                }
                let f2 = rd.u16()?;
                if f2 & !KNOWN_FLAGS2 != 0 {
                    return Err(IndexRefusal::Entry);
                }
                f2
            } else {
                0
            };
            let len_field = usize::from(flags & CE_NAMEMASK);
            let name_start = names.len();
            if version == 4 {
                let strip = rd.varint()?;
                let keep = prev.1.checked_sub(strip).ok_or(IndexRefusal::Entry)?;
                let prefix_end = prev.0.checked_add(keep).ok_or(IndexRefusal::TooLarge)?;
                if names.len().saturating_add(keep) > names_cap {
                    return Err(IndexRefusal::TooLarge);
                }
                names
                    .try_reserve(keep)
                    .map_err(|_| IndexRefusal::TooLarge)?;
                names.extend_from_within(prev.0..prefix_end.min(names.len()));
                rd.cstr_into(&mut names, names_cap)?;
            } else {
                rd.cstr_into(&mut names, names_cap)?;
                // the padding to a multiple of 8 bytes from the entry's start (the NUL is its first byte)
                let used = rd.pos.saturating_sub(start).saturating_sub(1);
                let padded = used.checked_add(8).ok_or(IndexRefusal::TooLarge)? & !7;
                rd.skip(padded.saturating_sub(used).saturating_sub(1))?;
            }
            let name_len = names.len().saturating_sub(name_start).saturating_sub(1);
            let len_ok = if len_field == usize::from(CE_NAMEMASK) {
                name_len >= len_field
            } else {
                name_len == len_field
            };
            if name_len == 0 || !len_ok {
                return Err(IndexRefusal::Entry);
            }
            prev = (name_start, name_len);
            let oid_start = if keep {
                let at = u32::try_from(oids.len()).map_err(|_| IndexRefusal::TooLarge)?;
                if at == NOT_KEPT {
                    return Err(IndexRefusal::TooLarge);
                }
                // reserved for every entry above
                oids.extend_from_slice(oid);
                at
            } else {
                NOT_KEPT
            };
            entries.push(IndexEntry {
                ctime,
                mtime,
                dev,
                ino,
                mode,
                uid,
                gid,
                size,
                flags,
                flags2,
                empty_blob: empty,
                name_start: u32::try_from(name_start).map_err(|_| IndexRefusal::TooLarge)?,
                name_len: u32::try_from(name_len).map_err(|_| IndexRefusal::TooLarge)?,
                oid_start,
            });
        }
        let mut extensions: Vec<[u8; 4]> = Vec::new();
        let mut tree: Option<Vec<u8>> = None;
        while rd.pos < end {
            let sig = rd.array::<4>()?;
            let size = usize::try_from(rd.u32()?).map_err(|_| IndexRefusal::TooLarge)?;
            let known = matches!(&sig, b"TREE" | b"REUC" | b"UNTR" | b"EOIE" | b"IEOT");
            let optional = sig.first().is_some_and(u8::is_ascii_uppercase);
            if !known && (matches!(&sig, b"link" | b"sdir" | b"FSMN") || !optional) {
                return Err(IndexRefusal::Extension(sig));
            }
            if keep && &sig == b"TREE" {
                // git reads the first `TREE` only if there are two; a second one is a doubt
                if tree.is_some() {
                    return Err(IndexRefusal::Extension(sig));
                }
                // bounded by the file before anything is reserved
                rd.after(size)?;
                let mut body: Vec<u8> = Vec::new();
                body.try_reserve_exact(size)
                    .map_err(|_| IndexRefusal::TooLarge)?;
                body.resize(size, 0);
                rd.fill(&mut body)?;
                tree = Some(body);
            } else {
                rd.skip(size)?;
            }
            extensions
                .try_reserve(1)
                .map_err(|_| IndexRefusal::TooLarge)?;
            extensions.push(sig);
        }
        // the trailing hash, then nothing more
        rd.end = len;
        rd.skip(hash_len)?;
        if !rd.at_eof()? {
            return Err(IndexRefusal::Truncated);
        }
        names.shrink_to_fit();
        Ok(Self {
            version,
            entries,
            extensions,
            names,
            oids,
            hash_len,
            tree,
        })
    }

    /// The raw object id of `e` (an entry of this index); `None` unless the index was read with
    /// [`Self::read_keep`] or [`Self::parse_keep`].
    pub fn oid(&self, e: &IndexEntry) -> Option<&[u8]> {
        if e.oid_start == NOT_KEPT {
            return None;
        }
        let s = span(e.oid_start);
        self.oids.get(s..s.checked_add(self.hash_len)?)
    }

    /// The object id length the index was read with.
    pub fn hash_len(&self) -> usize {
        self.hash_len
    }

    /// The `TREE` (cache-tree) extension's bytes, when the index was read with [`Self::read_keep`] or
    /// [`Self::parse_keep`] and has one.
    pub fn tree_extension(&self) -> Option<&[u8]> {
        self.tree.as_deref()
    }

    /// The name of `e` (an entry of this index), without its NUL.
    pub fn name(&self, e: &IndexEntry) -> &[u8] {
        let (s, n) = (span(e.name_start), span(e.name_len));
        self.names.get(s..s.saturating_add(n)).unwrap_or_default()
    }

    /// The name of `e` with its NUL, ready for a `*at` system call; `None` when the name holds a NUL of its own.
    pub fn name_cstr(&self, e: &IndexEntry) -> Option<&CStr> {
        let (s, n) = (span(e.name_start), span(e.name_len));
        let with_nul = self.names.get(s..s.saturating_add(n).saturating_add(1))?;
        CStr::from_bytes_with_nul(with_nul).ok()
    }
}

/// A stored `u32` offset as `usize` (a 16-bit `usize` is not a target).
fn span(v: u32) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

/// A read error: the end of the input is a truncation, anything else an I/O refusal.
fn io_refusal(e: &std::io::Error) -> IndexRefusal {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        IndexRefusal::Truncated
    } else {
        IndexRefusal::Io
    }
}

/// A bounds-checked cursor over the index stream, never reading past `end` (the trailing hash's start).
struct Reader<R> {
    r: R,
    pos: usize,
    end: usize,
}

impl<R: BufRead> Reader<R> {
    /// The position after `n` more bytes, when they are before `end`.
    fn after(&self, n: usize) -> Result<usize, IndexRefusal> {
        self.pos
            .checked_add(n)
            .filter(|e| *e <= self.end)
            .ok_or(IndexRefusal::Truncated)
    }

    fn fill(&mut self, out: &mut [u8]) -> Result<(), IndexRefusal> {
        let e = self.after(out.len())?;
        self.r.read_exact(out).map_err(|e| io_refusal(&e))?;
        self.pos = e;
        Ok(())
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], IndexRefusal> {
        let mut b = [0_u8; N];
        self.fill(&mut b)?;
        Ok(b)
    }

    fn u32(&mut self) -> Result<u32, IndexRefusal> {
        Ok(u32::from_be_bytes(self.array::<4>()?))
    }

    fn u16(&mut self) -> Result<u16, IndexRefusal> {
        Ok(u16::from_be_bytes(self.array::<2>()?))
    }

    fn skip(&mut self, n: usize) -> Result<(), IndexRefusal> {
        let e = self.after(n)?;
        let mut left = n;
        while left > 0 {
            let got = self.r.fill_buf().map_err(|e| io_refusal(&e))?.len();
            if got == 0 {
                return Err(IndexRefusal::Truncated);
            }
            let k = got.min(left);
            self.r.consume(k);
            left = left.saturating_sub(k);
        }
        self.pos = e;
        Ok(())
    }

    /// Whether the input is exhausted.
    fn at_eof(&mut self) -> Result<bool, IndexRefusal> {
        Ok(self.r.fill_buf().map_err(|e| io_refusal(&e))?.is_empty())
    }

    /// Appends the bytes up to and including the next NUL to `names` (fallibly reserved, at most `cap` bytes in all).
    fn cstr_into(&mut self, names: &mut Vec<u8>, cap: usize) -> Result<(), IndexRefusal> {
        loop {
            let avail = self.end.saturating_sub(self.pos);
            let buf = self.r.fill_buf().map_err(|e| io_refusal(&e))?;
            let buf = buf.get(..buf.len().min(avail)).unwrap_or_default();
            if buf.is_empty() {
                return Err(IndexRefusal::Truncated);
            }
            let (k, done) = match memchr::memchr(0, buf) {
                Some(i) => (i.saturating_add(1), true),
                None => (buf.len(), false),
            };
            let part = buf.get(..k).unwrap_or_default();
            if names.len().saturating_add(k) > cap {
                return Err(IndexRefusal::TooLarge);
            }
            names.try_reserve(k).map_err(|_| IndexRefusal::TooLarge)?;
            names.extend_from_slice(part);
            self.r.consume(k);
            self.pos = self.pos.saturating_add(k);
            if done {
                return Ok(());
            }
        }
    }

    /// git's offset varint (`decode_varint`): seven bits per byte, most significant first, each continuation adding
    /// one; an overflow refuses.
    fn varint(&mut self) -> Result<usize, IndexRefusal> {
        let [mut c] = self.array::<1>()?;
        let mut val = usize::from(c & 0x7f);
        while c & 0x80 != 0 {
            val = val.checked_add(1).ok_or(IndexRefusal::Entry)?;
            if val.leading_zeros() < 7 {
                return Err(IndexRefusal::Entry);
            }
            [c] = self.array::<1>()?;
            val = val.checked_mul(0x80).ok_or(IndexRefusal::Entry)? | usize::from(c & 0x7f);
        }
        Ok(val)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A v2/v3 entry image: stat words, a 20-byte id, flags (and flags2), name, padding.
    fn entry(name: &[u8], flags_extra: u16, flags2: Option<u16>, size: u32) -> Vec<u8> {
        let mut e = Vec::new();
        for w in [1_u32, 2, 3, 4, 5, 6, 0o100_644, 7, 8, size] {
            e.extend_from_slice(&w.to_be_bytes());
        }
        e.extend_from_slice(&[0xab; 20]);
        let len = u16::try_from(name.len().min(0xfff)).unwrap();
        let ext = if flags2.is_some() { CE_EXTENDED } else { 0 };
        e.extend_from_slice(&(len | flags_extra | ext).to_be_bytes());
        if let Some(f2) = flags2 {
            e.extend_from_slice(&f2.to_be_bytes());
        }
        e.extend_from_slice(name);
        let used = e.len();
        let padded = (used + 8) & !7;
        e.resize(padded, 0);
        e
    }

    fn image(version: u32, entries: &[Vec<u8>], ext: &[u8]) -> Vec<u8> {
        let mut b = b"DIRC".to_vec();
        b.extend_from_slice(&version.to_be_bytes());
        b.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_be_bytes());
        for e in entries {
            b.extend_from_slice(e);
        }
        b.extend_from_slice(ext);
        b.extend_from_slice(&[0; 20]);
        b
    }

    fn ext(sig: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut e = sig.to_vec();
        e.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
        e.extend_from_slice(body);
        e
    }

    #[test]
    fn v2_entries_and_skipped_extensions() {
        let b = image(
            2,
            &[entry(b"a", 0, None, 3), entry(b"dir/bbbbbbb", 0, None, 0)],
            &[ext(*b"TREE", b"xyz"), ext(*b"ZZZZ", b"")].concat(),
        );
        let ix = IndexFile::parse(&b, 20).unwrap();
        assert_eq!(ix.version, 2);
        assert_eq!(ix.entries.len(), 2);
        assert_eq!(ix.name(&ix.entries[0]), b"a");
        assert_eq!(ix.name(&ix.entries[1]), b"dir/bbbbbbb");
        assert_eq!(
            ix.name_cstr(&ix.entries[1]).unwrap().to_bytes(),
            b"dir/bbbbbbb"
        );
        assert_eq!(ix.entries[0].mode, 0o100_644);
        assert_eq!(ix.entries[0].ctime, (1, 2));
        assert_eq!(ix.entries[0].mtime, (3, 4));
        assert_eq!(ix.extensions, vec![*b"TREE", *b"ZZZZ"]);
        assert!(!ix.entries[0].empty_blob);
    }

    #[test]
    fn refusals() {
        let ok = entry(b"a", 0, None, 1);
        for sig in [b"link", b"sdir", b"FSMN", b"abcd"] {
            let b = image(2, std::slice::from_ref(&ok), &ext(*sig, b""));
            assert_eq!(
                IndexFile::parse(&b, 20).unwrap_err(),
                IndexRefusal::Extension(*sig)
            );
        }
        // a byte left before the hash, a short extension, an entry cut short
        let mut b = image(2, std::slice::from_ref(&ok), b"");
        let at = b.len() - 20;
        b.splice(at..at, [0]);
        assert_eq!(
            IndexFile::parse(&b, 20).unwrap_err(),
            IndexRefusal::Truncated
        );
        let b = image(2, std::slice::from_ref(&ok), &ext(*b"TREE", b"abc")[..10]);
        assert_eq!(
            IndexFile::parse(&b, 20).unwrap_err(),
            IndexRefusal::Truncated
        );
        let mut b = image(2, std::slice::from_ref(&ok), b"");
        b[11] = 2;
        assert_eq!(
            IndexFile::parse(&b, 20).unwrap_err(),
            IndexRefusal::Truncated
        );
        // the extended flag in v2, an unknown second-word bit, a length field that disagrees
        let b = image(2, &[entry(b"a", 0, Some(0), 1)], b"");
        assert_eq!(IndexFile::parse(&b, 20).unwrap_err(), IndexRefusal::Entry);
        let b = image(3, &[entry(b"a", 0, Some(0x1000), 1)], b"");
        assert_eq!(IndexFile::parse(&b, 20).unwrap_err(), IndexRefusal::Entry);
        let mut e = entry(b"ab", 0, None, 1);
        e[61] = 1;
        assert_eq!(
            IndexFile::parse(&image(2, &[e], b""), 20).unwrap_err(),
            IndexRefusal::Entry
        );
        // header, version, hash size
        let mut b = image(2, std::slice::from_ref(&ok), b"");
        b[0] = b'X';
        assert_eq!(IndexFile::parse(&b, 20).unwrap_err(), IndexRefusal::Header);
        let b = image(5, std::slice::from_ref(&ok), b"");
        assert_eq!(IndexFile::parse(&b, 20).unwrap_err(), IndexRefusal::Header);
        assert_eq!(
            IndexFile::parse(&b, 21).unwrap_err(),
            IndexRefusal::HashSize
        );
        assert_eq!(
            IndexFile::parse(b"DI", 20).unwrap_err(),
            IndexRefusal::Truncated
        );
    }

    #[test]
    fn v3_flags() {
        let b = image(
            3,
            &[
                entry(b"a", CE_VALID, None, 1),
                entry(b"b", 0, Some(CE_SKIP_WORKTREE), 1),
                entry(b"c", 0, Some(CE_INTENT_TO_ADD), 0),
                entry(b"d", 0x2000, None, 1),
            ],
            b"",
        );
        let ix = IndexFile::parse(&b, 20).unwrap();
        let e = &ix.entries;
        assert!(e[0].assume_valid() && !e[0].skip_worktree());
        assert!(e[1].skip_worktree() && !e[1].intent_to_add());
        assert!(e[2].intent_to_add());
        assert_eq!(e[3].stage(), 2);
        assert_eq!(ix.name(&e[3]), b"d");
    }

    /// ADR-153: the object ids and the `TREE` extension are kept only on request; a second `TREE` refuses then.
    #[test]
    fn keeps_object_ids_and_the_tree_extension() {
        let b = image(
            2,
            &[entry(b"a", 0, None, 3), entry(b"b", 0, None, 1)],
            &[ext(*b"REUC", b"q"), ext(*b"TREE", b"xyz")].concat(),
        );
        let plain = IndexFile::parse(&b, 20).unwrap();
        assert_eq!(plain.oid(&plain.entries[0]), None);
        assert_eq!(plain.tree_extension(), None);
        let kept = IndexFile::parse_keep(&b, 20).unwrap();
        assert_eq!(kept.oid(&kept.entries[1]), Some(&[0xab; 20][..]));
        assert_eq!(kept.tree_extension(), Some(&b"xyz"[..]));
        assert_eq!(kept.hash_len(), 20);
        assert_eq!(kept.extensions, vec![*b"REUC", *b"TREE"]);
        let two = image(
            2,
            &[entry(b"a", 0, None, 3)],
            &[ext(*b"TREE", b""), ext(*b"TREE", b"")].concat(),
        );
        assert!(IndexFile::parse(&two, 20).is_ok());
        assert_eq!(
            IndexFile::parse_keep(&two, 20).unwrap_err(),
            IndexRefusal::Extension(*b"TREE")
        );
        // a TREE size beyond the file refuses before anything is reserved
        let mut huge = image(2, &[entry(b"a", 0, None, 3)], &ext(*b"TREE", b""));
        let at = huge.len() - 24;
        huge.splice(at..at + 4, u32::MAX.to_be_bytes());
        assert_eq!(
            IndexFile::parse_keep(&huge, 20).unwrap_err(),
            IndexRefusal::Truncated
        );
    }

    #[test]
    fn long_names() {
        let long = vec![b'x'; 0x1200];
        let b = image(2, &[entry(&long, 0, None, 1)], b"");
        let ix = IndexFile::parse(&b, 20).unwrap();
        assert_eq!(ix.name(&ix.entries[0]), &long[..]);
    }

    /// A v4 entry: no padding, the name as (strip varint, suffix, NUL).
    fn entry4(strip: &[u8], suffix: &[u8], full_len: usize) -> Vec<u8> {
        let mut e = Vec::new();
        for w in [1_u32, 2, 3, 4, 5, 6, 0o100_755, 7, 8, 9] {
            e.extend_from_slice(&w.to_be_bytes());
        }
        e.extend_from_slice(&EMPTY_BLOB_SHA1);
        e.extend_from_slice(&u16::try_from(full_len.min(0xfff)).unwrap().to_be_bytes());
        e.extend_from_slice(strip);
        e.extend_from_slice(suffix);
        e.push(0);
        e
    }

    #[test]
    fn v4_prefix_compression() {
        let b = image(
            4,
            &[
                entry4(&[0], b"dir/aaa", 7),
                entry4(&[3], b"bbbb", 8),
                entry4(&[8], b"z", 1),
                // a two-byte varint: 0x80 0x00 is 128
                entry4(&[1], &[b'q'; 130], 130),
                entry4(&[0x80, 0x00], b"r", 3),
            ],
            b"",
        );
        let ix = IndexFile::parse(&b, 20).unwrap();
        let names: Vec<&[u8]> = ix.entries.iter().map(|e| ix.name(e)).collect();
        assert_eq!(names[..3], [&b"dir/aaa"[..], b"dir/bbbb", b"z"]);
        assert_eq!(names[4], b"qqr");
        assert!(ix.entries[0].empty_blob);
        // a prefix longer than the previous name
        let b = image(4, &[entry4(&[0], b"a", 1), entry4(&[2], b"b", 1)], b"");
        assert_eq!(IndexFile::parse(&b, 20).unwrap_err(), IndexRefusal::Entry);
    }
}
