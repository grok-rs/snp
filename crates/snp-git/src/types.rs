//! Plain data returned by the git calls, and the parsers of git's `-z` record formats.
//!
//! Owner: WP-18.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

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

/// Index entries as an arena (ADR-079): `ls-files -s -v -z` (all tracked: `DiffOracle` + git-mode tags) or
/// `diff --staged --raw -z --no-renames --diff-filter=ACMRTUXB` (staged only: git-mode tags when Stat mode is
/// impossible). `modes[i]` and `tags[i]` belong to `paths.get(i)`; one buffer, no per-path allocation.
#[derive(Debug, Clone, Default)]
pub struct IndexEntries {
    /// The paths.
    pub paths: ZList,
    /// The modes (`0o100644`, `0o100755`, `0o120000`, `0o160000`, …).
    pub modes: Box<[u32]>,
    /// The `ls-files -v` tag of each entry (ADR-133): `H` cached, `S` skip-worktree, `M` unmerged, lowercase when
    /// assume-unchanged. Empty when the listing had no tags (the staged list): a caller that needs them fails closed.
    pub tags: Box<[u8]>,
}

/// `git config --list --show-origin -z`, read once (fan-out query G, ADR-104; ADR-119): the origin files the
/// `DiffOracle` watches, and what the effective configuration says about a checkout's effect on `git diff`.
#[derive(Debug, Clone, Default)]
pub struct ConfigListing {
    /// The distinct origin files in first-seen order, relative ones joined to the root ([`crate::Git::config_origins`]).
    pub files: Arc<[PathBuf]>,
    /// ADR-119's facts, from the same records.
    pub checkout: CheckoutConfig,
}

/// Where git reads its system attributes file, as `git var GIT_ATTR_SYSTEM` (git ≥ 2.42) answered for the run's
/// executable and environment (ADR-133). Never kept across runs: on macOS `/usr/bin/git` is an `xcrun` shim whose
/// target follows `DEVELOPER_DIR` and `xcode-select`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SystemAttributes {
    /// git reads this file (absolute; it need not exist).
    File(PathBuf),
    /// `GIT_ATTR_NOSYSTEM` is in the environment and git reads none.
    Disabled,
}

/// [`SystemAttributes`] from `git var GIT_ATTR_SYSTEM`'s exit status and stdout; `nosystem` = `GIT_ATTR_NOSYSTEM` is in
/// its environment. Only exit 0 with exactly one line (ending in a newline, no other newline) holding an absolute
/// path, or exit 1 with `nosystem`, is an answer; anything else (git < 2.42 exits 129, a bad boolean in
/// `GIT_ATTR_NOSYSTEM` 128, a spawn failure, a relative path) is `None`: the caller does without (fail closed).
pub(crate) fn system_attributes(
    status: i64,
    stdout: &[u8],
    nosystem: bool,
) -> Option<SystemAttributes> {
    match status {
        0 => {
            let line = stdout.strip_suffix(b"\n")?;
            if line.is_empty() || line.contains(&b'\n') || line.contains(&0) {
                return None;
            }
            let path = PathBuf::from(crate::run::os_from_bytes(line));
            path.is_absolute().then_some(SystemAttributes::File(path))
        }
        1 if nosystem && stdout.is_empty() => Some(SystemAttributes::Disabled),
        _ => None,
    }
}

/// What the effective configuration (the last value of each key, as git resolves it; `-c` and `GIT_CONFIG_*` count)
/// says about whether a path-limited `git checkout` of index entries can leave `git diff` non-empty (ADR-119). Fail
/// closed: a value whose meaning is not certain makes [`CheckoutConfig::plain`] false. The default is not plain.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts of one config listing, each read by a different proof"
)]
pub struct CheckoutConfig {
    /// `core.autocrlf`, `core.fsmonitor`, `core.useBuiltinFSMonitor`, `core.ignoreStat`, `core.sparseCheckout` and
    /// `index.sparse` are unset or read as git's `false`; `core.fileMode` is unset or reads as a boolean;
    /// `core.hooksPath`, when set, has a value; no `hook.*` key (a configuration-based hook) is set; and neither
    /// `core.attributesFile` nor `attr.tree` is set at all. The last two are git's own word on which attributes apply:
    /// the `DiffOracle`'s config reader is best effort (a UTF-8 BOM, a `%(prefix)/` value), so ADR-119 does not let
    /// a file it read as missing prove that no attributes file exists.
    pub plain: bool,
    /// `core.fileMode`: `true` unless it reads as `false` (git's default; meaningful only when `plain`).
    pub file_mode: bool,
    /// The last `core.hooksPath` value as git printed it, `None` when unset.
    pub hooks_path: Option<Box<[u8]>>,
    /// `core.eol` is unset, or its last value is `lf` or `native` (git compares them ignoring ASCII case): with
    /// `core.autocrlf` off ([`Self::plain`]) and off Windows, a `text=auto` path is checked out as its blob's bytes
    /// (ADR-133). `false` for `crlf`, a valueless key or anything else.
    pub eol_lf: bool,
    /// `core.fsmonitor`, `core.useBuiltinFSMonitor`, `core.ignoreStat`, `core.sparseCheckout` and `index.sparse` are
    /// unset or read as git's `false`, and `core.worktree` is not set at all (a hook that sets it moves the worktree
    /// `git diff` compares): what the index-stat proof needs from the configuration (ADR-134).
    pub index_proof: bool,
    /// The object id length of the repository's index entries: 20 when `extensions.objectFormat` is unset or `sha1`,
    /// 32 for `sha256` (git compares the names exactly), `None` for anything else.
    pub object_id_len: Option<usize>,
}

impl Default for CheckoutConfig {
    fn default() -> Self {
        Self {
            plain: false,
            file_mode: true,
            hooks_path: None,
            eol_lf: false,
            index_proof: false,
            object_id_len: None,
        }
    }
}

/// git's boolean reading of a config value (`git_parse_maybe_bool` followed by `git_parse_int`): a valueless key is
/// true; `true`/`yes`/`on` and `false`/`no`/`off`/the empty string, in any case; a run of decimal digits is false
/// exactly when every digit is `0`. Anything else (a sign, a unit suffix, `input`, a hook path) is `None`: not
/// certain, the caller fails closed.
fn config_bool(value: Option<&[u8]>) -> Option<bool> {
    let Some(v) = value else {
        return Some(true);
    };
    let lower = v.to_ascii_lowercase();
    match lower.as_slice() {
        b"true" | b"yes" | b"on" => Some(true),
        b"false" | b"no" | b"off" | b"" => Some(false),
        digits if digits.iter().all(u8::is_ascii_digit) => Some(digits.iter().any(|&d| d != b'0')),
        _ => None,
    }
}

/// [`CheckoutConfig`] from the `key\nvalue` (or valueless `key`) records of `git config --list -z`, in config order.
/// Keys come lowercased in their section and variable names; a subsection keeps its case.
pub(crate) fn checkout_config<'a>(records: impl Iterator<Item = &'a [u8]>) -> CheckoutConfig {
    const OFF: [&[u8]; 6] = [
        b"core.autocrlf",
        b"core.fsmonitor",
        b"core.usebuiltinfsmonitor",
        b"core.ignorestat",
        b"core.sparsecheckout",
        b"index.sparse",
    ];
    // the last value of each key that matters: `None` = a valueless key
    let mut last: Vec<(&[u8], Option<&[u8]>)> = Vec::new();
    let mut hook_key = false;
    let mut attributes_key = false;
    let mut worktree_key = false;
    for r in records {
        let (key, value) = match memchr::memchr(b'\n', r) {
            Some(nl) => (
                r.get(..nl).unwrap_or_default(),
                Some(r.get(nl.saturating_add(1)..).unwrap_or_default()),
            ),
            None => (r, None),
        };
        if key.starts_with(b"hook.") {
            hook_key = true;
        }
        if key == b"core.attributesfile" || key == b"attr.tree" {
            attributes_key = true;
        }
        if key == b"core.worktree" {
            worktree_key = true;
        }
        if OFF.contains(&key)
            || key == b"core.filemode"
            || key == b"core.hookspath"
            || key == b"core.eol"
            || key == b"extensions.objectformat"
        {
            match last.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => last.push((key, value)),
            }
        }
    }
    let get = |key: &[u8]| last.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    let off = OFF
        .iter()
        .all(|k| get(k).is_none_or(|v| config_bool(v) == Some(false)));
    let file_mode = get(b"core.filemode").map_or(Some(true), config_bool);
    let hooks_path = get(b"core.hookspath");
    let eol_lf = get(b"core.eol").is_none_or(|v| {
        v.is_some_and(|v| v.eq_ignore_ascii_case(b"lf") || v.eq_ignore_ascii_case(b"native"))
    });
    let index_proof = !worktree_key
        && OFF
            .iter()
            .filter(|k| **k != b"core.autocrlf")
            .all(|k| get(k).is_none_or(|v| config_bool(v) == Some(false)));
    let object_id_len = match get(b"extensions.objectformat") {
        None | Some(Some(b"sha1")) => Some(20),
        Some(Some(b"sha256")) => Some(32),
        Some(_) => None,
    };
    CheckoutConfig {
        index_proof,
        object_id_len,
        plain: off
            && !hook_key
            && !attributes_key
            && file_mode.is_some()
            && hooks_path.is_none_or(|v| v.is_some()),
        file_mode: file_mode.unwrap_or(true),
        hooks_path: hooks_path.flatten().map(Box::from),
        eol_lf,
    }
}

/// `git ls-files -s -v -f -z` folded into the list [`crate::Git::all_files`] prints, with each path's index mode and
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
    /// Every record is tagged `H` (cached: neither assume-unchanged (`h` under `-v`), fsmonitor-valid (`h` under
    /// `-f`), skip-worktree (`S`) nor unmerged) and has stage 0.
    pub plain: bool,
}

/// The object id of the empty blob (`git hash-object -t blob /dev/null`) in SHA-1 and SHA-256 repositories.
const EMPTY_BLOBS: [&[u8]; 2] = [
    b"e69de29bb2d1d6434b8b29ae775ad8c2e48c5391",
    b"473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813",
];

/// The incremental parser behind [`TaggedIndex`]: `git ls-files -s -v -f -z` streams through it (`io::Write`), so the
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
    parse_stages(buf, false)
}

/// [`parse_ls_files_stages`] over `git ls-files -s -v -z` (query F, ADR-133): each record starts with its tag, one
/// ASCII letter and a space, kept in [`IndexEntries::tags`].
pub(crate) fn parse_ls_files_tagged_stages(
    buf: Vec<u8>,
) -> Result<(IndexEntries, bool), BadRecord> {
    parse_stages(buf, true)
}

/// [`parse_ls_files_tagged_stages`] without the unmerged flag.
pub(crate) fn parse_ls_files_tagged(buf: Vec<u8>) -> Result<IndexEntries, BadRecord> {
    parse_stages(buf, true).map(|(e, _)| e)
}

fn parse_stages(buf: Vec<u8>, tagged: bool) -> Result<(IndexEntries, bool), BadRecord> {
    let records =
        ZList::from_z(buf).map_err(|_| BadRecord(b"git output larger than 4 GiB".to_vec()))?;
    let (buf, spans) = records.into_parts();
    let mut out_spans = Vec::new();
    let mut modes = Vec::new();
    let mut tags = Vec::new();
    let mut unmerged = false;
    for (s, e) in spans {
        let rec = slice(&buf, s, e);
        let bad = || BadRecord(rec.to_vec());
        let tab = memchr::memchr(b'\t', rec).ok_or_else(bad)?;
        let mut head = rec.get(..tab).unwrap_or_default();
        if tagged {
            match head {
                [tag, b' ', rest @ ..] if tag.is_ascii_alphabetic() => {
                    tags.push(*tag);
                    head = rest;
                }
                _ => return Err(bad()),
            }
        }
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
            tags: tags.into_boxed_slice(),
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
        tags: Box::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-119: git's boolean reading; anything else is not certain.
    #[test]
    fn config_bool_reads_like_git() {
        for (v, want) in [
            (None, Some(true)),
            (Some(&b"true"[..]), Some(true)),
            (Some(b"Yes"), Some(true)),
            (Some(b"ON"), Some(true)),
            (Some(b"1"), Some(true)),
            (Some(b"0010"), Some(true)),
            (Some(b"false"), Some(false)),
            (Some(b"No"), Some(false)),
            (Some(b"off"), Some(false)),
            (Some(b""), Some(false)),
            (Some(b"0"), Some(false)),
            (Some(b"000"), Some(false)),
            (Some(b"input"), None),
            (Some(b"-1"), None),
            (Some(b"1k"), None),
            (Some(b" false"), None),
            (Some(b".git/hooks/query-watchman"), None),
        ] {
            assert_eq!(config_bool(v), want, "{v:?}");
        }
    }

    /// ADR-119: the last value of each key decides; every listed key must read false (or be unset), `core.fileMode`
    /// a boolean, `core.hooksPath` a value; any `hook.*` key turns the proof off.
    #[test]
    fn checkout_config_is_plain_only_when_certain() {
        let c = |recs: &[&[u8]]| checkout_config(recs.iter().copied());
        let plain = c(&[]);
        assert!(plain.plain && plain.file_mode && plain.hooks_path.is_none());
        assert_eq!(c(&[b"user.name\nx", b"core.bare\nfalse"]), plain);
        for key in [
            "core.autocrlf",
            "core.fsmonitor",
            "core.usebuiltinfsmonitor",
            "core.ignorestat",
            "core.sparsecheckout",
            "index.sparse",
        ] {
            for bad in ["true", "input", "/hook"] {
                let r = format!("{key}\n{bad}");
                assert!(!c(&[r.as_bytes()]).plain, "{r}");
            }
            assert!(!c(&[key.as_bytes()]).plain, "a valueless {key} is true");
            let off = format!("{key}\nfalse");
            assert!(c(&[off.as_bytes()]).plain, "{off}");
            let on = format!("{key}\ntrue");
            assert!(
                c(&[on.as_bytes(), off.as_bytes()]).plain,
                "the last value wins"
            );
            assert!(
                !c(&[off.as_bytes(), on.as_bytes()]).plain,
                "the last value wins"
            );
        }
        let fm = c(&[b"core.filemode\nfalse"]);
        assert!(fm.plain && !fm.file_mode);
        assert!(c(&[b"core.filemode\nfalse", b"core.filemode\ntrue"]).file_mode);
        assert!(!c(&[b"core.filemode\nmaybe"]).plain);
        for hook in [
            &b"hook.lint.command\ntrue"[..],
            b"hook.Lint.event\npost-checkout",
            b"hook.x",
        ] {
            assert!(!c(&[hook]).plain, "{hook:?}");
        }
        assert!(c(&[b"hooks.x\ny", b"githook.x\ny"]).plain);
        // git's listing, not the oracle's reader, says whether an attributes source is configured; any value counts
        for attrs in [
            &b"core.attributesfile\n/a"[..],
            b"core.attributesfile\n%(prefix)/etc/attrs",
            b"core.attributesfile\n",
            b"core.attributesfile",
            b"attr.tree\nHEAD",
            b"attr.tree",
        ] {
            assert!(!c(&[attrs]).plain, "{attrs:?}");
        }
        assert!(c(&[b"core.attributes\nx", b"attr.trees\nx"]).plain);
        let hp = c(&[b"core.hookspath\n/a", b"core.hookspath\n.githooks"]);
        assert!(hp.plain);
        assert_eq!(hp.hooks_path.as_deref(), Some(&b".githooks"[..]));
        assert!(
            !c(&[b"core.hookspath"]).plain,
            "a valueless hooksPath is an error"
        );
        assert!(!CheckoutConfig::default().plain);
    }

    /// ADR-134: the index-stat proof's gate is `plain`'s fsmonitor, ignoreStat and sparse keys (not autocrlf, hooks or
    /// attributes), and the object id length follows `extensions.objectFormat` exactly.
    #[test]
    fn checkout_config_reads_the_index_proof_gate() {
        let c = |recs: &[&[u8]]| checkout_config(recs.iter().copied());
        let d = c(&[]);
        assert!(d.index_proof);
        assert_eq!(d.object_id_len, Some(20));
        for key in [
            "core.fsmonitor",
            "core.usebuiltinfsmonitor",
            "core.ignorestat",
            "core.sparsecheckout",
            "index.sparse",
        ] {
            for bad in ["true", "/hook", ""] {
                let r = if bad.is_empty() {
                    key.to_owned()
                } else {
                    format!("{key}\n{bad}")
                };
                assert!(!c(&[r.as_bytes()]).index_proof, "{r}");
            }
            let off = format!("{key}\nfalse");
            assert!(c(&[off.as_bytes()]).index_proof, "{off}");
        }
        assert!(!c(&[b"core.worktree\n/elsewhere"]).index_proof);
        assert!(!c(&[b"core.worktree"]).index_proof);
        for other in [
            &b"core.autocrlf\ntrue"[..],
            b"hook.x.command\ny",
            b"core.attributesfile\n/a",
            b"core.filemode\nmaybe",
        ] {
            let got = c(&[other]);
            assert!(got.index_proof && !got.plain, "{other:?}");
        }
        assert!(!CheckoutConfig::default().index_proof);
        assert_eq!(CheckoutConfig::default().object_id_len, None);
        let len = |v: &[u8]| c(&[v]).object_id_len;
        assert_eq!(len(b"extensions.objectformat\nsha1"), Some(20));
        assert_eq!(len(b"extensions.objectformat\nsha256"), Some(32));
        assert_eq!(len(b"extensions.objectformat\nSHA256"), None);
        assert_eq!(len(b"extensions.objectformat"), None);
        assert_eq!(len(b"extensions.objectformat\nsha3"), None);
    }

    /// ADR-133: `core.eol` unset, `lf` or `native` (any ASCII case, the last value) lets `text=auto` check out raw
    /// bytes; `crlf`, a valueless key or anything else does not, and never changes `plain`.
    #[test]
    fn checkout_config_reads_core_eol() {
        let c = |recs: &[&[u8]]| checkout_config(recs.iter().copied());
        assert!(c(&[]).eol_lf);
        for ok in [
            &b"core.eol
lf"[..],
            b"core.eol
LF",
            b"core.eol
Native",
        ] {
            let got = c(&[ok]);
            assert!(got.eol_lf && got.plain, "{ok:?}");
        }
        for bad in [
            &b"core.eol
crlf"[..],
            b"core.eol",
            b"core.eol
",
            b"core.eol
lf ",
        ] {
            let got = c(&[bad]);
            assert!(!got.eol_lf && got.plain, "{bad:?}");
        }
        assert!(
            c(&[
                b"core.eol
crlf",
                b"core.eol
lf"
            ])
            .eol_lf
        );
        assert!(
            !c(&[
                b"core.eol
lf",
                b"core.eol
crlf"
            ])
            .eol_lf
        );
        assert!(!CheckoutConfig::default().eol_lf);
    }

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

    /// ADR-133: only exit 0 with one absolute line, or exit 1 under `GIT_ATTR_NOSYSTEM`, answers.
    #[test]
    fn system_attributes_answers() {
        let file = |p: &str| Some(SystemAttributes::File(PathBuf::from(p)));
        assert_eq!(
            system_attributes(0, b"/etc/gitattributes\n", false),
            file("/etc/gitattributes")
        );
        assert_eq!(
            system_attributes(0, b"/opt/a b/attrs\n", true),
            file("/opt/a b/attrs")
        );
        assert_eq!(
            system_attributes(1, b"", true),
            Some(SystemAttributes::Disabled)
        );
        for (status, out, nosystem) in [
            (1, &b""[..], false),
            (1, b"/etc/gitattributes\n", true),
            (0, b"/etc/gitattributes", false),
            (0, b"/etc/a\n/etc/b\n", false),
            (0, b"/etc/a\n\n", false),
            (0, b"\n", false),
            (0, b"", false),
            (0, b"etc/gitattributes\n", false),
            (0, b"/etc/a\0b\n", false),
            (129, b"usage: git var\n", false),
            (128, b"", true),
        ] {
            assert_eq!(
                system_attributes(status, out, nosystem),
                None,
                "{status} {out:?}"
            );
        }
    }

    /// Query F's `-v` records (ADR-133): one ASCII letter and a space before the mode, kept per entry; anything else
    /// is a bad record. The untagged parser keeps no tags.
    #[test]
    fn query_f_tagged_records() {
        let sha = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
        let out = format!(
            "H 100644 {sha} 0\ta\0h 100644 {sha} 0\t.gitattributes\0S 100755 {sha} 0\tb\0\
             M 100644 {sha} 1\tc\0M 100644 {sha} 2\tc\0"
        );
        let (e, unmerged) = parse_ls_files_tagged_stages(out.into_bytes()).unwrap();
        assert!(unmerged);
        assert_eq!(&*e.tags, b"HhSMM");
        assert_eq!(e.paths.len(), 5);
        assert_eq!(e.modes.first(), Some(&0o100_644));
        for bad in [
            format!("100644 {sha} 0\ta\0"),
            format!("HH 100644 {sha} 0\ta\0"),
            format!("? 100644 {sha} 0\ta\0"),
            format!("H  100644 {sha} 0\ta\0"),
        ] {
            assert!(
                parse_ls_files_tagged(bad.clone().into_bytes()).is_err(),
                "{bad:?}"
            );
        }
        let plain = format!("100644 {sha} 0\ta\0");
        assert!(
            parse_ls_files_stage(plain.into_bytes())
                .unwrap()
                .tags
                .is_empty()
        );
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
