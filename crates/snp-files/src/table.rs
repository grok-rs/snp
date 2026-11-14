//! The file table: byte arena + spans, git modes, tier columns allocated on first need (§2.3, ADR-079).
//!
//! pre-commit keeps the run's file names as a list of `str` and memoises `identify.tags_from_path` per name with
//! `functools.cache` (`commands/run.py:73-80`). snp keeps the names as one byte arena (git's `-z` output) and
//! memoises identification per file in struct-of-arrays columns, one per tier, each allocated only when a filter
//! first needs it (ADR-041):
//!
//! * **name** — `tags_from_filename` (no I/O), interned: two bytes per file;
//! * **stat** — one `lstat` (or a trusted git index mode, zero syscalls), stored as file-identify's
//!   `Classification` (kind + mode + name tags, and whether content is needed);
//! * **head** — at most one non-blocking `open` + a 1 KiB read (+ a long shebang line): `text`/`binary` and the
//!   shebang's interpreter tags, or `Vanished`/`Unreadable`;
//! * **view** — the lossy-UTF-8 / `/`-normalised matching view, only for names that are not already clean UTF-8.
//!
//! The arena's UTF-8 is validated once when the table is built (ADR-101): a valid arena is kept as a `String` and
//! the matching view of a name is a slice of it; only names of an invalid arena, or spans that do not fall on
//! character boundaries (`--files` arguments are joined without a separator), are checked one by one. On Linux the
//! `lstat` and `open` of the stat and head tiers are relative to one `O_PATH` descriptor of the root (`statx`,
//! `openat`), as pre-commit's relative names are relative to its working directory; the path-based calls remain
//! the fallback whenever that descriptor or `statx` is unavailable.
//!
//! Owner: WP-22.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};

use file_identify::{Classification, ExecPolicy, FileKind, Meta, Pending, TagSet};
use snp_core::failure::{Failure, PyExc, pyerr};
use snp_core::path::RepoPath;
use snp_os as _;
use snp_proc::pool::JobPool;
use snp_pycompat as _;

/// Index of a file in its [`FileTable`].
///
/// Hooks receive ids instead of pre-commit's `str` names (`run.py:151`); shuffling permutes ids.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct FileId(pub u32);

/// Returned by value (`Copy`): the error payload stays in the table's head column.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum TagOutcome<'a> {
    /// identify's tags for the required tiers.
    Tags(TagSet),
    /// The file disappeared (dropped, X13-CLS-01).
    Vanished,
    /// The file could not be read (exit 3, X13-CLS-02).
    Unreadable(&'a PyExc),
}

/// What a filter needs beyond the name tier (§2.3): stat (type/mode tags), encoding (text/binary), shebang tags.
///
/// The steps of `identify.tags_from_path` (`identify.py:40-83`) a filter can observe (ADR-041).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    /// `lstat` tier.
    pub stat: bool,
    /// `text`/`binary` tier.
    pub encoding: bool,
    /// Shebang tier.
    pub shebang: bool,
}

impl Needs {
    /// Whether anything beyond the name tier is needed (encoding and shebang imply the stat tier).
    pub(crate) const fn any(self) -> bool {
        self.stat || self.encoding || self.shebang
    }
}

/// The stat tier of one file.
#[derive(Clone, Copy, Debug)]
pub(crate) enum StatTier {
    /// `lstat` (or the trusted git mode) succeeded: kind + mode + name tags, and what content is still needed.
    Present(Classification),
    /// `lstat` failed (every `OSError` counts, as in identify: X13-CLS-01/04).
    Vanished,
}

/// The head tier of one file.
#[derive(Debug)]
enum Head {
    /// The complete tags (type, mode, name, interpreter and encoding).
    Tags(TagSet),
    /// The file disappeared before it could be opened.
    Vanished,
    /// `open`/`read` failed: pre-commit's exception (boxed: rare).
    Unreadable(Box<PyExc>),
}

/// A lazily allocated column of lazily computed cells.
struct Column<T>(OnceLock<Box<[OnceLock<T>]>>);

impl<T> Column<T> {
    /// An unallocated column.
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    /// Cell `i` of a column of `n` cells, allocating the column on first use.
    fn cell(&self, n: usize, i: usize) -> Option<&OnceLock<T>> {
        self.0
            .get_or_init(|| (0..n).map(|_| OnceLock::new()).collect())
            .get(i)
    }

    /// The value of cell `i` if the column exists and the cell is set (never allocates).
    fn peek(&self, i: usize) -> Option<&T> {
        self.0.get()?.get(i)?.get()
    }

    /// Keeps the cells whose `keep` flag is set (`retain_lexists`).
    fn retain(&mut self, keep: &[bool]) {
        if let Some(cells) = self.0.get_mut() {
            let old = std::mem::take(cells);
            *cells = old
                .into_vec()
                .into_iter()
                .zip(keep)
                .filter_map(|(c, k)| k.then_some(c))
                .collect();
        }
    }
}

/// Interned name-tier tag sets: the name column holds `u16` ids (0 = not computed).
#[derive(Default)]
struct Interner {
    /// Id − 1 → set.
    sets: Vec<TagSet>,
    /// Set → id − 1.
    index: HashMap<TagSet, u16>,
}

/// The path arena, its UTF-8 validated once (ADR-101).
enum Arena {
    /// Valid UTF-8 (and, on Windows, free of `\`): spans on character boundaries are matching views as they are.
    Text(String),
    /// Anything else: every span is checked on its own.
    Bytes(Vec<u8>),
}

impl Arena {
    /// Validates `buf` once (zero-copy either way).
    fn new(buf: Vec<u8>) -> Self {
        match String::from_utf8(buf) {
            Ok(s) if cfg!(windows) && s.contains('\\') => Self::Bytes(s.into_bytes()),
            Ok(s) => Self::Text(s),
            Err(e) => Self::Bytes(e.into_bytes()),
        }
    }

    /// The raw bytes.
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Text(s) => s.as_bytes(),
            Self::Bytes(b) => b,
        }
    }
}

/// One byte arena + spans; tier columns allocated on first need (§2.3, ADR-079).
///
/// pre-commit's `Classifier.filenames` plus the `functools.cache` of `_types_for_file` (`run.py:72-79`).
pub struct FileTable {
    /// All paths, as git printed them.
    arena: Arena,
    /// Half-open `(start, end)` offsets into `arena`, in git (or argument) order.
    spans: Box<[(u32, u32)]>,
    /// The repository root the relative paths are resolved against for `lstat`/`open`.
    root: PathBuf,
    /// Trusted git index modes (0 = unknown).
    git_modes: Option<Box<[u32]>>,
    /// Name tier: interned tag-set ids.
    name: OnceLock<Box<[AtomicU16]>>,
    /// The name tier's interner.
    interner: Mutex<Interner>,
    /// Stat tier.
    stat: Column<StatTier>,
    /// Head tier.
    head: Column<Head>,
    /// Matching view of names that are not clean UTF-8 (or contain `\` on Windows).
    view: Column<Box<str>>,
    /// `lexists` results of the newest `Classifier::new`, adopted as the stat tier of the first hook's candidates.
    pending: Mutex<Option<Box<[Option<Meta>]>>>,
    /// The root as an `O_PATH` descriptor, opened on first need; `None` when it cannot be opened (ADR-101).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    root_fd: OnceLock<Option<rustix::fd::OwnedFd>>,
}

impl fmt::Debug for FileTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileTable")
            .field("len", &self.spans.len())
            .field("arena_bytes", &self.arena.as_bytes().len())
            .field("root", &self.root)
            .field("git_modes", &self.git_modes.is_some())
            .finish_non_exhaustive()
    }
}

/// Below this many files `prefetch` runs inline (thread start-up costs more than the work).
pub(crate) const PARALLEL_MIN: usize = 256;
/// Files per prefetch partition (at least).
const CHUNK_MIN: usize = 64;

impl FileTable {
    /// From git `-z` output (git order kept); Windows: `\` → `/` in the matching view (02 §10).
    ///
    /// `spans` are the half-open `(start, end)` offsets of `snp_git::ZList::into_parts`; a span outside `buf` (never
    /// produced by git) is dropped. Order and duplicates are kept (`diff.orderFile` parity, 02 G.0 #8).
    pub fn from_zbuf(buf: Vec<u8>, spans: Vec<(u32, u32)>, root: &Path) -> Self {
        let len = buf.len();
        let valid = |&(s, e): &(u32, u32)| s <= e && usize::try_from(e).is_ok_and(|e| e <= len);
        let spans: Box<[(u32, u32)]> = if spans.iter().all(valid) {
            spans.into_boxed_slice()
        } else {
            spans.into_iter().filter(valid).collect()
        };
        Self::build(buf, spans, root)
    }

    /// `--files` / commit-msg / try-repo / stdin.
    ///
    /// The paths in argument order, duplicates kept (`run.py:_all_filenames` with `args.files`). Unix keeps the
    /// raw bytes; Windows the WTF-8 encoding (its matching view is lossy UTF-8). Paths beyond a 4 GiB arena (never
    /// reachable through `ARG_MAX` or a sane stdin) are not added.
    pub fn from_args(paths: Vec<OsString>, root: &Path) -> Self {
        let mut arena: Vec<u8> = Vec::new();
        let mut spans: Vec<(u32, u32)> = Vec::new();
        for p in paths {
            let bytes = os_bytes(&p);
            let Some(start) = u32::try_from(arena.len()).ok() else {
                break;
            };
            let Some(end) = arena
                .len()
                .checked_add(bytes.len())
                .and_then(|e| u32::try_from(e).ok())
            else {
                break;
            };
            arena.extend_from_slice(bytes);
            spans.push((start, end));
        }
        Self::build(arena, spans.into_boxed_slice(), root)
    }

    /// Common constructor.
    fn build(arena: Vec<u8>, spans: Box<[(u32, u32)]>, root: &Path) -> Self {
        Self {
            arena: Arena::new(arena),
            spans,
            root: root.to_path_buf(),
            git_modes: None,
            name: OnceLock::new(),
            interner: Mutex::new(Interner::default()),
            stat: Column::new(),
            head: Column::new(),
            view: Column::new(),
            pending: Mutex::new(None),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            root_fd: OnceLock::new(),
        }
    }

    /// git index modes (Unix, `core.fileMode=true`, `core.symlinks=true`, after the stash: worktree == index)
    /// (§2.3); matched by path against the table; the caller iterates `snp_git::IndexEntries` without copying paths
    /// (snp-files does not depend on snp-git).
    ///
    /// A file with a trusted mode needs no `lstat`: it counts as existing and its kind/executable bit come from
    /// `Meta::from_git_mode_with(mode, PythonCompat)` (identify's result whenever the worktree equals the index).
    /// Modes that name no file kind are ignored; on Windows symlink modes are ignored (`core.symlinks=false` checks
    /// them out as regular files).
    pub fn set_trusted_modes<'a>(&mut self, entries: impl IntoIterator<Item = (&'a [u8], u32)>) {
        let n = self.len();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| self.bytes_at(a).cmp(self.bytes_at(b)));
        let mut modes: Box<[u32]> = self
            .git_modes
            .take()
            .unwrap_or_else(|| vec![0_u32; n].into_boxed_slice());
        for (path, mode) in entries {
            if Meta::from_git_mode_with(mode, ExecPolicy::PythonCompat).is_none()
                || (cfg!(windows) && mode & 0o170_000 == 0o120_000)
            {
                continue;
            }
            let from = order.partition_point(|&i| self.bytes_at(i) < path);
            for &i in order.get(from..).unwrap_or_default() {
                if self.bytes_at(i) != path {
                    break;
                }
                if let Some(slot) = modes.get_mut(i) {
                    *slot = mode;
                }
            }
        }
        self.git_modes = Some(modes);
    }

    /// Once, before any hook (02 §10).
    ///
    /// `Classifier.__init__`'s `[f for f in filenames if os.path.lexists(f)]` (`run.py:73-74`): drops every file
    /// whose `lstat` fails (deleted files, nonexistent `--files` entries); broken symlinks stay. Files with a trusted
    /// git mode need no syscall. Order and duplicates are kept.
    pub fn retain_lexists(&mut self) {
        let n = self.len();
        let keep: Vec<bool> = parallel_flags(n, |i| self.lexists_meta(i).is_some());
        if keep.iter().all(|&k| k) {
            return;
        }
        let spans: Box<[(u32, u32)]> = self
            .spans
            .iter()
            .zip(&keep)
            .filter_map(|(s, &k)| k.then_some(*s))
            .collect();
        self.spans = spans;
        if let Some(modes) = self.git_modes.take() {
            self.git_modes = Some(
                modes
                    .iter()
                    .zip(&keep)
                    .filter_map(|(m, &k)| k.then_some(*m))
                    .collect(),
            );
        }
        if let Some(names) = self.name.get_mut() {
            let old = std::mem::take(names);
            *names = old
                .into_vec()
                .into_iter()
                .zip(&keep)
                .filter_map(|(c, &k)| k.then_some(c))
                .collect();
        }
        self.stat.retain(&keep);
        self.head.retain(&keep);
        self.view.retain(&keep);
        if let Ok(p) = self.pending.get_mut() {
            *p = None;
        }
    }

    /// Number of files.
    ///
    /// `len(classifier.filenames)` (`run.py:73-74`) once `retain_lexists` ran.
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// Whether the table is empty.
    ///
    /// pre-commit's emptiness checks of the file list (`run.py:151`).
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The path bytes as git printed them.
    ///
    /// What pre-commit's `zsplit` would decode (`git.py:19-24`); snp keeps the bytes (D-CRASH-12). Empty for an id
    /// outside the table.
    pub fn bytes(&self, id: FileId) -> &[u8] {
        self.index(id).map_or(&[], |i| self.bytes_at(i))
    }

    /// The path as an `OsStr` (argv).
    ///
    /// Unix: the raw bytes (D-CRASH-12: pre-commit would crash encoding them, X13-GIT-06). Windows: the matching view
    /// (pre-commit passes the `/`-normalised names to hooks, `run.py:119-124`).
    pub fn os(&self, id: FileId) -> Cow<'_, OsStr> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Cow::Borrowed(OsStr::from_bytes(self.bytes(id)))
        }
        #[cfg(not(unix))]
        {
            Cow::Borrowed(OsStr::new(self.match_view(id)))
        }
    }

    /// Lossy UTF-8 (D-CRASH-12).
    ///
    /// What `files`/`exclude` match against: the name itself when it is valid UTF-8 (borrowed from the arena), else
    /// its lossy decoding (U+FFFD); on Windows `\` is replaced by `/` (`Classifier.from_config`, `run.py:119-124`).
    pub fn match_view(&self, id: FileId) -> &str {
        let Some(i) = self.index(id) else {
            return "";
        };
        // A valid arena: the span itself, unless it splits a character (joined `--files` arguments).
        if let Arena::Text(s) = &self.arena
            && let Some(v) = self.span(i).and_then(|r| s.get(r))
        {
            return v;
        }
        let bytes = self.bytes_at(i);
        if let Ok(s) = std::str::from_utf8(bytes)
            && !(cfg!(windows) && s.contains('\\'))
        {
            return s;
        }
        let n = self.len();
        self.view.cell(n, i).map_or("", |c| {
            c.get_or_init(|| {
                let lossy = String::from_utf8_lossy(bytes);
                if cfg!(windows) {
                    lossy.replace('\\', "/").into_boxed_str()
                } else {
                    lossy.into_owned().into_boxed_str()
                }
            })
        })
    }

    /// The path relative to `prefix` when it lies under it (boundary-aware: `a/b` is under `a`, `ab/c` is not); the
    /// `root` extension and the v1.1 workspace mode use this (§1.6).
    ///
    /// A prefix of `.` (or `./`) is the whole repository. A trailing `/` on the prefix is ignored. The path equal
    /// to the prefix itself is not under it. Unix compares raw bytes; Windows the `/`-normalised view.
    pub fn relative_to(&self, id: FileId, prefix: &RepoPath) -> Option<Cow<'_, [u8]>> {
        let path: &[u8] = if cfg!(windows) {
            self.match_view(id).as_bytes()
        } else {
            self.bytes(id)
        };
        let normalized_prefix;
        let mut p: &[u8] = prefix.as_bytes();
        if cfg!(windows) && p.contains(&b'\\') {
            normalized_prefix = p
                .iter()
                .map(|&b| if b == b'\\' { b'/' } else { b })
                .collect::<Vec<u8>>();
            p = &normalized_prefix;
        }
        while let Some(rest) = p.strip_prefix(b"./") {
            p = rest;
        }
        while let Some(rest) = p.strip_suffix(b"/") {
            p = rest;
        }
        if p.is_empty() || p == b"." {
            return Some(Cow::Borrowed(path));
        }
        let rest = path.strip_prefix(p)?.strip_prefix(b"/")?;
        (!rest.is_empty()).then_some(Cow::Borrowed(rest))
    }

    /// Lazy, memoised, thread-safe tiered identify. Each tier has its own cell, so a later call needing a higher
    /// tier never reads a partial result: the outcome combines exactly the tiers `n` requires.
    ///
    /// `identify.tags_from_path` (`identify.py:40-83`) split into tiers: `Needs::default()` gives the name tier
    /// (`tags_from_filename`, no I/O); any other `n` takes the stat tier (`lstat`: directory/symlink/socket ⇒ that
    /// single tag, else `file` + `executable`/`non-executable` + name tags) and, only where `n` asks for a category
    /// the stat tier left open (`text`/`binary` of an unknown name, the interpreter of an executable without name
    /// tags), the head tier. A filter whose referenced tags are all covered by `n` (`HookFilter::needs`) gets
    /// exactly the outcome full identification would give (S13). A file whose `lstat` or `open` reported it gone is
    /// `Vanished` here and in every later call (X13-CLS-01, D-CRASH-08); an unreadable file is `Unreadable` only for
    /// an `n` that needs its content (X13-CLS-02, D-CRASH-18). FIFOs and devices are read non-blocking (X13-CLS-03,
    /// D-CRASH-09).
    pub fn tags(&self, id: FileId, n: Needs) -> TagOutcome<'_> {
        let Some(i) = self.index(id) else {
            return TagOutcome::Vanished;
        };
        if self.known_vanished(id) {
            return TagOutcome::Vanished;
        }
        if !n.any() {
            return TagOutcome::Tags(self.name_tags(i));
        }
        match self.stat_tier(i) {
            StatTier::Vanished => TagOutcome::Vanished,
            StatTier::Present(Classification::Done(tags)) => TagOutcome::Tags(tags),
            StatTier::Present(Classification::NeedsContent(p)) => {
                let need = (n.encoding && p.needs_encoding()) || (n.shebang && p.needs_shebang());
                if !need {
                    return TagOutcome::Tags(p.tags());
                }
                match self.head_tier(i, p) {
                    Some(Head::Tags(t)) => TagOutcome::Tags(*t),
                    Some(Head::Unreadable(e)) => TagOutcome::Unreadable(e),
                    Some(Head::Vanished) | None => TagOutcome::Vanished,
                }
            }
        }
    }

    /// Whether `tags(id, n)` is answered from memoised cells alone (no `lstat`, `open` or name-tier computation):
    /// the file is known to be gone, or every tier `n` needs for it is computed (ADR-101). A pending `lexists` meta
    /// does not count: adopting it still classifies the name.
    pub(crate) fn tiers_ready(&self, id: FileId, n: Needs) -> bool {
        let Some(i) = self.index(id) else {
            return true;
        };
        if self.known_vanished(id) {
            return true;
        }
        if !n.any() {
            return self
                .name
                .get()
                .and_then(|ids| ids.get(i))
                .is_some_and(|c| c.load(Ordering::Acquire) != 0);
        }
        match self.stat.peek(i) {
            None => false,
            Some(StatTier::Vanished | StatTier::Present(Classification::Done(_))) => true,
            Some(StatTier::Present(Classification::NeedsContent(p))) => {
                let need = (n.encoding && p.needs_encoding()) || (n.shebang && p.needs_shebang());
                !need || self.head.peek(i).is_some()
            }
        }
    }

    /// Whether entry `id` is already known to be gone (a computed stat or head tier said so); no I/O.
    pub(crate) fn known_vanished(&self, id: FileId) -> bool {
        let Some(i) = self.index(id) else {
            return true;
        };
        matches!(self.head.peek(i), Some(Head::Vanished))
            || matches!(self.stat.peek(i), Some(StatTier::Vanished))
    }

    /// Fills the required tiers on the pool (runs inline when the caller holds a permit, §1.4.8).
    ///
    /// The work is split into partitions of at least 64 files, run by `JobPool::map_ordered` (one permit each); a
    /// small list, a single-permit pool or a caller that already holds a permit (a meta hook classifying files) runs
    /// inline. Cancellation only stops the prefetch: `tags` computes whatever is missing on demand.
    pub fn prefetch(&self, ids: &[FileId], n: Needs, pool: &JobPool) {
        if !n.any() || ids.is_empty() {
            return;
        }
        let _done: Result<Vec<()>, Failure> = par_chunks(pool, ids.len(), |range| {
            for &id in ids.get(range).unwrap_or_default() {
                let _outcome: TagOutcome<'_> = self.tags(id, n);
            }
            Ok(Vec::new())
        });
    }

    // ----- crate-private -----

    /// The index of a valid id.
    pub(crate) fn index(&self, id: FileId) -> Option<usize> {
        let i = usize::try_from(id.0).ok()?;
        (i < self.spans.len()).then_some(i)
    }

    /// The arena range of entry `i`.
    fn span(&self, i: usize) -> Option<Range<usize>> {
        let &(s, e) = self.spans.get(i)?;
        Some(usize::try_from(s).ok()?..usize::try_from(e).ok()?)
    }

    /// The bytes of entry `i` (empty when out of range).
    fn bytes_at(&self, i: usize) -> &[u8] {
        self.span(i)
            .and_then(|r| self.arena.as_bytes().get(r))
            .unwrap_or_default()
    }

    /// Where entry `i` is on disk: `root/<path>` (pre-commit runs in the root with relative names). One allocation
    /// of the final size; `push` keeps `join`'s rule that an absolute name replaces the root.
    fn fs_path(&self, i: usize) -> PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            joined(&self.root, OsStr::from_bytes(self.bytes_at(i)))
        }
        #[cfg(not(unix))]
        {
            let id = FileId(u32::try_from(i).unwrap_or(u32::MAX));
            self.root.join(self.match_view(id))
        }
    }

    /// The root descriptor (`O_PATH | O_DIRECTORY | O_CLOEXEC`), opened once; `None` when it cannot be opened, in
    /// which case every caller uses the path-based call (never "gone": that would skip every hook, K5).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn root_fd(&self) -> Option<rustix::fd::BorrowedFd<'_>> {
        use rustix::fd::AsFd;
        use rustix::fs::{Mode, OFlags};
        self.root_fd
            .get_or_init(|| {
                rustix::fs::open(
                    &self.root,
                    OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .ok()
            })
            .as_ref()
            .map(AsFd::as_fd)
    }

    /// `lstat` of entry `i` relative to the root descriptor (`statx`, type/mode/owner only): `Ok(Some(meta))`
    /// present, `Ok(None)` gone (every error, as `os.path.lexists`), `Err(PathFallback)` when the path-based call
    /// must decide (no descriptor, `statx` unavailable or refused, or a result without type and mode).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn statx_meta(&self, i: usize) -> Result<Option<Meta>, PathFallback> {
        use std::os::unix::ffi::OsStrExt;

        use rustix::fs::{AtFlags, StatxFlags};
        use rustix::io::Errno;
        let fd = self.root_fd().ok_or(PathFallback)?;
        let want = StatxFlags::TYPE | StatxFlags::MODE | StatxFlags::UID | StatxFlags::GID;
        match rustix::fs::statx(
            fd,
            OsStr::from_bytes(self.bytes_at(i)),
            AtFlags::SYMLINK_NOFOLLOW,
            want,
        ) {
            Ok(st) => {
                let got = StatxFlags::from_bits_retain(st.stx_mask);
                if got.contains(want) {
                    Ok(Some(stat_meta(
                        u32::from(st.stx_mode),
                        st.stx_uid,
                        st.stx_gid,
                    )))
                } else {
                    Err(PathFallback)
                }
            }
            Err(Errno::NOSYS | Errno::PERM) => Err(PathFallback),
            Err(_) => Ok(None),
        }
    }

    /// The trusted git meta of entry `i`, if any.
    fn trusted_meta(&self, i: usize) -> Option<Meta> {
        let mode = *self.git_modes.as_ref()?.get(i)?;
        if mode == 0 {
            return None;
        }
        Meta::from_git_mode_with(mode, ExecPolicy::PythonCompat)
    }

    /// `os.path.lexists` with the `lstat` meta: the trusted mode, else one `lstat`. An empty name does not exist
    /// (`os.lstat('')` raises `FileNotFoundError`; `root.join("")` would be the root directory itself).
    pub(crate) fn lexists_meta(&self, i: usize) -> Option<Meta> {
        if self.bytes_at(i).is_empty() {
            return None;
        }
        if let Some(m) = self.trusted_meta(i) {
            return Some(m);
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Ok(decided) = self.statx_meta(i) {
            return decided;
        }
        std::fs::symlink_metadata(self.fs_path(i))
            .ok()
            .map(|md| Meta::from_metadata(&md, ExecPolicy::PythonCompat))
    }

    /// The classification of a meta: FIFOs and devices are classified as regular files so their (non-blocking)
    /// content decides `text`/`binary`, as identify reads them (X13-CLS-03).
    fn classify(&self, i: usize, meta: Meta) -> Classification {
        let meta = if meta.kind() == FileKind::Other {
            Meta::new(FileKind::Regular, meta.is_executable())
        } else {
            meta
        };
        file_identify::classify(meta, self.bytes_at(i))
    }

    /// The memoised stat tier of entry `i`.
    fn stat_tier(&self, i: usize) -> StatTier {
        let n = self.len();
        let compute = || match self.lexists_meta(i) {
            Some(meta) => StatTier::Present(self.classify(i, meta)),
            None => StatTier::Vanished,
        };
        match self.stat.cell(n, i) {
            Some(c) => *c.get_or_init(compute),
            None => StatTier::Vanished,
        }
    }

    /// Records the `lexists` metas of a fresh `Classifier::new` (index-aligned with the table).
    pub(crate) fn set_pending(&self, metas: Box<[Option<Meta>]>) {
        *snp_core::sync::lock(&self.pending) = Some(metas);
    }

    /// Takes the pending `lexists` metas (once: the first hook after `Classifier::new` adopts them).
    ///
    /// pre-commit identifies a file the first time a hook's `files`/`exclude` pass it; adopting the `lstat` taken
    /// moments before (by `Classifier::new`) for exactly those files is that same observation, while every other
    /// file keeps a fresh `lstat` at its first use, so a file deleted by an earlier hook is seen as gone
    /// (X13-CLS-01).
    pub(crate) fn take_pending(&self) -> Option<Box<[Option<Meta>]>> {
        snp_core::sync::lock(&self.pending).take()
    }

    /// Sets the stat tier of `id` from its pending `lexists` meta, when it has one and the tier is not computed.
    pub(crate) fn adopt(&self, id: FileId, pending: &[Option<Meta>]) {
        let Some(i) = self.index(id) else { return };
        let Some(Some(meta)) = pending.get(i) else {
            return;
        };
        let n = self.len();
        if let Some(c) = self.stat.cell(n, i)
            && c.get().is_none()
        {
            let _set: Result<(), StatTier> = c.set(StatTier::Present(self.classify(i, *meta)));
        }
    }

    /// The memoised head tier of entry `i`.
    fn head_tier(&self, i: usize, pending: Pending) -> Option<&Head> {
        let n = self.len();
        let cell = self.head.cell(n, i)?;
        Some(cell.get_or_init(|| self.read_head(i, pending)))
    }

    /// One non-blocking `open` + bounded read through `Pending::finish`.
    fn read_head(&self, i: usize, pending: Pending) -> Head {
        // `open('')` raises `FileNotFoundError` (the root directory is not what was named).
        if self.bytes_at(i).is_empty() {
            return Head::Vanished;
        }
        let file = match self.open_head(i) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Head::Vanished,
            Err(e) => return Head::Unreadable(Box::new(self.os_error(i, &e))),
        };
        match pending.finish(NonBlocking(file)) {
            Ok(tags) => Head::Tags(tags),
            Err(e) => Head::Unreadable(Box::new(self.os_error(i, &e))),
        }
    }

    /// `open(name, 'rb')` of entry `i`, non-blocking and without acquiring a terminal: on Linux `openat` relative to
    /// the root descriptor (with `O_CLOEXEC`, so no spawned child inherits it), else by path.
    fn open_head(&self, i: usize) -> io::Result<File> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Some(fd) = self.root_fd() {
            use std::os::unix::ffi::OsStrExt;

            use rustix::fs::{Mode, OFlags};
            let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
            let name = OsStr::from_bytes(self.bytes_at(i));
            // Retried on EINTR, as std's `open` is.
            return loop {
                match rustix::fs::openat(fd, name, flags, Mode::empty()) {
                    Err(rustix::io::Errno::INTR) => {}
                    other => {
                        break other
                            .map(File::from)
                            .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error()));
                    }
                }
            };
        }
        open_nonblocking(&self.fs_path(i))
    }

    /// The `OSError` pre-commit reports for entry `i` (its relative name, as passed to `tags_from_path`).
    fn os_error(&self, i: usize, e: &io::Error) -> PyExc {
        let id = FileId(u32::try_from(i).unwrap_or(u32::MAX));
        let name = self.os(id);
        PyExc::new(
            pyerr::os_error_class(e),
            pyerr::os_error_text(e, Some(&name), None),
        )
    }

    /// The memoised, interned name tier of entry `i`.
    fn name_tags(&self, i: usize) -> TagSet {
        let n = self.len();
        let ids = self
            .name
            .get_or_init(|| (0..n).map(|_| AtomicU16::new(0)).collect());
        let Some(cell) = ids.get(i) else {
            return TagSet::EMPTY;
        };
        let id = cell.load(Ordering::Acquire);
        if id != 0 {
            let interner = snp_core::sync::lock(&self.interner);
            if let Some(t) = interner.sets.get(usize::from(id).saturating_sub(1)) {
                return *t;
            }
        }
        let tags = file_identify::tags_from_filename(self.bytes_at(i));
        let mut interner = snp_core::sync::lock(&self.interner);
        let slot = if let Some(&k) = interner.index.get(&tags) {
            Some(k)
        } else {
            let k = u16::try_from(interner.sets.len())
                .ok()
                .filter(|&k| k < u16::MAX);
            if let Some(k) = k {
                interner.sets.push(tags);
                interner.index.insert(tags, k);
            }
            k
        };
        drop(interner);
        if let Some(k) = slot {
            cell.store(k.saturating_add(1), Ordering::Release);
        }
        tags
    }
}

/// Runs `f` over `0..n` in ordered partitions of at least 64 indices on the pool (`JobPool::map_ordered`, one
/// permit each) and concatenates the results in index order; the first error in index order wins. A small `n`, a
/// single-permit pool or a caller that already holds a permit runs inline (ADR-070 re-entrancy).
pub(crate) fn par_chunks<T: Send>(
    pool: &JobPool,
    n: usize,
    f: impl Fn(Range<usize>) -> Result<Vec<T>, Failure> + Sync,
) -> Result<Vec<T>, Failure> {
    par_chunks_on(pool, n, usize::MAX, false, f)
}

/// The most threads that match `files`/`exclude` patterns at once, whatever the pool size: every thread that runs a
/// pattern gets its own lazy-DFA cache for it (~77 KB each), so 20 patterns on 32 threads held 27 MB of caches (B12u)
/// while the wall time hardly improves beyond 8 threads. The pool's core count caps it further (ADR-101): more
/// matching threads than cores only add caches and wake-ups (4 on a 4-CPU CI runner with `SNP_JOBS=32`).
pub(crate) const REGEX_THREADS: usize = 8;

/// [`par_chunks`] on at most min(`max_threads`, permits, cores) threads (ADR-101).
///
/// `cheap` says the caller knows the work is memory-only (no regex call, no `lstat`/`open`): then a large `n` runs
/// on the calling thread too, as one partition under a permit (`JobPool::run_inline`: cancellation and panics as on
/// the pool), since starting threads costs more than the work. Deterministic: no timing decides the thread count.
pub(crate) fn par_chunks_on<T: Send>(
    pool: &JobPool,
    n: usize,
    max_threads: usize,
    cheap: bool,
    f: impl Fn(Range<usize>) -> Result<Vec<T>, Failure> + Sync,
) -> Result<Vec<T>, Failure> {
    let permits = pool.permits();
    let threads = permits
        .get()
        .min(max_threads.max(1))
        .min(pool.cores().get());
    if n < PARALLEL_MIN || threads == 1 || JobPool::holds_permit() {
        return f(0..n);
    }
    if cheap {
        return pool.run_inline(|| f(0..n));
    }
    let threads = NonZeroUsize::new(threads).unwrap_or(permits);
    let per = n.div_ceil(threads.get().saturating_mul(4)).max(CHUNK_MIN);
    let chunks = n.div_ceil(per);
    let parts = pool.map_ordered(threads, chunks, |k| {
        let start = k.saturating_mul(per);
        f(start..start.saturating_add(per).min(n))
    });
    let mut out = Vec::new();
    for part in parts {
        out.extend(part?);
    }
    Ok(out)
}

/// Runs `f(0..n)` and collects the flags, on a few scoped threads for large tables (`lstat` latency dominates).
fn parallel_flags(n: usize, f: impl Fn(usize) -> bool + Sync) -> Vec<bool> {
    const PER_THREAD: usize = 4096;
    let threads = std::thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .min(8)
        .min(n.div_ceil(PER_THREAD));
    if threads <= 1 {
        return (0..n).map(f).collect();
    }
    let per = n.div_ceil(threads);
    let parts: Option<Vec<Vec<bool>>> = std::thread::scope(|s| {
        let mut handles = Vec::new();
        for t in 0..threads {
            let start = t.saturating_mul(per);
            let end = start.saturating_add(per).min(n);
            let f = &f;
            let spawned = std::thread::Builder::new()
                .name(format!("snp-lexists-{t}"))
                .spawn_scoped(s, move || (start..end).map(f).collect::<Vec<bool>>());
            if let Ok(h) = spawned {
                handles.push(h);
            } else {
                for h in handles {
                    let _joined = h.join();
                }
                return None;
            }
        }
        let mut out = Vec::new();
        let mut ok = true;
        for h in handles {
            match h.join() {
                Ok(part) => out.push(part),
                Err(_) => ok = false,
            }
        }
        ok.then_some(out)
    });
    match parts {
        Some(parts) => parts.concat(),
        None => (0..n).map(f).collect(),
    }
}

/// The bytes of an `OsStr`: raw on Unix, WTF-8 elsewhere.
fn os_bytes(p: &OsStr) -> &[u8] {
    p.as_encoded_bytes()
}

/// `root.join(name)` in one allocation of the final size (`push` replaces the root when `name` is absolute).
#[cfg(unix)]
fn joined(root: &Path, name: &OsStr) -> PathBuf {
    let root_len = root.as_os_str().len();
    let mut p = PathBuf::with_capacity(root_len.saturating_add(1).saturating_add(name.len()));
    p.as_mut_os_string().push(root.as_os_str());
    p.push(name);
    p
}

/// `Meta::from_metadata(lstat, ExecPolicy::PythonCompat)` from a raw `st_mode`, `st_uid` and `st_gid`: the kind from
/// `S_IFMT` (directory, symlink, socket, regular, anything else) and, for regular files and the rest, identify's
/// `os.access(X_OK)` emulation (root needs any `x` bit, the owner the user bit, a group member the group bit, others
/// the other bit). A differential test pins it to file-identify; file-identify 0.5.x will expose it as
/// `Meta::from_unix_stat` (ADR-101).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn stat_meta(mode: u32, uid: u32, gid: u32) -> Meta {
    const S_IFMT: u32 = 0o170_000;
    let kind = match mode & S_IFMT {
        0o040_000 => FileKind::Directory,
        0o120_000 => FileKind::Symlink,
        0o140_000 => FileKind::Socket,
        0o100_000 => FileKind::Regular,
        _ => FileKind::Other,
    };
    let executable = matches!(kind, FileKind::Regular | FileKind::Other) && {
        let ids = process_ids();
        if ids.uid == 0 {
            mode & 0o111 != 0
        } else if uid == ids.uid {
            mode & 0o100 != 0
        } else if gid == ids.gid || ids.groups().contains(&gid) {
            mode & 0o010 != 0
        } else {
            mode & 0o001 != 0
        }
    };
    Meta::new(kind, executable)
}

/// `FileTable::statx_meta` could not decide: the path-based `lstat` must.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[derive(Debug, PartialEq, Eq)]
struct PathFallback;

/// The process's real uid and gid (queried once) and its supplementary groups (queried on first need).
#[cfg(any(target_os = "linux", target_os = "android"))]
struct ProcessIds {
    /// `getuid()`.
    uid: u32,
    /// `getgid()`.
    gid: u32,
    /// `getgroups()`, empty when it fails.
    groups: OnceLock<Box<[u32]>>,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl ProcessIds {
    /// The supplementary groups.
    fn groups(&self) -> &[u32] {
        self.groups.get_or_init(|| {
            rustix::process::getgroups().map_or_else(
                |_| Box::default(),
                |g| g.into_iter().map(rustix::process::Gid::as_raw).collect(),
            )
        })
    }
}

/// The cached [`ProcessIds`].
#[cfg(any(target_os = "linux", target_os = "android"))]
fn process_ids() -> &'static ProcessIds {
    static IDS: OnceLock<ProcessIds> = OnceLock::new();
    IDS.get_or_init(|| ProcessIds {
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
        groups: OnceLock::new(),
    })
}

/// `open(path, 'rb')` that never blocks on a FIFO or acquires a terminal (`O_NONBLOCK | O_NOCTTY`, X13-CLS-03).
fn open_nonblocking(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let flags = rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOCTTY;
        let flags = i32::try_from(flags.bits()).unwrap_or(0);
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(flags)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        File::open(path)
    }
}

/// A reader that ends at "no data yet" instead of failing with `EAGAIN` (a FIFO whose writer has not written).
struct NonBlocking(File);

impl Read for NonBlocking {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buf) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B12u: pattern matching uses at most `REGEX_THREADS` threads whatever the pool size; results stay in order.
    #[test]
    fn par_chunks_on_caps_the_threads() {
        let pool = JobPool::new(
            NonZeroUsize::new(32).unwrap(),
            snp_core::cancel::CancelToken::new(),
        );
        let threads = |pool: &JobPool| {
            let seen = Mutex::new(std::collections::HashSet::new());
            let out = par_chunks_on(pool, 20_000, REGEX_THREADS, false, |r| {
                seen.lock().unwrap().insert(std::thread::current().id());
                std::thread::sleep(std::time::Duration::from_micros(200));
                Ok(r.collect::<Vec<usize>>())
            })
            .unwrap();
            assert_eq!(out, (0..20_000).collect::<Vec<_>>());
            seen.into_inner().unwrap().len()
        };
        // The calling thread may take every chunk before a helper starts (a 1-core runner), hence `1..`.
        let n = threads(&pool);
        assert!((1..=REGEX_THREADS).contains(&n), "{n}");
        // ADR-101: the pool's core count caps it further.
        let two_cores = JobPool::with_cores(
            NonZeroUsize::new(32).unwrap(),
            NonZeroUsize::new(2).unwrap(),
            snp_core::cancel::CancelToken::new(),
        );
        let n = threads(&two_cores);
        assert!((1..=2).contains(&n), "{n}");
    }

    /// The first error in index order wins, whether the chunks run inline, on the pool or as one cheap partition;
    /// the cheap partition runs on the calling thread under a permit.
    #[test]
    fn par_chunks_on_first_error_in_index_order_wins() {
        let fail_at = |r: Range<usize>| -> Result<Vec<usize>, Failure> {
            for i in r.clone() {
                if i == 300 || i == 9_000 {
                    return Err(Failure::fatal("t", format!("at {i}")));
                }
            }
            Ok(r.collect())
        };
        let text = |e: Failure| match e {
            Failure::Exc { exc, .. } => String::from_utf8_lossy(&exc.text).into_owned(),
            other => format!("{other:?}"),
        };
        for permits in [1, 8] {
            let pool = JobPool::new(
                NonZeroUsize::new(permits).unwrap(),
                snp_core::cancel::CancelToken::new(),
            );
            for cheap in [false, true] {
                let e = par_chunks_on(&pool, 10_000, REGEX_THREADS, cheap, fail_at).unwrap_err();
                assert_eq!(text(e), "at 300", "permits {permits}, cheap {cheap}");
            }
        }
        let pool = JobPool::new(
            NonZeroUsize::new(4).unwrap(),
            snp_core::cancel::CancelToken::new(),
        );
        let me = std::thread::current().id();
        let out = par_chunks_on(&pool, 1_000, REGEX_THREADS, true, |r| {
            assert!(JobPool::holds_permit());
            assert_eq!(std::thread::current().id(), me);
            Ok(r.collect::<Vec<usize>>())
        })
        .unwrap();
        assert_eq!(out.len(), 1_000);
    }

    fn table(paths: &[&str]) -> FileTable {
        FileTable::from_args(
            paths.iter().map(OsString::from).collect(),
            Path::new("/nonexistent-root"),
        )
    }

    #[test]
    fn arena_and_spans() {
        let t = FileTable::from_zbuf(
            b"a.py\0b/c.txt\0".to_vec(),
            vec![(0, 4), (5, 12), (9, 99), (7, 6)],
            Path::new("/r"),
        );
        assert_eq!(t.len(), 2);
        assert_eq!(t.bytes(FileId(0)), b"a.py");
        assert_eq!(t.bytes(FileId(1)), b"b/c.txt");
        assert_eq!(t.bytes(FileId(2)), b"");
        assert_eq!(t.match_view(FileId(7)), "");
        assert!(matches!(
            t.tags(FileId(9), Needs::default()),
            TagOutcome::Vanished
        ));
    }

    #[test]
    fn name_tier_is_interned() {
        let t = table(&["a.py", "b.py", "README.md", "a.py"]);
        for _ in 0..2 {
            for i in 0..4 {
                let TagOutcome::Tags(tags) = t.tags(FileId(i), Needs::default()) else {
                    panic!("name tier never fails")
                };
                assert_eq!(tags, file_identify::tags_from_filename(t.bytes(FileId(i))));
            }
        }
        assert_eq!(snp_core::sync::lock(&t.interner).sets.len(), 2);
    }

    #[test]
    fn relative_to_is_boundary_aware() {
        let t = table(&["a/b", "ab/c", "a", "a/b/c"]);
        let p = RepoPath::new(b"a").unwrap();
        assert_eq!(t.relative_to(FileId(0), &p).as_deref(), Some(&b"b"[..]));
        assert_eq!(t.relative_to(FileId(1), &p), None);
        assert_eq!(t.relative_to(FileId(2), &p), None);
        assert_eq!(t.relative_to(FileId(3), &p).as_deref(), Some(&b"b/c"[..]));
        let slash = RepoPath::new(b"a/").unwrap();
        assert_eq!(t.relative_to(FileId(0), &slash).as_deref(), Some(&b"b"[..]));
        let dot = RepoPath::new(b".").unwrap();
        assert_eq!(
            t.relative_to(FileId(1), &dot).as_deref(),
            Some(&b"ab/c"[..])
        );
        let dot_slash = RepoPath::new(b"./a").unwrap();
        assert_eq!(
            t.relative_to(FileId(3), &dot_slash).as_deref(),
            Some(&b"b/c"[..])
        );
    }

    // POSIX only: on Windows `os.access(X_OK)` holds for every file, so identify (and snp) never take
    // `executable` from a mode there.
    #[cfg(unix)]
    #[test]
    fn trusted_modes_match_duplicates_by_path() {
        let mut t = table(&["x.sh", "a.py", "x.sh", "gone"]);
        t.set_trusted_modes(vec![
            (&b"x.sh"[..], 0o100_755),
            (&b"a.py"[..], 0o100_644),
            (&b"zzz"[..], 0o100_644),
        ]);
        let n = Needs {
            stat: true,
            ..Needs::default()
        };
        for id in [0, 2] {
            let TagOutcome::Tags(tags) = t.tags(FileId(id), n) else {
                panic!()
            };
            assert!(
                tags.contains("executable") && tags.contains("shell"),
                "{tags}"
            );
        }
        let TagOutcome::Tags(tags) = t.tags(FileId(1), n) else {
            panic!()
        };
        assert!(tags.contains("non-executable") && tags.contains("python"));
        // No mode and no file under the (nonexistent) root: gone.
        assert!(matches!(t.tags(FileId(3), n), TagOutcome::Vanished));
        // A mode that names no file kind is ignored.
        let mut t = table(&["q"]);
        t.set_trusted_modes(vec![(&b"q"[..], 0o777)]);
        assert!(matches!(t.tags(FileId(0), n), TagOutcome::Vanished));
    }

    /// ADR-101: a valid arena is validated once; its views are slices of it.
    #[test]
    fn valid_arena_views_borrow_the_arena() {
        let t = FileTable::from_zbuf(
            b"a.py\0d\xc3\xa9j\xc3\xa0/b\0".to_vec(),
            vec![(0, 4), (5, 13)],
            Path::new("/r"),
        );
        assert!(matches!(t.arena, Arena::Text(_)) || cfg!(windows));
        assert_eq!(t.match_view(FileId(0)), "a.py");
        assert_eq!(t.match_view(FileId(1)), "déjà/b");
        let arena = t.arena.as_bytes().as_ptr_range();
        assert!(arena.contains(&t.match_view(FileId(1)).as_ptr()));
        assert!(t.view.0.get().is_none(), "no lossy cell was needed");
    }

    /// `--files` arguments are joined without a separator, so a valid arena can hold spans that split a character:
    /// their views are the lossy decoding of each span, never "".
    #[cfg(unix)]
    #[test]
    fn joined_args_that_split_a_character_get_lossy_views() {
        use std::os::unix::ffi::OsStringExt;
        let t = FileTable::from_args(
            vec![
                OsString::from_vec(b"x\xc3".to_vec()),
                OsString::from_vec(b"\xa9y.py".to_vec()),
            ],
            Path::new("/r"),
        );
        assert!(
            matches!(t.arena, Arena::Text(_)),
            "x\u{e9}y.py is valid UTF-8"
        );
        assert_eq!(t.match_view(FileId(0)), "x\u{fffd}");
        assert_eq!(t.match_view(FileId(1)), "\u{fffd}y.py");
        assert_eq!(t.bytes(FileId(1)), b"\xa9y.py");
    }

    /// An invalid arena checks each span: valid names borrow, invalid ones get the lossy view.
    #[test]
    fn mixed_arena_checks_each_span() {
        let t = FileTable::from_zbuf(
            b"ok.py\0bad\xff.py\0\xc3\xa9.txt\0".to_vec(),
            vec![(0, 5), (6, 13), (14, 20)],
            Path::new("/r"),
        );
        assert!(matches!(t.arena, Arena::Bytes(_)));
        assert_eq!(t.match_view(FileId(0)), "ok.py");
        assert_eq!(t.match_view(FileId(1)), "bad\u{fffd}.py");
        assert_eq!(t.match_view(FileId(2)), "\u{e9}.txt");
    }

    /// `joined` is `Path::join` (an absolute name replaces the root).
    #[cfg(unix)]
    #[test]
    fn joined_is_join() {
        for (root, name) in [
            ("/r", "a/b"),
            ("/r/", "a"),
            ("", "a"),
            ("rel", "x"),
            ("/r", "/abs/x"),
            ("/", "a"),
        ] {
            assert_eq!(
                joined(Path::new(root), OsStr::new(name)),
                Path::new(root).join(name),
                "{root:?} {name:?}"
            );
        }
    }

    /// ADR-101: the `statx` stat tier equals `Meta::from_metadata(symlink_metadata)` for every kind and mode,
    /// including a broken symlink (present), files of another owner and absolute names (`/dev/null`).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn statx_meta_matches_file_identify() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let mut names: Vec<&str> = Vec::new();
        for (name, mode) in [
            ("r644", 0o644),
            ("r744", 0o744),
            ("r654", 0o654),
            ("r645", 0o645),
            ("r755", 0o755),
            ("r000", 0o000),
            ("r011", 0o011),
        ] {
            std::fs::write(root.join(name), b"x").unwrap();
            std::fs::set_permissions(root.join(name), std::fs::Permissions::from_mode(mode))
                .unwrap();
            names.push(name);
        }
        std::os::unix::fs::symlink("r644", root.join("link")).unwrap();
        std::os::unix::fs::symlink("missing", root.join("broken")).unwrap();
        std::fs::create_dir_all(root.join("dir")).unwrap();
        let fd = rustix::fs::open(
            root,
            rustix::fs::OFlags::PATH | rustix::fs::OFlags::DIRECTORY,
            rustix::fs::Mode::empty(),
        )
        .unwrap();
        rustix::fs::mknodat(
            &fd,
            "fifo",
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o644),
            0,
        )
        .unwrap();
        let _sock = std::os::unix::net::UnixListener::bind(root.join("sock")).unwrap();
        names.extend([
            "link",
            "broken",
            "dir",
            "fifo",
            "sock",
            "/dev/null",
            "/etc/passwd",
            "/bin/sh",
            "/usr/bin/env",
            "gone",
            "dir/../r755",
        ]);
        let t = FileTable::from_args(names.iter().map(OsString::from).collect(), root);
        assert!(t.root_fd().is_some());
        for (i, name) in names.iter().enumerate() {
            let path = root.join(name);
            let want = std::fs::symlink_metadata(&path)
                .ok()
                .map(|md| Meta::from_metadata(&md, ExecPolicy::PythonCompat));
            assert_eq!(t.statx_meta(i).ok(), Some(want), "{name}");
            assert_eq!(t.lexists_meta(i), want, "{name}");
        }
        assert!(
            matches!(t.lexists_meta(names.iter().position(|&n| n == "broken").unwrap()), Some(m) if m.kind() == FileKind::Symlink)
        );
    }

    /// A root that cannot be opened falls back to the path-based calls: never "gone" (K5).
    #[cfg(unix)]
    #[test]
    fn unopenable_root_falls_back_to_paths() {
        // The test runs in the package directory: `"" / Cargo.toml` is a relative path that exists.
        let t = table_at(&["Cargo.toml", "src/lib.rs", "missing"], "");
        #[cfg(any(target_os = "linux", target_os = "android"))]
        assert!(t.root_fd().is_none());
        assert!(t.lexists_meta(0).is_some());
        let TagOutcome::Tags(tags) = t.tags(
            FileId(1),
            Needs {
                stat: true,
                encoding: true,
                shebang: true,
            },
        ) else {
            panic!("src/lib.rs is readable")
        };
        assert!(tags.contains("text") && tags.contains("rust"), "{tags}");
        assert!(t.lexists_meta(2).is_none());
    }

    #[cfg(unix)]
    fn table_at(paths: &[&str], root: &str) -> FileTable {
        FileTable::from_args(paths.iter().map(OsString::from).collect(), Path::new(root))
    }

    /// The root descriptor and every file the head tier opens carry `FD_CLOEXEC` (a spawned hook inherits neither).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn descriptors_are_close_on_exec() {
        use rustix::io::FdFlags;
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.unknownext"), b"hello\n").unwrap();
        let t = FileTable::from_args(vec!["a.unknownext".into()], d.path());
        let root = t.root_fd().unwrap();
        assert!(
            rustix::io::fcntl_getfd(root)
                .unwrap()
                .contains(FdFlags::CLOEXEC)
        );
        let f = t.open_head(0).unwrap();
        assert!(
            rustix::io::fcntl_getfd(&f)
                .unwrap()
                .contains(FdFlags::CLOEXEC)
        );
        let flags = rustix::fs::fcntl_getfl(&f).unwrap();
        assert!(flags.contains(rustix::fs::OFlags::NONBLOCK));
    }

    /// An unreadable file reports pre-commit's `PermissionError` text through the dirfd-relative `openat`.
    #[cfg(unix)]
    #[test]
    fn unreadable_head_reports_the_python_error() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secret.unknownext");
        std::fs::write(&p, b"x").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        let t = FileTable::from_args(vec!["secret.unknownext".into()], d.path());
        let n = Needs {
            stat: true,
            encoding: true,
            shebang: false,
        };
        let TagOutcome::Unreadable(exc) = t.tags(FileId(0), n) else {
            panic!("chmod 000 must be unreadable")
        };
        assert_eq!(&*exc.ty, "PermissionError");
        assert_eq!(
            String::from_utf8_lossy(&exc.text),
            "[Errno 13] Permission denied: 'secret.unknownext'"
        );
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// A root plus name longer than `PATH_MAX` still finds the file relative to the root descriptor, as
    /// pre-commit's relative `lstat`/`open` in the root do.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn over_long_root_plus_name_counts_as_present() {
        use rustix::fs::{Mode, OFlags};
        let d = tempfile::tempdir().unwrap();
        let comp: String = std::iter::repeat_n('d', 200).collect();
        // Root: ~12 × 201 bytes (under PATH_MAX); name: 12 × 201 bytes more (the sum is over 4096).
        let mut root = d.path().to_path_buf();
        for _ in 0..12 {
            root.push(&comp);
        }
        std::fs::create_dir_all(&root).unwrap();
        let mut dir = rustix::fs::open(
            &root,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .unwrap();
        let mut rel = String::new();
        for _ in 0..12 {
            rustix::fs::mkdirat(&dir, comp.as_str(), Mode::from_raw_mode(0o755)).unwrap();
            dir = rustix::fs::openat(
                &dir,
                comp.as_str(),
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .unwrap();
            rel.push_str(&comp);
            rel.push('/');
        }
        let f = rustix::fs::openat(
            &dir,
            "f.py",
            OFlags::WRONLY | OFlags::CREATE | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o644),
        )
        .unwrap();
        drop(f);
        rel.push_str("f.py");
        assert!(root.as_os_str().len() + rel.len() > 4096);
        let t = FileTable::from_args(vec![rel.into()], &root);
        let n = Needs {
            stat: true,
            encoding: true,
            shebang: true,
        };
        assert!(
            matches!(t.tags(FileId(0), n), TagOutcome::Tags(tags) if tags.contains("python") && tags.contains("text"))
        );
    }

    #[test]
    fn retain_lexists_keeps_trusted_and_compacts() {
        let mut t = table(&["keep.py", "gone.py", "keep2.py"]);
        t.set_trusted_modes(vec![
            (&b"keep.py"[..], 0o100_644),
            (&b"keep2.py"[..], 0o100_755),
        ]);
        let _name: TagOutcome<'_> = t.tags(FileId(2), Needs::default());
        t.retain_lexists();
        assert_eq!(t.len(), 2);
        assert_eq!(t.bytes(FileId(1)), b"keep2.py");
        let TagOutcome::Tags(tags) = t.tags(
            FileId(1),
            Needs {
                stat: true,
                ..Needs::default()
            },
        ) else {
            panic!()
        };
        assert!(tags.contains("executable"));
        let TagOutcome::Tags(tags) = t.tags(FileId(1), Needs::default()) else {
            panic!()
        };
        assert_eq!(tags, file_identify::tags_from_filename("keep2.py"));
    }
}
