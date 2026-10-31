//! The stash journal (`<store>/patches/run-<pid>-<rand>.snp-journal`) and the git-dir pointer
//! (`<git_dir>/snp-stash.journal`): format, parsing and crash-safe writes (ARCHITECTURE §4.1–§4.2, §4.6, ADR-067).
//!
//! Both files are JSON objects. Byte strings (paths, intent-to-add and staged paths) are JSON strings when they are
//! UTF-8 and `{"hex": "<lower-case hex>"}` otherwise, so non-UTF-8 paths round-trip exactly. Parsing is total: any
//! input (truncated, hostile, deeply nested) yields `None` or a value, never a panic (fuzz target
//! `stash_journal_parse`).
//!
//! Owner: WP-23.

use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Journal format version.
pub(crate) const VERSION: u64 = 1;

/// The journal file suffix in the patches directory.
pub(crate) const JOURNAL_SUFFIX: &str = ".snp-journal";

/// The git-dir pointer: `<git_dir>/snp-stash.journal`; further pointers (while an orphaned run's pointer is kept) are
/// `snp-stash-<n>.journal`.
pub(crate) const POINTER_NAME: &str = "snp-stash.journal";

/// The largest journal or pointer file read (bigger files are not snp's).
pub(crate) const READ_CAP: u64 = 64 * 1024 * 1024;

/// The journaled states (ARCHITECTURE §4.1).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Journal created; the intent-to-add list is recorded and `git rm --cached` may have run.
    Started,
    /// The patch is durable and the worktree checkout may have run.
    Stashed,
    /// `stage_fixed` is about to stage (or has staged) hook fixes.
    StagingFixed,
    /// The first two applies failed; the rollback checkout may have run.
    RollingBack,
    /// The restore failed (K+): the patch holds the only copy.
    Failed,
    /// The patch was re-applied.
    Restored,
}

impl State {
    /// The journal spelling.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Stashed => "stashed",
            Self::StagingFixed => "staging-fixed",
            Self::RollingBack => "rolling-back",
            Self::Failed => "failed",
            Self::Restored => "restored",
        }
    }

    /// Whether the state protects user data (fsynced, ADR-067).
    pub(crate) const fn durable(self) -> bool {
        matches!(self, Self::Stashed | Self::StagingFixed)
    }
}

/// Whether a journaled state means the run never restored (ARCHITECTURE §4.6).
pub(crate) fn is_unrestored(state: &str) -> bool {
    matches!(
        state,
        "stashed" | "staging-fixed" | "rolling-back" | "failed"
    )
}

/// The journal contents.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct JournalData {
    /// The last journaled state.
    pub(crate) state: String,
    /// The worktree-lock token of the run (empty for a nested run).
    pub(crate) token: String,
    /// The worktree root.
    pub(crate) worktree: PathBuf,
    /// `--git-dir`.
    pub(crate) git_dir: PathBuf,
    /// `--git-common-dir`.
    pub(crate) common_dir: PathBuf,
    /// `--git-path index`.
    pub(crate) index_file: PathBuf,
    /// The contents of `<git_dir>/HEAD` at stash time (informational).
    pub(crate) head: String,
    /// The pid of the run (informational: liveness is the worktree lock).
    pub(crate) pid: u32,
    /// Unix seconds when the run started.
    pub(crate) start_time: u64,
    /// The snp version that wrote the journal.
    pub(crate) snp_version: String,
    /// The intent-to-add paths cleared from the index (git order).
    pub(crate) ita: Vec<Vec<u8>>,
    /// The `git write-tree` of the index at stash time.
    pub(crate) tree: Option<String>,
    /// The saved patch.
    pub(crate) patch: Option<PathBuf>,
    /// The paths `stage_fixed` stages.
    pub(crate) staged_fixed: Vec<Vec<u8>>,
}

/// The git-dir pointer contents.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct PointerData {
    /// The store journal (absent when the store was not writable).
    pub(crate) journal: Option<PathBuf>,
    /// The saved patch.
    pub(crate) patch: Option<PathBuf>,
    /// The index tree.
    pub(crate) tree: Option<String>,
    /// The last journaled state.
    pub(crate) state: String,
    /// The worktree root.
    pub(crate) worktree: Option<PathBuf>,
    /// The paths `stage_fixed` stages.
    pub(crate) staged_fixed: Vec<Vec<u8>>,
    /// The intent-to-add paths cleared from the index (git order; absent in older pointers).
    pub(crate) ita: Vec<Vec<u8>>,
}

// ---- byte-string codec ----

/// Path → bytes (Unix: exact; elsewhere: the UTF-8 view, lossless for every path std can name as UTF-8).
pub(crate) fn path_bytes(p: &Path) -> Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Cow::Borrowed(p.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        match p.to_string_lossy() {
            Cow::Borrowed(s) => Cow::Borrowed(s.as_bytes()),
            Cow::Owned(s) => Cow::Owned(s.into_bytes()),
        }
    }
}

/// Bytes → path (the inverse of [`path_bytes`]).
pub(crate) fn bytes_path(b: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(b))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(b).into_owned())
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn enc_bytes(b: &[u8]) -> Value {
    if let Ok(s) = std::str::from_utf8(b) {
        return Value::String(s.to_owned());
    }
    let mut hex = String::new();
    for &byte in b {
        hex.push(char::from(
            *HEX.get(usize::from(byte >> 4)).unwrap_or(&b'0'),
        ));
        hex.push(char::from(
            *HEX.get(usize::from(byte & 0xf)).unwrap_or(&b'0'),
        ));
    }
    let mut m = Map::new();
    m.insert("hex".to_owned(), Value::String(hex));
    Value::Object(m)
}

fn unhex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => c.checked_sub(b'0'),
        b'a'..=b'f' => c.checked_sub(b'a').and_then(|v| v.checked_add(10)),
        b'A'..=b'F' => c.checked_sub(b'A').and_then(|v| v.checked_add(10)),
        _ => None,
    }
}

fn dec_bytes(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::String(s) => Some(s.as_bytes().to_vec()),
        Value::Object(m) => {
            let hex = m.get("hex")?.as_str()?.as_bytes();
            if hex.len() % 2 != 0 {
                return None;
            }
            let mut out = Vec::new();
            out.try_reserve(hex.len() / 2).ok()?;
            let mut it = hex.iter();
            while let (Some(&hi), Some(&lo)) = (it.next(), it.next()) {
                let h = unhex_digit(hi)?;
                let l = unhex_digit(lo)?;
                out.push(h.checked_mul(16)?.checked_add(l)?);
            }
            Some(out)
        }
        _ => None,
    }
}

fn enc_path(p: &Path) -> Value {
    enc_bytes(&path_bytes(p))
}

fn enc_opt_path(p: Option<&Path>) -> Value {
    p.map_or(Value::Null, enc_path)
}

fn enc_list(items: &[Vec<u8>]) -> Value {
    Value::Array(items.iter().map(|b| enc_bytes(b)).collect())
}

fn get_path(m: &Map<String, Value>, k: &str) -> Option<PathBuf> {
    dec_bytes(m.get(k)?).map(|b| bytes_path(&b))
}

fn get_str(m: &Map<String, Value>, k: &str) -> Option<String> {
    m.get(k)?.as_str().map(str::to_owned)
}

fn get_list(m: &Map<String, Value>, k: &str) -> Option<Vec<Vec<u8>>> {
    match m.get(k) {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(dec_bytes).collect(),
        Some(_) => None,
    }
}

/// `Ok(None)` when absent or `null`, `Err(Malformed)` when present but not a byte string.
fn get_opt_path(m: &Map<String, Value>, k: &str) -> Result<Option<PathBuf>, Malformed> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => dec_bytes(v).map(|b| Some(bytes_path(&b))).ok_or(Malformed),
    }
}

/// A present field of the wrong type.
#[derive(Debug)]
struct Malformed;

/// `Ok(None)` when absent or `null`, `Err(Malformed)` when present but not a string.
fn get_opt_str(m: &Map<String, Value>, k: &str) -> Result<Option<String>, Malformed> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(Malformed),
    }
}

impl JournalData {
    /// The JSON bytes (one line, newline-terminated).
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut m = Map::new();
        m.insert("v".to_owned(), Value::from(VERSION));
        m.insert("state".to_owned(), Value::String(self.state.clone()));
        m.insert("token".to_owned(), Value::String(self.token.clone()));
        m.insert("worktree".to_owned(), enc_path(&self.worktree));
        m.insert("git_dir".to_owned(), enc_path(&self.git_dir));
        m.insert("common_dir".to_owned(), enc_path(&self.common_dir));
        m.insert("index_file".to_owned(), enc_path(&self.index_file));
        m.insert("head".to_owned(), Value::String(self.head.clone()));
        m.insert("pid".to_owned(), Value::from(self.pid));
        m.insert("start_time".to_owned(), Value::from(self.start_time));
        m.insert(
            "snp_version".to_owned(),
            Value::String(self.snp_version.clone()),
        );
        m.insert("ita".to_owned(), enc_list(&self.ita));
        m.insert(
            "tree".to_owned(),
            self.tree.clone().map_or(Value::Null, Value::String),
        );
        m.insert("patch".to_owned(), enc_opt_path(self.patch.as_deref()));
        m.insert("staged_fixed".to_owned(), enc_list(&self.staged_fixed));
        let mut out = Value::Object(m).to_string().into_bytes();
        out.push(b'\n');
        out
    }

    /// Parses a journal; `None` for anything that is not a version-1 journal object.
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        let v: Value = serde_json::from_slice(bytes).ok()?;
        let m = v.as_object()?;
        if m.get("v")?.as_u64()? != VERSION {
            return None;
        }
        Some(Self {
            state: get_str(m, "state")?,
            token: get_str(m, "token").unwrap_or_default(),
            worktree: get_path(m, "worktree")?,
            git_dir: get_path(m, "git_dir").unwrap_or_default(),
            common_dir: get_path(m, "common_dir").unwrap_or_default(),
            index_file: get_path(m, "index_file").unwrap_or_default(),
            head: get_str(m, "head").unwrap_or_default(),
            pid: m
                .get("pid")
                .and_then(Value::as_u64)
                .and_then(|p| u32::try_from(p).ok())
                .unwrap_or(0),
            start_time: m.get("start_time").and_then(Value::as_u64).unwrap_or(0),
            snp_version: get_str(m, "snp_version").unwrap_or_default(),
            ita: get_list(m, "ita")?,
            tree: get_opt_str(m, "tree").ok()?,
            patch: get_opt_path(m, "patch").ok()?,
            staged_fixed: get_list(m, "staged_fixed")?,
        })
    }
}

impl PointerData {
    /// The JSON bytes (one line, newline-terminated).
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut m = Map::new();
        m.insert("v".to_owned(), Value::from(VERSION));
        m.insert("journal".to_owned(), enc_opt_path(self.journal.as_deref()));
        m.insert("patch".to_owned(), enc_opt_path(self.patch.as_deref()));
        m.insert(
            "tree".to_owned(),
            self.tree.clone().map_or(Value::Null, Value::String),
        );
        m.insert("state".to_owned(), Value::String(self.state.clone()));
        m.insert(
            "worktree".to_owned(),
            enc_opt_path(self.worktree.as_deref()),
        );
        m.insert("staged_fixed".to_owned(), enc_list(&self.staged_fixed));
        m.insert("ita".to_owned(), enc_list(&self.ita));
        let mut out = Value::Object(m).to_string().into_bytes();
        out.push(b'\n');
        out
    }

    /// Parses a pointer; `None` for anything that is not a version-1 pointer object.
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        let v: Value = serde_json::from_slice(bytes).ok()?;
        let m = v.as_object()?;
        if m.get("v")?.as_u64()? != VERSION {
            return None;
        }
        Some(Self {
            journal: get_opt_path(m, "journal").ok()?,
            patch: get_opt_path(m, "patch").ok()?,
            tree: get_opt_str(m, "tree").ok()?,
            state: get_str(m, "state")?,
            worktree: get_opt_path(m, "worktree").ok()?,
            staged_fixed: get_list(m, "staged_fixed")?,
            ita: get_list(m, "ita")?,
        })
    }
}

// ---- crash-safe writes ----

/// `create_new` (`O_EXCL`, 0600) + write; `durable` adds `sync_all`.
pub(crate) fn write_new(path: &Path, bytes: &[u8], durable: bool) -> io::Result<()> {
    let mut f = new_file(path)?;
    let res = f
        .write_all(bytes)
        .and_then(|()| if durable { f.sync_all() } else { Ok(()) });
    if res.is_err() {
        drop(f);
        let _ignored = std::fs::remove_file(path);
    }
    res
}

/// A new file (`create_new`, mode 0600 on Unix).
pub(crate) fn new_file(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// temp file in the same directory + rename over `path` (ADR-026 "written temp+rename"); `durable` adds the file
/// `sync_all` before the rename and a directory fsync after it (the rename itself must survive a power loss).
pub(crate) fn replace(path: &Path, bytes: &[u8], durable: bool) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut last = io::Error::from(io::ErrorKind::AlreadyExists);
    for _ in 0..16 {
        let tmp = dir.join(format!(".{name}.tmp-{}", crate::lock::random_hex(8)));
        match write_new(&tmp, bytes, durable) {
            Ok(()) => {
                snp_os::fs::rename_with_retry(&tmp, path).inspect_err(|_| {
                    let _ignored = std::fs::remove_file(&tmp);
                })?;
                return if durable { sync_dir(dir) } else { Ok(()) };
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

/// fsync a directory (Unix; a no-op elsewhere, where directory entries are durable with the file).
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _: &Path = dir;
        Ok(())
    }
}

/// Reads a journal-sized file (`None` when missing or unreadable).
pub(crate) fn read_small(path: &Path) -> Option<Vec<u8>> {
    snp_os::fs::read_capped(path, READ_CAP).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> JournalData {
        JournalData {
            state: "stashed".to_owned(),
            token: "abc".to_owned(),
            worktree: PathBuf::from("/r"),
            git_dir: PathBuf::from("/r/.git"),
            common_dir: PathBuf::from("/r/.git"),
            index_file: PathBuf::from("/r/.git/index"),
            head: "ref: refs/heads/master".to_owned(),
            pid: 42,
            start_time: 1_700_000_000,
            snp_version: "2.0.0".to_owned(),
            ita: vec![b"a".to_vec(), b"\xff\x00b".to_vec()],
            tree: Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_owned()),
            patch: Some(PathBuf::from("/s/patches/patch1-2")),
            staged_fixed: vec![b"x y".to_vec()],
        }
    }

    #[test]
    fn journal_round_trips() {
        let j = sample();
        let bytes = j.to_bytes();
        assert!(bytes.ends_with(b"\n"));
        assert_eq!(JournalData::parse(&bytes), Some(j));
    }

    #[test]
    fn pointer_round_trips() {
        let p = PointerData {
            journal: Some(PathBuf::from("/s/patches/run-1-ab.snp-journal")),
            patch: None,
            tree: None,
            state: "started".to_owned(),
            worktree: Some(PathBuf::from("/r")),
            staged_fixed: Vec::new(),
            ita: vec![b"n".to_vec(), b"\xff".to_vec()],
        };
        assert_eq!(PointerData::parse(&p.to_bytes()), Some(p));
    }

    #[test]
    fn hostile_inputs_are_rejected() {
        for bad in [
            &b""[..],
            b"{",
            b"[]",
            b"{\"v\":2,\"state\":\"x\",\"worktree\":\"/r\"}",
            b"{\"v\":1}",
            b"{\"v\":1,\"state\":\"s\",\"worktree\":{\"hex\":\"f\"}}",
            b"{\"v\":1,\"state\":\"s\",\"worktree\":{\"hex\":\"zz\"}}",
            b"{\"v\":1,\"state\":\"s\",\"worktree\":\"/r\",\"ita\":5}",
            b"{\"v\":1,\"state\":\"s\",\"worktree\":\"/r\",\"tree\":5}",
        ] {
            assert_eq!(JournalData::parse(bad), None, "{bad:?}");
        }
        let deep: String = std::iter::repeat_n('[', 100_000).collect();
        assert_eq!(JournalData::parse(deep.as_bytes()), None);
        let ok = b"{\"v\":1,\"state\":\"s\",\"worktree\":{\"hex\":\"2F72\"}}";
        assert_eq!(
            JournalData::parse(ok).unwrap().worktree,
            PathBuf::from("/r")
        );
    }

    #[test]
    fn states() {
        assert!(State::Stashed.durable() && State::StagingFixed.durable());
        assert!(!State::Started.durable() && !State::Restored.durable());
        for s in [
            State::Stashed,
            State::StagingFixed,
            State::RollingBack,
            State::Failed,
        ] {
            assert!(is_unrestored(s.as_str()));
        }
        for s in [State::Started, State::Restored] {
            assert!(!is_unrestored(s.as_str()));
        }
    }

    #[test]
    fn writes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("j");
        write_new(&p, b"one", true).unwrap();
        assert_eq!(
            write_new(&p, b"two", false).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        replace(&p, b"two", false).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        sync_dir(d.path()).unwrap();
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
        assert_eq!(read_small(&p).unwrap(), b"two");
        assert!(read_small(&d.path().join("missing")).is_none());
    }
}
