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
