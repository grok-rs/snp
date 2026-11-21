//! The file-content cache of a window of consecutive in-process hooks (ADR-131, variant "B-racy").
//!
//! pre-commit runs every pre-commit-hooks hook as a fresh process that opens each file again. snp runs the ports in
//! process, so a window of consecutive in-process hooks (ADR-102's prefetch window) re-reads the same files once per
//! hook. This cache serves those re-reads from memory, but only when one `stat` of the path proves the bytes are the
//! ones a fresh `open` + `read` would return:
//!
//! - **Fill** (only `read_all` and pygrep): `t0` is taken before the `open`, the `statx(fd)`
//!   that `std::fs::read` makes anyway gives the stamp, and the bytes are kept only when the file is a regular file
//!   on the work tree's device, its mtime **and** ctime are older than `t0 − 2 s` (git's racy window: a file whose
//!   last change is that old cannot change again within the same timestamp tick unnoticed), the bytes read equal the
//!   stat's size (no torn fill), and the size fits the caps ([`PER_FILE_CAP`], [`TOTAL_CAP`], allocated with
//!   `try_reserve`). Errors are never cached.
//! - **Hit**: one `statx(path)` that follows symlinks, as `open` does, compared on device, inode, size, mtime, ctime
//!   (seconds and nanoseconds) and file type. Any difference or error is a miss, and the caller takes today's read
//!   path (whose error, if any, is the hook's). A name without an entry costs nothing.
//! - **Writes**: every native write evicts its name **before** writing, also when the write then fails; written
//!   bytes are never cached (a freshly written file is racy). An alias written through another name (a hard link,
//!   a symlink, a case twin) changes the inode's ctime, so its entry misses.
//!
//! Exact under ADR-027's assumptions (kernel-maintained ctime, no backward clock step of 2 s or more, no mmap writer
//! keeping a page dirty for more than 2 s without a timestamp update), the ones git and the `DiffOracle` rely on.
//! Unix only, and only on a `stat_oracle_safe` work tree ([`ContentCache::new`] answers `None` elsewhere).
//!
//! `SNP_VERIFY_FILE_CACHE=1` ([`ContentCache::new`]'s `verify`) re-reads the file on every hit and counts a
//! difference ([`ContentCache::mismatches`]); the fresh read is then used.
//!
//! Owner: WP-26.

use std::collections::HashMap;
use std::collections::hash_map::Entry::{Occupied, Vacant};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The largest file kept (1 MiB).
pub const PER_FILE_CAP: usize = 1 << 20;
/// The most bytes kept at once (16 MiB).
pub const TOTAL_CAP: usize = 16 << 20;
/// git's racy window: a file changed less than this before the fill started is not kept.
pub const RACY_WINDOW: Duration = Duration::from_secs(2);

/// Hits of every cache of this process (a counting probe for tests and measurements).
static PROCESS_HITS: AtomicU64 = AtomicU64::new(0);

/// The validated hits of every [`ContentCache`] of this process so far.
pub fn process_hits() -> u64 {
    PROCESS_HITS.load(Ordering::Relaxed)
}

/// The pre-commit-hooks ids whose native reads file contents through the cache-aware helpers (`read_all`,
/// `read_head`, end-of-file-fixer's tail). A window gets a cache only when it holds two or more readers (pygrep
/// hooks count too); a missing id only costs the gain, never correctness.
const READERS: [&str; 20] = [
    "check-byte-order-marker",
    "check-executables-have-shebangs",
    "check-json",
    "check-merge-conflict",
    "check-shebang-scripts-are-executable",
    "check-toml",
    "check-vcs-permalinks",
    "check-xml",
    "check-yaml",
    "detect-aws-credentials",
    "detect-private-key",
    "end-of-file-fixer",
    "file-contents-sorter",
    "fix-byte-order-marker",
    "fix-encoding-pragma",
    "mixed-line-ending",
    "pretty-format-json",
    "requirements-txt-fixer",
    "sort-simple-yaml",
    "trailing-whitespace",
];

/// Whether the native hook `id` reads file contents through the cache (ADR-131's enabling rule).
pub fn reads_contents(id: &str) -> bool {
    READERS.contains(&id)
}

/// What a fill and a hit compare: the identity, size, timestamps and type of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
    /// `st_mode & S_IFMT`.
    kind: u32,
}

impl Stamp {
    #[cfg(unix)]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "`None` off Unix (the signature is shared)"
    )]
    fn of(m: &std::fs::Metadata) -> Option<Self> {
        use std::os::unix::fs::MetadataExt as _;
        Some(Self {
            dev: m.dev(),
            ino: m.ino(),
            size: m.size(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
            kind: m.mode() & 0o170_000,
        })
    }

    /// Windows has no ctime or file id in stable std: nothing is ever stamped (and no cache is ever made).
    #[cfg(not(unix))]
    fn of(_m: &std::fs::Metadata) -> Option<Self> {
        None
    }

    /// `(seconds, nanoseconds)` as nanoseconds since the epoch.
    fn ns((s, ns): (i64, i64)) -> i128 {
        i128::from(s)
            .saturating_mul(1_000_000_000)
            .saturating_add(i128::from(ns))
    }
}

/// The regular-file `S_IFMT` value.
const S_IFREG: u32 = 0o100_000;

/// File contents as a native reads them: its own buffer, or one shared with the content cache (a hit, or a fill that
/// was kept). Derefs to the bytes.
#[derive(Debug)]
pub(crate) enum Bytes {
    /// Read without the cache, or not kept.
    Owned(Vec<u8>),
    /// Shared with the cache.
    Shared(Arc<Vec<u8>>),
}

impl std::ops::Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Self::Owned(v) => v,
            Self::Shared(a) => a,
        }
    }
}

/// One cached file.
#[derive(Debug, Clone)]
struct Entry {
    bytes: Arc<Vec<u8>>,
    stamp: Stamp,
}

/// The map and its byte total. Keyed by the name as the hook opens it relative to the cache's root (short, and
/// hashed as bytes: `Path`'s own `Hash` walks components byte by byte).
#[derive(Debug, Default)]
struct Inner {
    map: HashMap<OsString, Entry>,
    total: usize,
}

impl Inner {
    fn remove(&mut self, name: &OsStr) {
        if let Some(e) = self.map.remove(name) {
            self.total = self.total.saturating_sub(e.bytes.len());
        }
    }
}

/// Counters of one cache (tests and measurements).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Reads served from memory after a successful validation.
    pub hits: u64,
    /// Lookups of a cached name whose validation failed (the caller read the file).
    pub stale: u64,
    /// Files kept by a fill.
    pub fills: u64,
    /// Fills that read a file but did not keep it (racy, not regular, other device, torn, over a cap).
    pub refused: u64,
}

/// The cache of one window (ADR-131). Shared by the window's hooks and their partitions (`Sync`).
#[derive(Debug)]
pub struct ContentCache {
    /// The directory the names are relative to (the work-tree root).
    root: PathBuf,
    root_dev: u64,
    verify: bool,
    inner: Mutex<Inner>,
    hits: AtomicU64,
    stale: AtomicU64,
    fills: AtomicU64,
    refused: AtomicU64,
    mismatches: AtomicU64,
}

impl ContentCache {
    /// A cache for the work tree at `root`: `None` off Unix, when `root` is not on a `stat_oracle_safe` filesystem
    /// (ADR-078's allow-list), or when it cannot be stat'ed. `verify` (`SNP_VERIFY_FILE_CACHE=1`) re-reads every hit.
    pub fn new(root: &Path, verify: bool) -> Option<Self> {
        if !cfg!(unix) || !snp_os::fsinfo::stat_oracle_safe(root).unwrap_or(false) {
            return None;
        }
        let stamp = Stamp::of(&std::fs::metadata(root).ok()?)?;
        Some(Self {
            root: root.to_path_buf(),
            root_dev: stamp.dev,
            verify,
            inner: Mutex::new(Inner::default()),
            hits: AtomicU64::new(0),
            stale: AtomicU64::new(0),
            fills: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            mismatches: AtomicU64::new(0),
        })
    }

    /// The counters so far.
    pub fn stats(&self) -> Stats {
        Stats {
            hits: self.hits.load(Ordering::Relaxed),
            stale: self.stale.load(Ordering::Relaxed),
            fills: self.fills.load(Ordering::Relaxed),
            refused: self.refused.load(Ordering::Relaxed),
        }
    }

    /// Hits whose bytes differed from a fresh read under `verify` (always 0 unless the cache is unsound).
    pub fn mismatches(&self) -> u64 {
        self.mismatches.load(Ordering::Relaxed)
    }

    /// The cache for a hook whose cwd is `cwd`: itself when that is the cache's root (names are keys relative to
    /// it), else `None` (a hook with the `root` extension key reads without the cache).
    pub(crate) fn view(&self, cwd: &Path) -> Option<&Self> {
        (cwd.as_os_str() == self.root.as_os_str()).then_some(self)
    }

    /// The bytes cached for `name` (opened as `path`), when the `stat` of `path` still equals the fill's (one
    /// `statx(path)`; none for a name without an entry). A stale entry is dropped.
    pub(crate) fn hit(&self, name: &OsStr, path: &Path) -> Option<Arc<Vec<u8>>> {
        let entry = snp_core::sync::lock(&self.inner).map.get(name).cloned()?;
        let now = std::fs::metadata(path).ok().as_ref().and_then(Stamp::of);
        if now != Some(entry.stamp) {
            self.stale.fetch_add(1, Ordering::Relaxed);
            let mut g = snp_core::sync::lock(&self.inner);
            if g.map
                .get(name)
                .is_some_and(|e| Arc::ptr_eq(&e.bytes, &entry.bytes))
            {
                g.remove(name);
            }
            return None;
        }
        if self.verify {
            let fresh = std::fs::read(path).ok();
            if fresh.as_deref() != Some(entry.bytes.as_slice()) {
                self.mismatches.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        }
        self.hits.fetch_add(1, Ordering::Relaxed);
        PROCESS_HITS.fetch_add(1, Ordering::Relaxed);
        Some(entry.bytes)
    }

    /// `open(name, 'rb').read()` (`name` opened as `path`): a validated hit, else a fill. The fill makes the
    /// syscalls `std::fs::read` makes (`open`, `statx(fd)`, reads) and keeps the bytes only under the rules of the
    /// module documentation.
    pub(crate) fn read(&self, name: &OsStr, path: &Path) -> io::Result<Bytes> {
        if let Some(b) = self.hit(name, path) {
            return Ok(Bytes::Shared(b));
        }
        let t0 = SystemTime::now();
        let f = File::open(path)?;
        // `std::fs::read` ignores a failing `fstat` too (no size hint, nothing kept)
        let meta = f.metadata().ok();
        let mut buf = Vec::new();
        if let Some(size) = meta.as_ref().and_then(|m| usize::try_from(m.len()).ok()) {
            buf.try_reserve_exact(size).map_err(|_| {
                io::Error::new(io::ErrorKind::OutOfMemory, "failed to allocate read buffer")
            })?;
        }
        // `Take` reads with the plain loop: `File::read_to_end` would `fstat` and `lseek` again for its size hint
        (&f).take(u64::MAX).read_to_end(&mut buf)?;
        drop(f);
        match meta.as_ref().and_then(Stamp::of) {
            Some(stamp) if meta.as_ref().is_some_and(std::fs::Metadata::is_file) => {
                Ok(self.offer(name, stamp, t0, buf))
            }
            _ => {
                self.refused.fetch_add(1, Ordering::Relaxed);
                Ok(Bytes::Owned(buf))
            }
        }
    }

    /// Keeps `bytes` read for `name` (stamped `stamp` before the read, which started at `t0`) when [`admit`] allows
    /// it, shared with the caller (no copy).
    fn offer(&self, name: &OsStr, stamp: Stamp, t0: SystemTime, bytes: Vec<u8>) -> Bytes {
        let len = bytes.len();
        // the file's own conditions first, outside the lock; the total is checked under it
        if !admit(stamp, self.root_dev, t0, len, 0) {
            self.refused.fetch_add(1, Ordering::Relaxed);
            return Bytes::Owned(bytes);
        }
        let shared = Arc::new(bytes);
        let entry = Entry {
            bytes: Arc::clone(&shared),
            stamp,
        };
        let mut g = snp_core::sync::lock(&self.inner);
        let Inner { map, total } = &mut *g;
        let kept = match map.entry(name.to_os_string()) {
            Occupied(mut o) => {
                let others = total.saturating_sub(o.get().bytes.len());
                if admit(stamp, self.root_dev, t0, len, others) {
                    *total = others.saturating_add(len);
                    o.insert(entry);
                    true
                } else {
                    *total = others;
                    o.remove();
                    false
                }
            }
            Vacant(v) => {
                let fits = admit(stamp, self.root_dev, t0, len, *total);
                if fits {
                    *total = total.saturating_add(len);
                    v.insert(entry);
                }
                fits
            }
        };
        drop(g);
        let counter = if kept { &self.fills } else { &self.refused };
        counter.fetch_add(1, Ordering::Relaxed);
        Bytes::Shared(shared)
    }

    /// Drops the entry of `name`: every native write calls this **before** writing (also when the write fails).
    pub(crate) fn evict(&self, name: &OsStr) {
        snp_core::sync::lock(&self.inner).remove(name);
    }
}

/// The fill rule (pure): a regular file on the root's device, both timestamps older than `t0 − 2 s`, the bytes read
/// equal to the stat's size, and within the caps given `others` bytes already kept.
fn admit(stamp: Stamp, root_dev: u64, t0: SystemTime, len: usize, others: usize) -> bool {
    let Ok(t0) = t0.duration_since(UNIX_EPOCH) else {
        return false;
    };
    let limit = i128::try_from(t0.saturating_sub(RACY_WINDOW).as_nanos()).unwrap_or(0);
    stamp.kind == S_IFREG
        && stamp.dev == root_dev
        && u64::try_from(len).is_ok_and(|l| l == stamp.size)
        && Stamp::ns(stamp.mtime) < limit
        && Stamp::ns(stamp.ctime) < limit
        && len <= PER_FILE_CAP
        && others.saturating_add(len) <= TOTAL_CAP
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(age: Duration, now: SystemTime) -> Stamp {
        let t = now
            .checked_sub(age)
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap();
        let ts = (
            i64::try_from(t.as_secs()).unwrap(),
            i64::from(t.subsec_nanos()),
        );
        Stamp {
            dev: 7,
            ino: 1,
            size: 10,
            mtime: ts,
            ctime: ts,
            kind: S_IFREG,
        }
    }

    #[test]
    fn admit_rule() {
        let now = SystemTime::now();
        let old = stamp(Duration::from_secs(3), now);
        assert!(admit(old, 7, now, 10, 0));
        // racy: mtime or ctime within 2 s of t0
        let fresh = stamp(Duration::from_millis(1900), now);
        assert!(!admit(fresh, 7, now, 10, 0));
        assert!(!admit(
            Stamp {
                mtime: fresh.mtime,
                ..old
            },
            7,
            now,
            10,
            0
        ));
        assert!(!admit(
            Stamp {
                ctime: fresh.ctime,
                ..old
            },
            7,
            now,
            10,
            0
        ));
        // exactly t0 - 2 s is still racy
        let edge = stamp(RACY_WINDOW, now);
        assert!(!admit(edge, 7, now, 10, 0));
        // another device, not a regular file, torn read
        assert!(!admit(old, 8, now, 10, 0));
        assert!(!admit(
            Stamp {
                kind: 0o020_000,
                ..old
            },
            7,
            now,
            10,
            0
        ));
        assert!(!admit(old, 7, now, 9, 0));
        // caps
        let big = Stamp {
            size: u64::try_from(PER_FILE_CAP).unwrap(),
            ..old
        };
        assert!(admit(big, 7, now, PER_FILE_CAP, 0));
        let over = Stamp {
            size: u64::try_from(PER_FILE_CAP + 1).unwrap(),
            ..old
        };
        assert!(!admit(over, 7, now, PER_FILE_CAP + 1, 0));
        assert!(admit(big, 7, now, PER_FILE_CAP, TOTAL_CAP - PER_FILE_CAP));
        assert!(!admit(
            big,
            7,
            now,
            PER_FILE_CAP,
            TOTAL_CAP - PER_FILE_CAP + 1
        ));
        // a clock before the epoch keeps nothing
        assert!(!admit(old, 7, UNIX_EPOCH - Duration::from_secs(1), 10, 0));
    }

    /// A hit whose stamp holds but whose bytes differ (the mmap blind spot, forged here) is counted under `verify`
    /// and not served; without `verify` it would be.
    #[cfg(unix)]
    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "a test fixture writes files; the write ban (ADR-132) is for native hook code"
    )]
    fn verify_counts_a_forged_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let Some(cache) = ContentCache::new(dir.path(), true) else {
            return;
        };
        let path = dir.path().join("f");
        std::fs::write(&path, b"real\n").unwrap();
        let stamp = Stamp::of(&std::fs::metadata(&path).unwrap()).unwrap();
        let forged = |c: &ContentCache| {
            let mut g = snp_core::sync::lock(&c.inner);
            g.map.insert(
                OsString::from("f"),
                Entry {
                    bytes: Arc::new(b"fake\n".to_vec()),
                    stamp,
                },
            );
        };
        let f = OsStr::new("f");
        forged(&cache);
        assert!(cache.hit(f, &path).is_none());
        assert_eq!(cache.mismatches(), 1);
        assert_eq!(&*cache.read(f, &path).unwrap(), b"real\n");
        let plain = ContentCache::new(dir.path(), false).unwrap();
        forged(&plain);
        assert_eq!(plain.hit(f, &path).unwrap().as_slice(), b"fake\n");
        assert_eq!(plain.mismatches(), 0);
    }

    /// A stale entry is dropped and its bytes leave the total.
    #[cfg(unix)]
    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "a test fixture writes files; the write ban (ADR-132) is for native hook code"
    )]
    fn stale_entries_leave_the_total() {
        let dir = tempfile::tempdir().unwrap();
        let Some(cache) = ContentCache::new(dir.path(), false) else {
            return;
        };
        let path = dir.path().join("f");
        std::fs::write(&path, b"abc").unwrap();
        let stamp = Stamp::of(&std::fs::metadata(&path).unwrap()).unwrap();
        {
            let mut g = snp_core::sync::lock(&cache.inner);
            g.map.insert(
                OsString::from("f"),
                Entry {
                    bytes: Arc::new(b"abc".to_vec()),
                    stamp: Stamp {
                        ino: stamp.ino ^ 1,
                        ..stamp
                    },
                },
            );
            g.total = 3;
        }
        assert!(cache.hit(OsStr::new("f"), &path).is_none());
        assert_eq!(cache.stats().stale, 1);
        let g = snp_core::sync::lock(&cache.inner);
        assert!(g.map.is_empty());
        assert_eq!(g.total, 0);
    }

    #[test]
    fn readers_are_natives() {
        for id in READERS {
            assert!(
                crate::native(crate::NativeSource::PreCommitHooks, id).is_some(),
                "{id}"
            );
        }
        assert!(!reads_contents("check-added-large-files"));
        assert!(!reads_contents("check-case-conflict"));
    }
}
