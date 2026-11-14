//! Plain data returned by the git calls, and the parsers of git's `-z` record formats.
//!
//! Owner: WP-18.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use crate::ZList;

/// `git rev-parse` discovery result (02 §3, 02 G.9 item 6).
#[derive(Clone, Debug)]
pub struct RepoInfo {
    /// Work-tree root: `os.path.abspath(git rev-parse --show-cdup)` (`git.py:51-73`), never canonicalised (Windows
    /// SUBST drives keep their letter).
    pub root: PathBuf,
    /// `--git-dir`, absolute (`git.get_git_dir`, `git.py:76-83`).
    pub git_dir: PathBuf,
    /// `--git-common-dir`, absolute (`git.get_git_common_dir`, `git.py:86-93`; equals `git_dir` on git < 2.5).
    pub common_dir: PathBuf,
    /// `--git-path index`: honours `GIT_INDEX_FILE` (`git.git_path`, `git.py:205-207`).
    pub index_file: PathBuf,
    /// A linked worktree (`git_dir != common_dir`).
    pub is_worktree: bool,
}

/// A git object id: 40 (SHA-1) or 64 (SHA-256) lower- or upper-case hex digits.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectId(Box<str>);

impl ObjectId {
    /// Validates `hex` (`git write-tree` / `rev-parse` output after `.strip()`).
    pub fn parse(hex: &str) -> Option<Self> {
        let ok = matches!(hex.len(), 40 | 64) && hex.bytes().all(|b| b.is_ascii_hexdigit());
        ok.then(|| Self(hex.into()))
    }

    /// The hex digits.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The all-zero id git uses for "no object" (`Z40`, and its 64-digit SHA-256 form; D-CRASH-10).
    pub fn is_null(&self) -> bool {
        is_null_oid(self.0.as_bytes())
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `sha == '0' * 40` generalised to SHA-256 (D-CRASH-10, ADR-022): 40 or 64 ASCII zeros.
pub(crate) fn is_null_oid(sha: &[u8]) -> bool {
    matches!(sha.len(), 40 | 64) && sha.iter().all(|&b| b == b'0')
}

/// `git diff-index --binary` outcome (`staged_files_only.py:50-104`, 02 §7.2).
#[derive(Debug)]
#[non_exhaustive]
pub enum DiffIndex {
    /// rc 0: nothing unstaged.
    Clean,
    /// rc 1 with `stdout.strip()` empty (CRLF quirk, stat-only dirtiness; 02 G.3).
    CrlfQuirk,
    /// rc 1 with a patch streamed into the sink.
    Patch {
        /// Bytes written.
        bytes_written: u64,
    },
}

/// Streaming blake3 of `git diff --no-ext-diff --no-textconv --ignore-submodules` output (`run.py:274-279`, 02 §12).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct DiffFingerprint {
    /// blake3 digest of the diff bytes.
    pub digest: [u8; 32],
    /// The diff was not empty.
    pub non_empty: bool,
}

impl DiffFingerprint {
    /// The fingerprint of an empty diff: blake3 of no bytes.
    pub const EMPTY: Self = Self {
        digest: [
            0xaf, 0x13, 0x49, 0xb9, 0xf5, 0xf9, 0xa1, 0xa6, 0xa0, 0x40, 0x4d, 0xea, 0x36, 0xdc,
            0xc9, 0x49, 0x9b, 0xcb, 0x25, 0xc9, 0xad, 0xc1, 0x12, 0xb7, 0xcc, 0x9a, 0x93, 0xca,
            0xe4, 0x1f, 0x32, 0x62,
        ],
        non_empty: false,
    };
}

/// `MERGE_MSG` ∧ `MERGE_HEAD` in the git dir (`git.is_in_merge_conflict`, `git.py:96-101`, 02 §9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MergeState {
    /// Not merging.
    None,
    /// A merge is in progress.
    Merging,
}

/// `hook_impl._pre_push_ns` result (`hook_impl.py:120-174`, 02 §16.3); `remote_name`/`remote_url` are hook-impl's
/// own arguments and are added by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrePushNs {
    /// `--all-files` (a push of a whole new history: the first new commit is a root).
    pub all_files: bool,
    /// `--from-ref`.
    pub from_ref: Option<OsString>,
    /// `--to-ref`.
    pub to_ref: Option<OsString>,
    /// `--local-branch` (git's local ref, may contain spaces).
    pub local_branch: OsString,
    /// `--remote-branch`.
    pub remote_branch: OsString,
}

/// Index entries as an arena (ADR-079): `ls-files -s -z` (all tracked: `DiffOracle` + git-mode tags) or
/// `diff --staged --raw -z --no-renames --diff-filter=ACMRTUXB` (staged only: git-mode tags when Stat mode is
/// impossible). `modes[i]` belongs to `paths.get(i)`; one buffer, no per-path allocation.
#[derive(Debug, Clone, Default)]
pub struct IndexEntries {
    /// The paths.
    pub paths: ZList,
    /// The modes (`0o100644`, `0o100755`, `0o120000`, `0o160000`, …).
    pub modes: Box<[u32]>,
}

/// `git ls-files -s -v -z` folded into the list [`crate::Git::all_files`] prints, with each path's index mode and
/// whether every entry is a plain cached one (ADR-106): what a clean `git diff` lets a `--all-files` run trust
/// instead of an `lstat` per file.
#[derive(Debug, Clone, Default)]
pub struct TaggedIndex {
    /// The path of every record, a run of equal paths (the stages of an unmerged entry) once, in index order: byte
    /// for byte the list of `ls-files -z --deduplicate`, in a compact buffer of its own (no mode or object text).
    pub paths: ZList,
    /// Per path, the mode of the first record of its run, or 0 when that record names the empty blob: `git diff`
    /// cannot tell an empty file from a FIFO or a socket of size 0 (it compares their empty content), so an empty
    /// entry proves nothing about the file's kind.
    pub modes: Box<[u32]>,
    /// Every record is tagged `H` (cached: neither assume-unchanged, skip-worktree, fsmonitor-valid nor unmerged)
    /// and has stage 0.
    pub plain: bool,
}

/// The object id of the empty blob (`git hash-object -t blob /dev/null`) in SHA-1 and SHA-256 repositories.
const EMPTY_BLOBS: [&[u8]; 2] = [
    b"e69de29bb2d1d6434b8b29ae775ad8c2e48c5391",
    b"473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813",
];

/// The incremental parser behind [`TaggedIndex`]: `git ls-files -s -v -z` streams through it (`io::Write`), so the
/// listing (about 80 bytes per entry) never sits in memory; only the paths are kept, NUL-separated as git printed
/// them. Records are `<tag> SP <mode> SP <object> SP <stage> TAB <path> NUL`, parsed by position with `memchr` (S2:
/// no indexing, no unchecked arithmetic): a letter, an octal mode, an object of 40 or 64 bytes without a space or a
/// TAB (it is only compared with the empty blob's id), a stage digit 0–3. A malformed record makes
/// [`TaggedParser::finish`] fail, so the caller asks `git ls-files -z --deduplicate` instead.
#[derive(Debug, Default)]
pub(crate) struct TaggedParser {
    /// The start of a record whose NUL has not arrived yet.
    carry: Vec<u8>,
    /// The kept paths, each followed by a NUL.
    buf: Vec<u8>,
    /// `(start, end)` of each kept path in `buf`.
    spans: Vec<(u32, u32)>,
    modes: Vec<u32>,
    /// A record other than `H` at stage 0 was seen.
    flagged: bool,
    /// The first malformed record (or the too-large marker); nothing is parsed after it.
    bad: Option<BadRecord>,
}

impl TaggedParser {
    /// Parses one complete record (without its NUL).
    fn record(&mut self, rec: &[u8]) -> Result<(), BadRecord> {
        let bad = || BadRecord(rec.to_vec());
        // `<tag> SP`: one letter
        let [tag, b' ', rest @ ..] = rec else {
            return Err(bad());
        };
        if !tag.is_ascii_alphabetic() {
            return Err(bad());
        }
        // `<mode> SP`: the modes git writes, else a few octal digits
        let (mode, rest) = match rest {
            [b'1', b'0', b'0', b'6', b'4', b'4', b' ', rest @ ..] => (0o100_644, rest),
            [b'1', b'0', b'0', b'7', b'5', b'5', b' ', rest @ ..] => (0o100_755, rest),
            [b'1', b'2', b'0', b'0', b'0', b'0', b' ', rest @ ..] => (0o120_000, rest),
            _ => {
                let sp = rest
                    .iter()
                    .take(8)
                    .position(|&b| b == b' ')
                    .ok_or_else(bad)?;
                let mode = octal(rest.get(..sp).unwrap_or_default()).ok_or_else(bad)?;
                (mode, rest.get(sp.saturating_add(1)..).unwrap_or_default())
            }
        };
        // `<object> SP`: 40 or 64 bytes (SHA-1 or SHA-256) without a space or a TAB inside
        let len = if rest.get(40) == Some(&b' ') {
            40
        } else if rest.get(64) == Some(&b' ') {
            64
        } else {
            return Err(bad());
        };
        let object = rest.get(..len).unwrap_or_default();
        if memchr::memchr2(b' ', b'\t', object).is_some() {
            return Err(bad());
        }
        let rest = rest.get(len.saturating_add(1)..).unwrap_or_default();
        // `<stage> TAB <path>`
        let [stage @ b'0'..=b'3', b'\t', path @ ..] = rest else {
            return Err(bad());
        };
        self.flagged |= *tag != b'H' || *stage != b'0';
        // A run of equal paths (the stages of an unmerged entry) is listed once, as `--deduplicate` does.
        if let Some(&(s, e)) = self.spans.last()
            && slice(&self.buf, s, e) == path
        {
            return Ok(());
        }
        let too_large = || BadRecord(b"git output larger than 4 GiB".to_vec());
        let start = u32::try_from(self.buf.len()).map_err(|_| too_large())?;
        let end = start
            .checked_add(u32::try_from(path.len()).map_err(|_| too_large())?)
            .ok_or_else(too_large)?;
        // the NUL after the path must fit as well: offsets stay below 4 GiB
        end.checked_add(1).ok_or_else(too_large)?;
        self.buf.extend_from_slice(path);
        self.buf.push(0);
        self.spans.push((start, end));
        // the first byte rules out nearly every object before any comparison
        let empty = matches!(object.first(), Some(b'e' | b'E' | b'4'))
            && EMPTY_BLOBS.iter().any(|e| object.eq_ignore_ascii_case(e));
        self.modes.push(if empty { 0 } else { mode });
        Ok(())
    }

    /// Takes the next chunk of git's stdout.
    pub(crate) fn feed(&mut self, mut chunk: &[u8]) {
        while self.bad.is_none() {
            let Some(nul) = memchr::memchr(0, chunk) else {
                self.carry.extend_from_slice(chunk);
                return;
            };
            let head = chunk.get(..nul).unwrap_or_default();
            let parsed = if self.carry.is_empty() {
                self.record(head)
            } else {
                // the record began in an earlier chunk; the carry keeps its capacity for the next one
                let mut rec = std::mem::take(&mut self.carry);
                rec.extend_from_slice(head);
                let parsed = self.record(&rec);
                rec.clear();
                self.carry = rec;
                parsed
            };
            if let Err(b) = parsed {
                self.bad = Some(b);
            }
            chunk = chunk.get(nul.saturating_add(1)..).unwrap_or_default();
        }
    }

    /// The listing, or the first malformed record. A last record without its NUL counts (as `git.zsplit` keeps
    /// it); an empty listing is an empty list.
    pub(crate) fn finish(mut self) -> Result<TaggedIndex, BadRecord> {
        if self.bad.is_none() && !self.carry.is_empty() {
            let rec = std::mem::take(&mut self.carry);
            if let Err(b) = self.record(&rec) {
                self.bad = Some(b);
            }
        }
        if let Some(b) = self.bad {
            return Err(b);
        }
        self.buf.shrink_to_fit();
        Ok(TaggedIndex {
            paths: ZList::from_parts(self.buf, self.spans),
            modes: self.modes.into_boxed_slice(),
            plain: !self.flagged,
        })
    }
}

impl std::io::Write for TaggedParser {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.feed(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The stat stamp of an index file ([`crate::Git::index_stamp`]): device, inode, size, and the modification and
/// status-change times in nanoseconds. Git replaces the index by renaming `index.lock` over it, so any rewrite gives
/// a new inode as well as new times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexStamp {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl IndexStamp {
    /// The stamp of `path` (Unix); `None` when it cannot be stat'ed or on other platforms.
    #[cfg(unix)]
    pub(crate) fn of(path: &std::path::Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt as _;
        let m = std::fs::metadata(path).ok()?;
        Some(Self {
            dev: m.dev(),
            ino: m.ino(),
            size: m.size(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        })
    }

    /// No stable inode or ctime outside Unix: never a stamp.
    #[cfg(not(unix))]
    pub(crate) fn of(_path: &std::path::Path) -> Option<Self> {
        None
    }
}

/// A record git printed in a shape the parser does not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BadRecord(pub(crate) Vec<u8>);

/// An octal mode field (`100644`); at most 7 digits so the value fits a `u32`.
fn octal(field: &[u8]) -> Option<u32> {
    if field.is_empty() || field.len() > 7 {
        return None;
    }
    field.iter().try_fold(0_u32, |acc, &b| {
        let d = b.checked_sub(b'0').filter(|d| *d < 8)?;
        acc.checked_mul(8)?.checked_add(u32::from(d))
    })
}

/// `buf[s..e]`, empty when out of range.
fn slice(buf: &[u8], s: u32, e: u32) -> &[u8] {
    match (usize::try_from(s), usize::try_from(e)) {
        (Ok(s), Ok(e)) => buf.get(s..e).unwrap_or_default(),
        _ => &[],
    }
}

fn u32_at(i: usize) -> Result<u32, BadRecord> {
    u32::try_from(i).map_err(|_| BadRecord(b"git output larger than 4 GiB".to_vec()))
}

/// `git ls-files -s -z`: `<mode> SP <object> SP <stage> TAB <path> NUL` per entry (unmerged paths appear once per
/// stage, in index order), and whether any record has a stage other than 0: `git ls-files --unmerged` prints exactly
/// those records, so its output is non-blank iff this is `true` (ADR-104). The stage is parsed strictly (one digit,
/// 0–3), over every raw record, before any caller filters or deduplicates.
pub(crate) fn parse_ls_files_stages(buf: Vec<u8>) -> Result<(IndexEntries, bool), BadRecord> {
    let records =
        ZList::from_z(buf).map_err(|_| BadRecord(b"git output larger than 4 GiB".to_vec()))?;
    let (buf, spans) = records.into_parts();
    let mut out_spans = Vec::new();
    let mut modes = Vec::new();
    let mut unmerged = false;
    for (s, e) in spans {
        let rec = slice(&buf, s, e);
        let bad = || BadRecord(rec.to_vec());
        let tab = memchr::memchr(b'\t', rec).ok_or_else(bad)?;
        let head = rec.get(..tab).unwrap_or_default();
        let mut fields = head.split(|&b| b == b' ');
        let mode = fields.next().and_then(octal).ok_or_else(bad)?;
        let _object = fields.next().ok_or_else(bad)?;
        let stage = match fields.next() {
            Some([d @ b'0'..=b'3']) => *d,
            _ => return Err(bad()),
        };
        if fields.next().is_some() {
            return Err(bad());
        }
        unmerged |= stage != b'0';
        let path_start = s
            .checked_add(u32_at(tab)?)
            .and_then(|v| v.checked_add(1))
            .ok_or_else(bad)?;
        out_spans.push((path_start, e));
        modes.push(mode);
    }
    Ok((
        IndexEntries {
            paths: ZList::from_parts(buf, out_spans),
            modes: modes.into_boxed_slice(),
        },
        unmerged,
    ))
}

/// [`parse_ls_files_stages`] without the unmerged flag.
pub(crate) fn parse_ls_files_stage(buf: Vec<u8>) -> Result<IndexEntries, BadRecord> {
    parse_ls_files_stages(buf).map(|(e, _)| e)
}

/// `git diff --staged --raw -z …`: `:<src mode> SP <dst mode> SP <src> SP <dst> SP <status> NUL <path> NUL`, with a
/// second path for copies and renames (`C`/`R`; never with `--no-renames`, handled anyway: the destination wins).
/// The index (destination) mode is kept.
pub(crate) fn parse_diff_raw(buf: Vec<u8>) -> Result<IndexEntries, BadRecord> {
    let records =
        ZList::from_z(buf).map_err(|_| BadRecord(b"git output larger than 4 GiB".to_vec()))?;
    let (buf, spans) = records.into_parts();
    let mut out_spans = Vec::new();
    let mut modes = Vec::new();
    let mut it = spans.into_iter();
    let field = |(s, e): (u32, u32)| slice(&buf, s, e);
    while let Some(head_span) = it.next() {
        let head = field(head_span);
        let body = head
            .strip_prefix(b":")
            .ok_or_else(|| BadRecord(head.to_vec()))?;
        let mut parts = body.split(|&b| b == b' ');
        let _src_mode = parts.next();
        let dst_mode = parts
            .next()
            .and_then(octal)
            .ok_or_else(|| BadRecord(head.to_vec()))?;
        let status = parts.nth(2).unwrap_or_default();
        let two_paths = matches!(status.first(), Some(b'R' | b'C'));
        if two_paths {
            let _source = it.next().ok_or_else(|| BadRecord(head.to_vec()))?;
        }
        let path = it.next().ok_or_else(|| BadRecord(head.to_vec()))?;
        out_spans.push(path);
        modes.push(dst_mode);
    }
    Ok(IndexEntries {
        paths: ZList::from_parts(buf, out_spans),
        modes: modes.into_boxed_slice(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_ids() {
        let z40: String = std::iter::repeat_n('0', 40).collect();
        let z64: String = std::iter::repeat_n('0', 64).collect();
        let sha: String = std::iter::repeat_n('a', 40).collect();
        assert!(ObjectId::parse(&z40).unwrap().is_null());
        assert!(ObjectId::parse(&z64).unwrap().is_null());
        let id = ObjectId::parse(&sha).unwrap();
        assert!(!id.is_null());
        assert_eq!(id.as_str(), sha);
        assert_eq!(id.to_string(), sha);
        assert!(ObjectId::parse("abc").is_none());
        assert!(ObjectId::parse(&std::iter::repeat_n('g', 40).collect::<String>()).is_none());
        assert!(!is_null_oid(b"0"));
        assert!(!is_null_oid(&[b'0'; 41]));
    }

    #[test]
    fn ls_files_stage_records() {
        let out = b"100644 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 0\ta b.txt\0\
                    100755 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 0\tbin/x\twith tab\0\
                    120000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 0\tlink\0\
                    160000 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 0\tsub\0";
        let (e, unmerged) = parse_ls_files_stages(out.to_vec()).unwrap();
        assert!(!unmerged);
        let paths: Vec<&[u8]> = e.paths.iter().collect();
        assert_eq!(
            paths,
            vec![&b"a b.txt"[..], b"bin/x\twith tab", b"link", b"sub"]
        );
        assert_eq!(&*e.modes, &[0o100_644, 0o100_755, 0o120_000, 0o160_000]);
        assert!(parse_ls_files_stage(b"garbage\0".to_vec()).is_err());
        assert!(parse_ls_files_stage(b"9x 0 0\tp\0".to_vec()).is_err());
        assert!(parse_ls_files_stage(Vec::new()).unwrap().paths.is_empty());
    }

    /// The stage field is strict and `unmerged` covers every raw record (ADR-104).
    #[test]
    fn ls_files_stage_unmerged() {
        let sha = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
        let rec = |mode: &str, stage: &str, path: &str| format!("{mode} {sha} {stage}\t{path}\0");
        for stage in ["1", "2", "3"] {
            let out = format!("{}{}", rec("100644", "0", "a"), rec("100644", stage, "b"));
            let (e, unmerged) = parse_ls_files_stages(out.into_bytes()).unwrap();
            assert!(unmerged, "stage {stage}");
            assert_eq!(e.paths.len(), 2);
        }
        // a gitlink conflict is unmerged as well
        let out = format!("{}{}", rec("160000", "2", "sub"), rec("160000", "3", "sub"));
        assert!(parse_ls_files_stages(out.into_bytes()).unwrap().1);
        for bad in ["4", "00", "", "x", "0 extra"] {
            let out = rec("100644", bad, "a");
            assert!(
                parse_ls_files_stages(out.into_bytes()).is_err(),
                "stage {bad:?}"
            );
        }
        assert!(parse_ls_files_stages(format!("100644 {sha}\ta\0").into_bytes()).is_err());
    }

    fn tagged(out: &[u8]) -> Result<TaggedIndex, BadRecord> {
        let mut p = TaggedParser::default();
        p.feed(out);
        p.finish()
    }

    const SHA: &str = "78981922613b2afb6025042ff6bd878ac1994e85";
    const EMPTY: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";

    fn trec(tag: &str, mode: &str, object: &str, stage: &str, path: &str) -> String {
        format!("{tag} {mode} {object} {stage}\t{path}\0")
    }

    /// ADR-106: tags, modes, the empty blob, stages and the `--deduplicate` fold of unmerged runs.
    #[test]
    fn ls_files_tagged_records() {
        let out = [
            trec("H", "100644", SHA, "0", "a b.txt"),
            trec("H", "100755", SHA, "0", "bin/x\twith tab"),
            trec("H", "120000", SHA, "0", "link"),
            trec("H", "160000", SHA, "0", "sub"),
            trec("H", "100644", EMPTY, "0", "__init__.py"),
        ]
        .concat();
        let t = tagged(out.as_bytes()).unwrap();
        assert!(t.plain);
        let paths: Vec<&[u8]> = t.paths.iter().collect();
        assert_eq!(
            paths,
            vec![
                &b"a b.txt"[..],
                b"bin/x\twith tab",
                b"link",
                b"sub",
                b"__init__.py"
            ]
        );
        assert_eq!(&*t.modes, &[0o100_644, 0o100_755, 0o120_000, 0o160_000, 0]);
        // the paths keep git's NUL separators and nothing else of the listing
        let (buf, _) = t.paths.into_parts();
        assert_eq!(buf, b"a b.txt\0bin/x\twith tab\0link\0sub\0__init__.py\0");
        // the SHA-256 empty blob is empty too
        let sha256_empty = "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813";
        let t = tagged(trec("H", "100644", sha256_empty, "0", "e").as_bytes()).unwrap();
        assert_eq!(&*t.modes, &[0]);
        // every tag other than `H`, and every stage other than 0, clears `plain`
        for (tag, stage) in [("h", "0"), ("S", "0"), ("s", "0"), ("M", "1"), ("H", "2")] {
            let out = [
                trec("H", "100644", SHA, "0", "a"),
                trec(tag, "100644", SHA, stage, "b"),
            ]
            .concat();
            assert!(!tagged(out.as_bytes()).unwrap().plain, "{tag} {stage}");
        }
        // an unmerged entry is listed once, with its first stage's mode
        let out = [
            trec("H", "100644", SHA, "0", "a"),
            trec("M", "100644", SHA, "1", "f"),
            trec("M", "100755", SHA, "2", "f"),
            trec("M", "100644", SHA, "3", "f"),
            trec("H", "100644", SHA, "0", "g"),
        ]
        .concat();
        let t = tagged(out.as_bytes()).unwrap();
        assert_eq!(t.paths.iter().collect::<Vec<_>>(), [&b"a"[..], b"f", b"g"]);
        assert_eq!(&*t.modes, &[0o100_644, 0o100_644, 0o100_644]);
        assert!(!t.plain);
        // empty output: an empty list
        let t = tagged(b"").unwrap();
        assert!(t.paths.is_empty() && t.modes.is_empty() && t.plain);
        // a last record without its NUL still counts (`git.zsplit`)
        let t = tagged(
            trec("H", "100644", SHA, "0", "x")
                .trim_end_matches('\0')
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(t.paths.iter().collect::<Vec<_>>(), [&b"x"[..]]);
    }

    /// Every malformed shape fails (the caller then asks `ls-files -z --deduplicate`), never panics.
    #[test]
    fn ls_files_tagged_rejects_malformed_records() {
        for bad in [
            "garbage\0".to_owned(),
            "\0".to_owned(),
            format!("100644 {SHA} 0\ta\0"),
            format!("HH 100644 {SHA} 0\ta\0"),
            format!("1 100644 {SHA} 0\ta\0"),
            format!("H 9x {SHA} 0\ta\0"),
            format!("H 100644 {} 0\ta\0", SHA.get(..39).unwrap()),
            format!("H 100644 {}\t 0\ta\0", SHA.get(..39).unwrap()),
            format!("H 100644 {SHA}0 0\ta\0"),
            format!("H 100644 {SHA} 4\ta\0"),
            format!("H 100644 {SHA} 00\ta\0"),
            format!("H 100644 {SHA} 0 x\ta\0"),
            format!("H 100644 {SHA} 0a\0"),
            format!("{}garbage\0", trec("H", "100644", SHA, "0", "a")),
        ] {
            assert!(tagged(bad.as_bytes()).is_err(), "{bad:?}");
        }
    }

    /// Fed in chunks of any size, the parser gives what one write gives.
    #[test]
    fn ls_files_tagged_chunking_is_invisible() {
        let out = [
            trec("H", "100644", SHA, "0", "a"),
            trec("M", "100644", SHA, "1", "dé/f"),
            trec("M", "100755", SHA, "2", "dé/f"),
            trec("S", "120000", SHA, "0", "link"),
            trec("H", "100644", EMPTY, "0", "e"),
        ]
        .concat();
        let whole = tagged(out.as_bytes()).unwrap();
        for size in 1..=out.len() {
            let mut p = TaggedParser::default();
            let mut rest = out.as_bytes();
            while !rest.is_empty() {
                let (chunk, tail) = rest.split_at_checked(size.min(rest.len())).unwrap();
                std::io::Write::write_all(&mut p, chunk).unwrap();
                rest = tail;
            }
            let t = p.finish().unwrap();
            assert_eq!(t.paths, whole.paths, "chunk size {size}");
            assert_eq!(t.modes, whole.modes);
            assert_eq!(t.plain, whole.plain);
        }
    }

    #[test]
    fn diff_raw_records() {
        let out = b":000000 100644 0000000 e69de29 A\0new.txt\0\
                    :100644 100755 e69de29 e69de29 M\0mode\0\
                    :100644 100644 e69de29 e69de29 R100\0old\0renamed\0";
        let e = parse_diff_raw(out.to_vec()).unwrap();
        let paths: Vec<&[u8]> = e.paths.iter().collect();
        assert_eq!(paths, vec![&b"new.txt"[..], b"mode", b"renamed"]);
        assert_eq!(&*e.modes, &[0o100_644, 0o100_755, 0o100_644]);
        assert!(parse_diff_raw(b"no-colon\0x\0".to_vec()).is_err());
        assert!(parse_diff_raw(b":100644\0x\0".to_vec()).is_err());
        assert!(parse_diff_raw(b":100644 100644 a b M\0".to_vec()).is_err());
    }

    #[test]
    fn octal_fields() {
        assert_eq!(octal(b"100644"), Some(0o100_644));
        assert_eq!(octal(b""), None);
        assert_eq!(octal(b"8"), None);
        assert_eq!(octal(b"77777777"), None);
    }
}
