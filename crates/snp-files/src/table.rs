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
//! Owner: WP-22.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};

use file_identify::{Classification, ExecPolicy, FileKind, Meta, Pending, TagSet};
use snp_core::failure::{PyExc, pyerr};
use snp_core::path::RepoPath;
use snp_os as _;
use snp_proc::pool::JobPool;
use snp_pycompat as _;

/// Index of a file in its [`FileTable`].
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

/// One byte arena + spans; tier columns allocated on first need (§2.3, ADR-079).
pub struct FileTable {
    /// All paths, as git printed them.
    arena: Vec<u8>,
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
}

impl fmt::Debug for FileTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileTable")
            .field("len", &self.spans.len())
            .field("arena_bytes", &self.arena.len())
            .field("root", &self.root)
            .field("git_modes", &self.git_modes.is_some())
            .finish_non_exhaustive()
    }
}

/// Below this many files `prefetch` runs inline (thread start-up costs more than the work).
const PARALLEL_MIN: usize = 256;
/// Files per prefetch partition (at least).
const CHUNK_MIN: usize = 64;

impl FileTable {
    /// From git `-z` output (git order kept); Windows: `\` → `/` in the matching view (02 §10).
    ///
    /// `spans` are the half-open `(start, end)` offsets of `snp_git::ZList::into_parts`; a span outside `buf` (never
    /// produced by git) is dropped. Order and duplicates are kept (`diff.orderFile` parity, 02 G.0 #8).
    pub fn from_zbuf(buf: Vec<u8>, spans: Vec<(u32, u32)>, root: &Path) -> Self {
        let len = buf.len();
        let valid = |&(s, e): &(u32, u32)| {
            s <= e && usize::try_from(e).is_ok_and(|e| e <= len)
        };
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
            arena,
            spans,
            root: root.to_path_buf(),
            git_modes: None,
            name: OnceLock::new(),
            interner: Mutex::new(Interner::default()),
            stat: Column::new(),
            head: Column::new(),
            view: Column::new(),
            pending: Mutex::new(None),
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
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The path bytes as git printed them.
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
        if matches!(self.head.peek(i), Some(Head::Vanished))
            || matches!(self.stat.peek(i), Some(StatTier::Vanished))
        {
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

    /// Fills the required tiers on the pool (runs inline when the caller holds a permit, §1.4.8).
    ///
    /// The work is split into partitions of at least 64 files, run by `JobPool::map_ordered` (one permit each); a
    /// small list, a single-permit pool or a caller that already holds a permit (a meta hook classifying files) runs
    /// inline. Cancellation only stops the prefetch: `tags` computes whatever is missing on demand.
    pub fn prefetch(&self, ids: &[FileId], n: Needs, pool: &JobPool) {
        if !n.any() || ids.is_empty() {
            return;
        }
        let work = |chunk: &[FileId]| {
            for &id in chunk {
                let _outcome: TagOutcome<'_> = self.tags(id, n);
            }
        };
        let permits = pool.permits();
        if ids.len() < PARALLEL_MIN || permits.get() == 1 || JobPool::holds_permit() {
            work(ids);
            return;
        }
        let per = ids
            .len()
            .div_ceil(permits.get().saturating_mul(4))
            .max(CHUNK_MIN);
        let chunks = ids.len().div_ceil(per);
        let _done = pool.map_ordered(permits, chunks, |k| {
            let start = k.saturating_mul(per);
            let end = start.saturating_add(per).min(ids.len());
            work(ids.get(start..end).unwrap_or_default());
            Ok(())
        });
    }

    // ----- crate-private -----

    /// The index of a valid id.
    pub(crate) fn index(&self, id: FileId) -> Option<usize> {
        let i = usize::try_from(id.0).ok()?;
        (i < self.spans.len()).then_some(i)
    }

    /// The bytes of entry `i` (empty when out of range).
    fn bytes_at(&self, i: usize) -> &[u8] {
        self.spans
            .get(i)
            .and_then(|&(s, e)| {
                self.arena
                    .get(usize::try_from(s).ok()?..usize::try_from(e).ok()?)
            })
            .unwrap_or_default()
    }

    /// Where entry `i` is on disk: `root/<path>` (pre-commit runs in the root with relative names).
    fn fs_path(&self, i: usize) -> PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            self.root.join(OsStr::from_bytes(self.bytes_at(i)))
        }
        #[cfg(not(unix))]
        {
            let id = FileId(u32::try_from(i).unwrap_or(u32::MAX));
            self.root.join(self.match_view(id))
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

    /// `os.path.lexists` with the `lstat` meta: the trusted mode, else one `lstat`.
    pub(crate) fn lexists_meta(&self, i: usize) -> Option<Meta> {
        if let Some(m) = self.trusted_meta(i) {
            return Some(m);
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

    /// Sets the stat tier of entry `i` from an earlier `lexists` meta (when not computed yet).
    pub(crate) fn adopt_stat(&self, i: usize, meta: Meta) {
        let n = self.len();
        if let Some(c) = self.stat.cell(n, i) {
            let _set: Result<(), StatTier> = c.set(StatTier::Present(self.classify(i, meta)));
        }
    }

    /// Records the `lexists` metas of a fresh `Classifier::new` (index-aligned with the table).
    pub(crate) fn set_pending(&self, metas: Box<[Option<Meta>]>) {
        *snp_core::sync::lock(&self.pending) = Some(metas);
    }

    /// Adopts the pending `lexists` metas for `ids` (once: the first hook after `Classifier::new`). pre-commit
    /// identifies a file the first time a hook's `files`/`exclude` pass it; adopting the `lstat` taken moments
    /// before for exactly those files is that same observation, while every other file keeps a fresh `lstat` at its
    /// first use (so a file deleted by an earlier hook is seen as gone, X13-CLS-01).
    pub(crate) fn adopt_pending(&self, ids: &[FileId]) {
        let Some(metas) = snp_core::sync::lock(&self.pending).take() else {
            return;
        };
        for &id in ids {
            if let Some(i) = self.index(id)
                && let Some(Some(meta)) = metas.get(i)
            {
                self.adopt_stat(i, *meta);
            }
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
        let path = self.fs_path(i);
        let file = match open_nonblocking(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Head::Vanished,
            Err(e) => return Head::Unreadable(Box::new(self.os_error(i, &e))),
        };
        match pending.finish(NonBlocking(file)) {
            Ok(tags) => Head::Tags(tags),
            Err(e) => Head::Unreadable(Box::new(self.os_error(i, &e))),
        }
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

    fn table(paths: &[&str]) -> FileTable {
        FileTable::from_args(paths.iter().map(OsString::from).collect(), Path::new("/nonexistent-root"))
    }

    #[test]
    fn arena_and_spans() {
        let t = FileTable::from_zbuf(b"a.py\0b/c.txt\0".to_vec(), vec![(0, 4), (5, 12), (9, 99), (7, 6)], Path::new("/r"));
        assert_eq!(t.len(), 2);
        assert_eq!(t.bytes(FileId(0)), b"a.py");
        assert_eq!(t.bytes(FileId(1)), b"b/c.txt");
        assert_eq!(t.bytes(FileId(2)), b"");
        assert_eq!(t.match_view(FileId(7)), "");
        assert!(matches!(t.tags(FileId(9), Needs::default()), TagOutcome::Vanished));
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
        assert_eq!(t.relative_to(FileId(1), &dot).as_deref(), Some(&b"ab/c"[..]));
        let dot_slash = RepoPath::new(b"./a").unwrap();
        assert_eq!(t.relative_to(FileId(3), &dot_slash).as_deref(), Some(&b"b/c"[..]));
    }

    #[test]
    fn trusted_modes_match_duplicates_by_path() {
        let mut t = table(&["x.sh", "a.py", "x.sh", "gone"]);
        t.set_trusted_modes(vec![(&b"x.sh"[..], 0o100_755), (&b"a.py"[..], 0o100_644), (&b"zzz"[..], 0o100_644)]);
        let n = Needs { stat: true, ..Needs::default() };
        for id in [0, 2] {
            let TagOutcome::Tags(tags) = t.tags(FileId(id), n) else { panic!() };
            assert!(tags.contains("executable") && tags.contains("shell"), "{tags}");
        }
        let TagOutcome::Tags(tags) = t.tags(FileId(1), n) else { panic!() };
        assert!(tags.contains("non-executable") && tags.contains("python"));
        // No mode and no file under the (nonexistent) root: gone.
        assert!(matches!(t.tags(FileId(3), n), TagOutcome::Vanished));
        // A mode that names no file kind is ignored.
        let mut t = table(&["q"]);
        t.set_trusted_modes(vec![(&b"q"[..], 0o777)]);
        assert!(matches!(t.tags(FileId(0), n), TagOutcome::Vanished));
    }

    #[test]
    fn retain_lexists_keeps_trusted_and_compacts() {
        let mut t = table(&["keep.py", "gone.py", "keep2.py"]);
        t.set_trusted_modes(vec![(&b"keep.py"[..], 0o100_644), (&b"keep2.py"[..], 0o100_755)]);
        let _ = t.tags(FileId(2), Needs::default());
        t.retain_lexists();
        assert_eq!(t.len(), 2);
        assert_eq!(t.bytes(FileId(1)), b"keep2.py");
        let TagOutcome::Tags(tags) = t.tags(FileId(1), Needs { stat: true, ..Needs::default() }) else {
            panic!()
        };
        assert!(tags.contains("executable"));
        let TagOutcome::Tags(tags) = t.tags(FileId(1), Needs::default()) else { panic!() };
        assert_eq!(tags, file_identify::tags_from_filename("keep2.py"));
    }
}
