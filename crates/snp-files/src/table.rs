//! The file table: byte arena + spans, git modes, tier columns allocated on first need (§2.3, ADR-079).
//!
//! pre-commit keeps the run's file names as a list of `str` and memoises `identify.tags_from_path` per name with
//! `functools.cache` (`commands/run.py:73-80`). snp keeps the names as one byte arena (git's `-z` output) and
//! memoises identification per file in struct-of-arrays columns, one per tier, each allocated only when a filter
//! first needs it (ADR-041):
//!
//! * **name** — `tags_from_filename` (no I/O), interned: two bytes per file;
//! * **meta** — one `lstat` (or a trusted git index mode) kept as its kind and executable bit alone, for filters that
//!   reference only `file`/`directory`/`symlink`/`socket`/`executable`/`non-executable` (ADR-110): no name tags,
//!   eight bytes per file; a later stat tier of the file is built from it, never from a second `lstat` (not used when
//!   trusted index modes serve a run with shebang filters, whose stat tier must classify the name first);
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
//! A clean `--all-files` run can trust the index modes instead (ADR-106): the pipeline proves with an empty, clean
//! `git diff` that every unflagged entry exists with its index kind, hands the modes over with a flag the diff
//! oracle clears after every hook that may have changed the worktree, and the stat tier of a trusted entry is then
//! built from its mode with zero syscalls. Such a cell is marked: its kind is exact, its executable bit is not (git
//! records the owner bit; identify emulates `os.access`), so a query whose answer depends on that bit takes an
//! `lstat` instead.
//!
//! Owner: WP-22.

use std::borrow::Cow;
use std::collections::{HashMap, TryReserveError};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use file_identify::{Classification, ExecPolicy, FileKind, Meta, Pending, Tag, TagSet};
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

/// The tags identify takes from the `lstat` result alone (kind and executable bit), never from a name, a shebang or
/// the content (ADR-110).
pub(crate) const META_TAGS: TagSet = TagSet::from_tag(Tag::FILE)
    .with(Tag::DIRECTORY)
    .with(Tag::SYMLINK)
    .with(Tag::SOCKET)
    .with(Tag::EXECUTABLE)
    .with(Tag::NON_EXECUTABLE);

/// A filter's identification query: its [`Needs`] plus what the tags it references allow (ADR-106, ADR-110). Kept
/// crate-private because `Needs` is a published struct with public fields: a field there would break every struct
/// literal of it, while the public [`FileTable::tags`] keeps asking for full identification.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Query {
    /// The tiers.
    pub(crate) needs: Needs,
    /// The filter references `executable` or `non-executable`: the executable bit is never taken from a trusted index
    /// mode (ADR-106).
    pub(crate) exec: bool,
    /// Every referenced tag is in [`META_TAGS`] and no content tier is needed: the `lstat` meta alone decides the
    /// filter (ADR-110), so the stat tier (name tags) is not built. [`FileTable::query`] clears it in a run whose
    /// trusted index modes serve a filter that needs shebang tags (ADR-106 decision (4)).
    pub(crate) meta_only: bool,
}

impl Query {
    /// The query of a filter that needs `needs` and references `referenced` (`types ∪ types_or ∪ exclude_types`).
    pub(crate) const fn new(needs: Needs, referenced: TagSet) -> Self {
        Self {
            needs,
            exec: referenced.intersects(&EXEC_TAGS),
            meta_only: needs.stat
                && !needs.encoding
                && !needs.shebang
                && referenced.is_subset(&META_TAGS),
        }
    }

    /// Full identification of the tiers `needs` (the public [`FileTable::tags`]).
    pub(crate) const fn full(needs: Needs) -> Self {
        Self {
            needs,
            exec: false,
            meta_only: false,
        }
    }
}

/// identify's meta tags of an entry with meta `m`: `classify(m, name).tags() ∩ META_TAGS` for every name, with snp's
/// mapping of `Other` (FIFOs, devices) to a regular file (ADR-110). `None` for a kind this version does not know: the
/// caller then classifies in full (fail closed).
pub(crate) const fn meta_tags(m: Meta) -> Option<TagSet> {
    match m.kind() {
        FileKind::Directory => Some(TagSet::from_tag(Tag::DIRECTORY)),
        FileKind::Symlink => Some(TagSet::from_tag(Tag::SYMLINK)),
        FileKind::Socket => Some(TagSet::from_tag(Tag::SOCKET)),
        FileKind::Regular | FileKind::Other => {
            Some(TagSet::from_tag(Tag::FILE).with(if m.is_executable() {
                Tag::EXECUTABLE
            } else {
                Tag::NON_EXECUTABLE
            }))
        }
        _ => None,
    }
}

/// The meta tier of one file (ADR-110): the `lstat` result alone, for queries whose filters reference only meta tags.
/// The stat tier of a file with a meta cell is built from it, never from a second `lstat`.
#[derive(Clone, Copy, Debug)]
enum MetaTier {
    /// `lstat` (or an adopted `lexists` meta) succeeded: kind and executable bit.
    Present(Meta),
    /// From a trusted index mode (ADR-106): the kind is exact, the executable bit is not.
    Trusted(Meta),
    /// `lstat` failed.
    Vanished,
}

/// The stat tier of one file.
#[derive(Clone, Copy, Debug)]
pub(crate) enum StatTier {
    /// `lstat` (or the trusted git mode of [`FileTable::set_trusted_modes`]) succeeded: kind + mode + name tags, and
    /// what content is still needed.
    Present(Classification),
    /// Built from a trusted index mode (ADR-106): the kind and the name tags are exact, the executable bit is not
    /// verified (the index records the owner bit, identify emulates `os.access`). Only queries that cannot observe
    /// that bit use it; any other takes the exact tier ([`FileTable::exact_tags`]).
    Trusted(Classification),
    /// `lstat` failed (every `OSError` counts, as in identify: X13-CLS-01/04).
    Vanished,
}

/// The trusted index modes of a clean `--all-files` run (ADR-106).
#[derive(Debug)]
struct IndexTrust {
    /// Per entry: the mode a clean `git diff` vouches for (`100644`, `100755`, `120000`), 0 = none.
    modes: Box<[u32]>,
    /// True while the modes describe the worktree; the diff oracle clears it after every hook that may have
    /// changed the worktree. Cells already built stay (pre-commit memoises the first identification).
    valid: Arc<AtomicBool>,
    /// Some filter of the run needs shebang tags: a file without name tags gets no trusted cell, since its
    /// executable bit decides whether identify reads its shebang.
    shebang: bool,
}

/// The tags whose presence depends on the executable bit alone.
const EXEC_TAGS: TagSet = TagSet::from_tag(Tag::EXECUTABLE).with(Tag::NON_EXECUTABLE);

/// The tags a regular file's classification holds besides its name tags.
const KIND_TAGS: TagSet = EXEC_TAGS.with(Tag::FILE);

/// Whether a regular file's classification carries no name tags (`file_identify::classify`: a regular file's tags
/// are `file`, its mode tag and its name tags, and only name tags give `text`/`binary` before the content is read,
/// so an empty name-tag set always leaves content to read). Then its executable bit decides whether the shebang is
/// read.
#[inline]
fn no_name_tags(class: &Classification) -> bool {
    matches!(class, Classification::NeedsContent(p) if p.tags().difference(KIND_TAGS).is_empty())
}

/// Whether query `q` can observe the executable bit of `class`: through the `executable`/`non-executable` tags its
/// filter references, or through the shebang of a file without name tags.
#[inline]
fn observes_exec(class: &Classification, q: Query) -> bool {
    q.exec || (q.needs.shebang && no_name_tags(class))
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
    /// Meta tier (ADR-110): the `lstat` meta of files first queried by filters that reference only meta tags.
    meta: Column<MetaTier>,
    /// Stat tier.
    stat: Column<StatTier>,
    /// Head tier.
    head: Column<Head>,
    /// Matching view of names that are not clean UTF-8 (or contain `\` on Windows).
    view: Column<Box<str>>,
    /// `lexists` results of the newest `Classifier::new`, adopted as the stat tier of the first hook's candidates.
    pending: Mutex<Option<Box<[Option<Meta>]>>>,
    /// Trusted index modes of a clean `--all-files` run (ADR-106).
    index: Option<IndexTrust>,
    /// The exact stat tier of an entry whose `stat` cell is [`StatTier::Trusted`], for a query that observes the
    /// executable bit (allocated only then, which a run that declared its filters to the trust never does).
    exact_stat: Column<StatTier>,
    /// The head tier computed from [`Self::exact_stat`].
    exact_head: Column<Head>,
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
            .field("index_trust", &self.index.is_some())
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
            meta: Column::new(),
            stat: Column::new(),
            head: Column::new(),
            view: Column::new(),
            pending: Mutex::new(None),
            index: None,
            exact_stat: Column::new(),
            exact_head: Column::new(),
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

    /// Whether hooks whose filters reference `referenced` (the union of every selected hook's `types`, `types_or`
    /// and `exclude_types`) can classify from trusted index modes (ADR-106): on Unix, and only when none of them
    /// references `executable` or `non-executable` (the executable bit is never taken from an index mode).
    pub fn index_trust_possible(referenced: TagSet) -> bool {
        cfg!(unix) && !referenced.intersects(&EXEC_TAGS)
    }

    /// Trusted index modes for a clean `--all-files` run (ADR-106).
    ///
    /// `modes[i]` is entry `i`'s index mode from the same listing the table was built from
    /// (`snp_git::Git::all_files_tagged`, whose paths are the table's), so no path comparison is needed; a length
    /// mismatch (a span `from_zbuf` dropped) refuses the trust. The caller has proved that `git diff` was empty and
    /// clean, that no entry is flagged or unmerged, and that the configuration lets git compare kinds (Unix,
    /// `core.fileMode`, `core.symlinks`, no fsmonitor, no sparse index); `valid` is cleared by the diff oracle after
    /// every hook that may have changed the worktree, and from then on every new cell takes an `lstat`.
    /// `referenced` is the union of the tags every filter of the run references ([`Self::index_trust_possible`]
    /// must hold for it). Only modes naming a regular file or a symlink are kept: a gitlink, a directory or the
    /// empty blob (mode 0 from `all_files_tagged`) keeps its `lstat`. Returns whether the trust was set.
    pub fn trust_index_modes(
        &mut self,
        modes: Box<[u32]>,
        valid: Arc<AtomicBool>,
        referenced: TagSet,
    ) -> bool {
        if !Self::index_trust_possible(referenced) || modes.len() != self.len() {
            return false;
        }
        let mut modes = modes;
        for m in &mut modes {
            if !matches!(*m, 0o100_644 | 0o100_755 | 0o120_000) {
                *m = 0;
            }
        }
        self.index = Some(IndexTrust {
            modes,
            valid,
            shebang: crate::filter::needs_for(referenced).shebang,
        });
        true
    }

    /// Once, before any hook (02 §10).
    ///
    /// `Classifier.__init__`'s `[f for f in filenames if os.path.lexists(f)]` (`run.py:73-74`): drops every file
    /// whose `lstat` fails (deleted files, nonexistent `--files` entries); broken symlinks stay. Files with a trusted
    /// git mode need no syscall. Order and duplicates are kept.
    pub fn retain_lexists(&mut self) {
        let n = self.len();
        let keep: Vec<bool> = parallel_flags(n, |i| {
            self.index_exists(i) || self.lexists_meta(i).is_some()
        });
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
        if let Some(t) = self.index.as_mut() {
            t.modes = t
                .modes
                .iter()
                .zip(&keep)
                .filter_map(|(m, &k)| k.then_some(*m))
                .collect();
        }
        self.meta.retain(&keep);
        self.stat.retain(&keep);
        self.head.retain(&keep);
        self.view.retain(&keep);
        self.exact_stat.retain(&keep);
        self.exact_head.retain(&keep);
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
    ///
    /// With trusted index modes (ADR-106) the `executable`/`non-executable` tags of a trusted file come from its index
    /// mode; the classifier, whose filters may reference them, takes those tags from an `lstat` instead. When a
    /// classifier filter that references only meta tags identified the file first (ADR-110), its stat tier is built
    /// from that observation rather than a second `lstat`.
    pub fn tags(&self, id: FileId, n: Needs) -> TagOutcome<'_> {
        self.tags_for(id, Query::full(n))
    }

    /// The query of filter `f` on this table: [`HookFilter::query`](crate::HookFilter), kept off the meta tier
    /// (ADR-110) when trusted index modes serve a run with a filter that needs shebang tags (ADR-106 decision (4)).
    /// There a file whose name gives no tags gets no trusted cell, and only classifying the name tells which files
    /// those are, so a meta-only filter takes the stat tier as it would without ADR-110: such a file takes its
    /// `lstat` at its first identification, never after a later hook changed it. The trust is fixed before the
    /// classifier shares the table, so one decision per hook holds for every file.
    pub(crate) fn query(&self, f: &crate::HookFilter) -> Query {
        let q = f.query();
        let shebang_trust = self.index.as_ref().is_some_and(|t| t.shebang);
        Query {
            meta_only: q.meta_only && !shebang_trust,
            ..q
        }
    }

    /// [`Self::tags`] for a filter's query. When the filter references `executable` or `non-executable` the
    /// executable bit is never taken from a trusted index mode (ADR-106); neither is it for a query that needs shebang
    /// tags of a file without name tags (the bit decides whether identify reads the shebang). A meta-only query
    /// (ADR-110, from [`Self::query`]) of a file without a stat tier is answered from the meta tier: its meta tags,
    /// which decide its filter exactly as full identification would, since the filter references no other tag.
    pub(crate) fn tags_for(&self, id: FileId, q: Query) -> TagOutcome<'_> {
        let Some(i) = self.index(id) else {
            return TagOutcome::Vanished;
        };
        if self.known_vanished(id) {
            return TagOutcome::Vanished;
        }
        let n = q.needs;
        if !n.any() {
            return TagOutcome::Tags(self.name_tags(i));
        }
        if q.meta_only
            && self.stat.peek(i).is_none()
            && let Some(outcome) = self.meta_outcome(i, q)
        {
            return outcome;
        }
        let class = match self.stat_tier(i, !q.exec) {
            StatTier::Vanished => return TagOutcome::Vanished,
            StatTier::Trusted(c) if observes_exec(&c, q) => {
                return self.exact_tags(i, n);
            }
            StatTier::Present(c) | StatTier::Trusted(c) => c,
        };
        self.outcome(i, class, n, &self.head)
    }

    /// The answer of the meta tier to meta-only query `q` for entry `i` (ADR-110): its meta tags, `Vanished`, or the
    /// `lstat` answer when `q` observes the executable bit of a meta taken from the index (ADR-106). `None` for a kind
    /// this version cannot tag from the meta: the caller builds the stat tier from the same meta (fail closed).
    #[inline]
    fn meta_outcome(&self, i: usize, q: Query) -> Option<TagOutcome<'_>> {
        match self.meta_tier(i, !q.exec) {
            MetaTier::Vanished => Some(TagOutcome::Vanished),
            MetaTier::Trusted(_) if q.exec => Some(self.exact_tags(i, q.needs)),
            MetaTier::Present(m) | MetaTier::Trusted(m) => meta_tags(m).map(TagOutcome::Tags),
        }
    }

    /// The outcome of a classification for `n`: its tags, or the head tier memoised in `heads` when `n` asks for a
    /// category the stat tier left open.
    #[inline]
    fn outcome<'t>(
        &'t self,
        i: usize,
        class: Classification,
        n: Needs,
        heads: &'t Column<Head>,
    ) -> TagOutcome<'t> {
        match class {
            Classification::Done(tags) => TagOutcome::Tags(tags),
            Classification::NeedsContent(p) => {
                let need = (n.encoding && p.needs_encoding()) || (n.shebang && p.needs_shebang());
                if !need {
                    return TagOutcome::Tags(p.tags());
                }
                let len = self.len();
                match heads
                    .cell(len, i)
                    .map(|c| c.get_or_init(|| self.read_head(i, p)))
                {
                    Some(Head::Tags(t)) => TagOutcome::Tags(*t),
                    Some(Head::Unreadable(e)) => TagOutcome::Unreadable(e),
                    Some(Head::Vanished) | None => TagOutcome::Vanished,
                }
            }
        }
    }

    /// The outcome from an `lstat` of entry `i`, for a query that observes the executable bit of a trusted entry
    /// (ADR-106): its own memoised stat and head cells, so the trusted cells stay as they are for every other query.
    /// A run that declared its filters to [`Self::trust_index_modes`] never gets here.
    fn exact_tags(&self, i: usize, n: Needs) -> TagOutcome<'_> {
        let len = self.len();
        let compute = || self.lstat_tier(i);
        match self
            .exact_stat
            .cell(len, i)
            .map(|c| *c.get_or_init(compute))
        {
            Some(StatTier::Present(class) | StatTier::Trusted(class)) => {
                self.outcome(i, class, n, &self.exact_head)
            }
            Some(StatTier::Vanished) | None => TagOutcome::Vanished,
        }
    }

    /// Whether `tags_for(id, q)` is answered from memoised cells alone (no `lstat`, `open` or name-tier
    /// computation): the file is known to be gone, or every tier `q` needs for it is computed (ADR-101; the meta tier
    /// for a meta-only query, ADR-110). A pending `lexists` meta does not count: adopting it still writes a cell.
    pub(crate) fn tiers_ready(&self, id: FileId, q: Query) -> bool {
        let Some(i) = self.index(id) else {
            return true;
        };
        if self.known_vanished(id) {
            return true;
        }
        let n = q.needs;
        if !n.any() {
            return self
                .name
                .get()
                .and_then(|ids| ids.get(i))
                .is_some_and(|c| c.load(Ordering::Acquire) != 0);
        }
        if q.meta_only && self.stat.peek(i).is_none() {
            return match self.meta.peek(i) {
                None => false,
                Some(MetaTier::Vanished) => true,
                Some(MetaTier::Trusted(_)) if q.exec => false,
                Some(MetaTier::Present(m) | MetaTier::Trusted(m)) => meta_tags(*m).is_some(),
            };
        }
        match self.stat.peek(i) {
            None => false,
            Some(StatTier::Trusted(c)) if observes_exec(c, q) => false,
            Some(
                StatTier::Vanished
                | StatTier::Present(Classification::Done(_))
                | StatTier::Trusted(Classification::Done(_)),
            ) => true,
            Some(
                StatTier::Present(Classification::NeedsContent(p))
                | StatTier::Trusted(Classification::NeedsContent(p)),
            ) => {
                let need = (n.encoding && p.needs_encoding()) || (n.shebang && p.needs_shebang());
                !need || self.head.peek(i).is_some()
            }
        }
    }

    /// Whether entry `id` is already known to be gone (a computed stat or head tier said so, or, before any stat tier,
    /// the meta tier); no I/O. A stat tier, once computed, decides: it is built from the meta tier when there is one.
    pub(crate) fn known_vanished(&self, id: FileId) -> bool {
        let Some(i) = self.index(id) else {
            return true;
        };
        match self.stat.peek(i) {
            Some(StatTier::Vanished) => true,
            Some(StatTier::Present(_) | StatTier::Trusted(_)) => {
                matches!(self.head.peek(i), Some(Head::Vanished))
            }
            None => matches!(self.meta.peek(i), Some(MetaTier::Vanished)),
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

    /// The trusted index mode of entry `i` while the trust holds (ADR-106), as a meta whose executable bit is the
    /// index's owner bit; `None` without trust, after the diff oracle cleared it, or for an untrusted entry.
    fn index_meta(&self, i: usize) -> Option<Meta> {
        let t = self.index.as_ref()?;
        if !t.valid.load(Ordering::Acquire) || self.bytes_at(i).is_empty() {
            return None;
        }
        let mode = *t.modes.get(i).filter(|&&m| m != 0)?;
        Meta::from_git_mode_with(mode, ExecPolicy::PythonCompat)
    }

    /// `os.path.lexists` answered by a trusted index mode (ADR-106): a clean, empty `git diff` saw the entry with
    /// its index kind, and no hook has run since. No syscall.
    pub(crate) fn index_exists(&self, i: usize) -> bool {
        self.index_meta(i).is_some()
    }

    /// The trusted stat tier of entry `i` (ADR-106), unless a filter of the run needs the shebang of a file whose
    /// name gives no tags: its executable bit decides whether the shebang is read, so it keeps its `lstat`.
    fn trusted_tier(&self, i: usize) -> Option<StatTier> {
        self.trusted_class(i, self.index_meta(i)?)
    }

    /// [`Self::trusted_tier`] from the trusted `meta` of entry `i` (a meta tier taken from the index, ADR-110).
    fn trusted_class(&self, i: usize, meta: Meta) -> Option<StatTier> {
        let class = self.classify(i, meta);
        let shebang = self.index.as_ref().is_some_and(|t| t.shebang);
        (!(shebang && no_name_tags(&class))).then_some(StatTier::Trusted(class))
    }

    /// The stat tier from one `lstat` of entry `i`.
    fn lstat_tier(&self, i: usize) -> StatTier {
        match self.lexists_meta(i) {
            Some(meta) => StatTier::Present(self.classify(i, meta)),
            None => StatTier::Vanished,
        }
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

    /// The memoised stat tier of entry `i`. Built from the meta tier when a meta-only query computed one first
    /// (ADR-110: the same observation, never a second `lstat`; a meta taken from a trusted index mode stays trusted,
    /// as a stat tier built at that moment would have); else the trusted index mode while the trust holds (ADR-106)
    /// and `trust` allows it (the first query is not one that observes the executable bit); else one `lstat`.
    fn stat_tier(&self, i: usize, trust: bool) -> StatTier {
        let n = self.len();
        let compute = || {
            if self.meta.0.get().is_some()
                && let Some(tier) = self.stat_from_meta(i)
            {
                return tier;
            }
            trust
                .then(|| self.trusted_tier(i))
                .flatten()
                .unwrap_or_else(|| self.lstat_tier(i))
        };
        match self.stat.cell(n, i) {
            Some(c) => *c.get_or_init(compute),
            None => StatTier::Vanished,
        }
    }

    /// The stat tier of entry `i` built from its meta tier, if it has one (ADR-110).
    #[inline(never)]
    fn stat_from_meta(&self, i: usize) -> Option<StatTier> {
        Some(match self.meta.peek(i)? {
            MetaTier::Present(meta) => StatTier::Present(self.classify(i, *meta)),
            // `trusted_class` refuses only in a run with shebang filters, whose queries never take the meta tier
            // (`Self::query`), so the `lstat` is unreachable; kept so that a broken invariant costs a late `lstat`,
            // never a trusted cell ADR-106 decision (4) forbids.
            MetaTier::Trusted(meta) => self
                .trusted_class(i, *meta)
                .unwrap_or_else(|| self.lstat_tier(i)),
            MetaTier::Vanished => StatTier::Vanished,
        })
    }

    /// The memoised meta tier of entry `i` (ADR-110): the trusted index mode while the trust holds and `trust` allows
    /// it (the query does not observe the executable bit), else one `lstat`. Never built in a run whose trusted
    /// modes serve a filter that needs shebang tags ([`Self::query`]).
    fn meta_tier(&self, i: usize, trust: bool) -> MetaTier {
        let n = self.len();
        let compute = || match trust.then(|| self.index_meta(i)).flatten() {
            Some(meta) => MetaTier::Trusted(meta),
            None => self
                .lexists_meta(i)
                .map_or(MetaTier::Vanished, MetaTier::Present),
        };
        match self.meta.cell(n, i) {
            Some(c) => *c.get_or_init(compute),
            None => MetaTier::Vanished,
        }
    }

    /// Records the `lexists` metas of a fresh `Classifier::new` (index-aligned with the table), replacing older ones;
    /// `None` when no kept file has one (every kept file was trusted, ADR-106).
    pub(crate) fn set_pending(&self, metas: Option<Box<[Option<Meta>]>>) {
        *snp_core::sync::lock(&self.pending) = metas;
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

    /// Records the pending `lexists` meta of `id` as the first identification `q` needs, when it has one and neither
    /// the meta nor the stat tier is computed: the meta tier for a meta-only query (ADR-110), else the stat tier.
    #[inline]
    pub(crate) fn adopt(&self, id: FileId, pending: &[Option<Meta>], q: Query) {
        let Some(i) = self.index(id) else { return };
        let Some(Some(meta)) = pending.get(i) else {
            return;
        };
        if q.meta_only {
            self.adopt_meta(i, *meta);
        } else if let Some(c) = self.stat.cell(self.len(), i)
            && c.get().is_none()
            && self.meta.peek(i).is_none()
        {
            let _set: Result<(), StatTier> = c.set(StatTier::Present(self.classify(i, *meta)));
        }
    }

    /// [`Self::adopt`] for a meta-only query: the meta tier of entry `i`, unless its stat tier is computed (out of
    /// line, so the stat-tier branch stays as small as it was).
    #[inline(never)]
    fn adopt_meta(&self, i: usize, meta: Meta) {
        if self.stat.peek(i).is_none()
            && let Some(c) = self.meta.cell(self.len(), i)
        {
            let _set: Result<(), MetaTier> = c.set(MetaTier::Present(meta));
        }
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
    concat_ordered(parts)
}

/// The partitions' results concatenated in index order (ADR-110): the first error in index order wins, before
/// anything is reserved; otherwise the first partition's vector is reused and grown once to the total length (a
/// failed reservation only means ordinary growth).
fn concat_ordered<T>(parts: Vec<Result<Vec<T>, Failure>>) -> Result<Vec<T>, Failure> {
    let parts: Vec<Vec<T>> = parts.into_iter().collect::<Result<_, _>>()?;
    let total = parts
        .iter()
        .fold(0_usize, |sum, part| sum.saturating_add(part.len()));
    let mut parts = parts.into_iter();
    let Some(mut out) = parts.next() else {
        return Ok(Vec::new());
    };
    let _reserved: Result<(), TryReserveError> =
        out.try_reserve_exact(total.saturating_sub(out.len()));
    for part in parts {
        out.extend(part);
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

    fn tags_of(names: &[&str]) -> TagSet {
        TagSet::from_names(names).unwrap()
    }

    /// The query of a filter that references `executable` and needs `needs` (not meta-only).
    #[cfg(unix)]
    fn exec_query(needs: Needs) -> Query {
        Query {
            needs,
            exec: true,
            meta_only: false,
        }
    }

    #[cfg(unix)]
    fn stat_only() -> Needs {
        Needs {
            stat: true,
            ..Needs::default()
        }
    }

    /// ADR-106: trusted index modes answer existence and the stat tier with zero syscalls (the root does not even
    /// exist) while the flag holds; gitlinks, directories and the empty blob (mode 0) keep their `lstat`; once the
    /// diff oracle clears the flag every new cell takes an `lstat`, and the cells already built stay.
    #[cfg(unix)]
    #[test]
    fn index_trust_needs_no_syscalls_until_cleared() {
        let mut t = table(&["a.py", "x.sh", "link", "sub", "empty", "dir", "b.py", ""]);
        let valid = Arc::new(AtomicBool::new(true));
        let modes: Box<[u32]> = Box::new([
            0o100_644, 0o100_755, 0o120_000, 0o160_000, 0, 0o040_000, 0o100_644, 0o100_644,
        ]);
        assert!(t.trust_index_modes(modes, Arc::clone(&valid), tags_of(&["file"])));
        let exists: Vec<bool> = (0..8).map(|i| t.index_exists(i)).collect();
        assert_eq!(
            exists,
            [true, true, true, false, false, false, true, false],
            "an empty name never exists"
        );
        let TagOutcome::Tags(tags) = t.tags(FileId(0), stat_only()) else {
            panic!("a trusted file needs no lstat")
        };
        assert_eq!(tags, tags_of(&["file", "non-executable", "python", "text"]));
        let TagOutcome::Tags(tags) = t.tags(FileId(2), stat_only()) else {
            panic!()
        };
        assert_eq!(tags, tags_of(&["symlink"]));
        assert!(matches!(t.stat.peek(0), Some(StatTier::Trusted(_))));
        // gitlinks and the rest take an lstat: gone under the nonexistent root
        assert!(matches!(
            t.tags(FileId(3), stat_only()),
            TagOutcome::Vanished
        ));
        assert!(matches!(
            t.tags(FileId(4), stat_only()),
            TagOutcome::Vanished
        ));
        // the diff oracle clears the flag: a new cell takes an lstat, a built one stays
        valid.store(false, Ordering::Release);
        assert!(!t.index_exists(6));
        assert!(matches!(
            t.tags(FileId(6), stat_only()),
            TagOutcome::Vanished
        ));
        assert!(matches!(
            t.tags(FileId(0), stat_only()),
            TagOutcome::Tags(_)
        ));
        // retain_lexists compacts the trusted modes with the spans
        let mut t = table(&["keep.py", "gone.py", "keep2.py"]);
        let modes: Box<[u32]> = Box::new([0o100_644, 0, 0o100_644]);
        assert!(t.trust_index_modes(modes, Arc::new(AtomicBool::new(true)), TagSet::EMPTY));
        t.retain_lexists();
        assert_eq!(t.len(), 2);
        assert_eq!(t.bytes(FileId(1)), b"keep2.py");
        assert!(t.index_exists(1));
    }

    /// The trust is refused for filters that reference the executable bit, for a mode column of the wrong length,
    /// and off Unix.
    #[test]
    fn index_trust_is_refused_when_it_cannot_be_exact() {
        let valid = Arc::new(AtomicBool::new(true));
        let mut t = table(&["a.py", "b.py"]);
        for exec in ["executable", "non-executable"] {
            assert!(!FileTable::index_trust_possible(tags_of(&["file", exec])));
            assert!(!t.trust_index_modes(
                Box::new([0o100_644, 0o100_644]),
                Arc::clone(&valid),
                tags_of(&[exec])
            ));
        }
        assert!(!t.trust_index_modes(Box::new([0o100_644]), Arc::clone(&valid), TagSet::EMPTY));
        assert!(t.index.is_none());
        assert_eq!(
            t.trust_index_modes(Box::new([0o100_644, 0o100_644]), valid, TagSet::EMPTY),
            cfg!(unix)
        );
    }

    /// ADR-106: the executable bit never comes from an index mode. A filter that references `executable` or
    /// `non-executable` gets the `lstat` answer for a trusted file (`exact_tags`), and a file without name tags gets
    /// no trusted cell when a filter of the run needs shebang tags, so its shebang follows identify's `os.access`
    /// (any `x` bit for root, the owner bit for the owner). The index here claims 100755 for a 0o654 file.
    #[cfg(unix)]
    #[test]
    fn index_trust_never_supplies_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        for name in ["tool", "t.py"] {
            std::fs::write(d.path().join(name), b"#!/usr/bin/env python3\nprint(1)\n").unwrap();
            std::fs::set_permissions(d.path().join(name), std::fs::Permissions::from_mode(0o654))
                .unwrap();
        }
        let names = ["tool", "t.py"];
        let fresh = || FileTable::from_args(names.iter().map(OsString::from).collect(), d.path());
        let all = Needs {
            stat: true,
            encoding: true,
            shebang: true,
        };
        let truth = fresh();
        let want = |id: u32, n: Needs| match truth.tags(FileId(id), n) {
            TagOutcome::Tags(t) => t,
            _ => panic!("readable"),
        };
        let lied: Box<[u32]> = Box::new([0o100_755, 0o100_755]);
        // a run with shebang filters: `tool` (no name tags) keeps its lstat; `t.py` is trusted
        let mut t = fresh();
        assert!(t.trust_index_modes(
            lied.clone(),
            Arc::new(AtomicBool::new(true)),
            tags_of(&["python"])
        ));
        assert!(matches!(t.tags(FileId(0), all), TagOutcome::Tags(x) if x == want(0, all)));
        assert!(matches!(t.stat.peek(0), Some(StatTier::Present(_))));
        assert!(matches!(t.stat_tier(1, true), StatTier::Trusted(_)));
        // a filter on `executable`: the lstat answer, not the index's owner bit
        assert!(
            matches!(t.tags_for(FileId(1), exec_query(stat_only())), TagOutcome::Tags(x) if x == want(1, stat_only()))
        );
        // a run declared without shebang filters that asks for a shebang anyway: still the lstat answer
        let mut t = fresh();
        assert!(t.trust_index_modes(lied, Arc::new(AtomicBool::new(true)), tags_of(&["file"])));
        assert!(matches!(t.stat_tier(0, true), StatTier::Trusted(_)));
        assert!(!t.tiers_ready(FileId(0), Query::full(all)));
        assert!(matches!(t.tags(FileId(0), all), TagOutcome::Tags(x) if x == want(0, all)));
        assert!(
            t.exact_stat.0.get().is_some(),
            "answered from the exact cells"
        );
    }

    /// `no_name_tags` reads exactly `tags_from_filename(name).is_empty()` off a regular file's classification.
    #[test]
    fn no_name_tags_matches_tags_from_filename() {
        for name in [
            "tool",
            "a.py",
            "Makefile",
            "x.unknownext",
            "README",
            "LICENSE",
            "a.svg",
            "a.plist",
            ".bashrc",
            "dir/file",
            "a.tar.gz",
            "a.PY",
        ] {
            for exec in [false, true] {
                let class = file_identify::classify(Meta::new(FileKind::Regular, exec), name);
                assert_eq!(
                    no_name_tags(&class),
                    file_identify::tags_from_filename(name).is_empty(),
                    "{name} exec={exec}"
                );
            }
        }
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

    /// The filter of `types` (no pattern, no prefix).
    #[cfg(unix)]
    fn types_filter(types: &[&str], cache: &snp_pyre::RegexCache) -> crate::HookFilter {
        crate::HookFilter::compile(
            "",
            "^$",
            tags_of(types),
            TagSet::EMPTY,
            TagSet::EMPTY,
            None,
            cache,
        )
        .unwrap()
    }

    /// A classifier over `table` with the default top-level patterns.
    #[cfg(unix)]
    fn classifier_of(
        table: &Arc<FileTable>,
        cache: &snp_pyre::RegexCache,
        pool: &JobPool,
    ) -> crate::Classifier {
        let all = cache.get("", snp_pyre::Options::CONFIG).unwrap();
        let none = cache.get("^$", snp_pyre::Options::CONFIG).unwrap();
        crate::Classifier::new(Arc::clone(table), &all, &none, pool).unwrap()
    }

    /// ADR-110: filters that reference only meta tags never build the stat tier (no name tags, no 64-byte cells),
    /// adopted `lexists` metas included; a later filter that needs name tags builds it from the memoised meta, never
    /// a second `lstat` (a file deleted in between keeps its first identification, as pre-commit's memo does).
    #[cfg(unix)]
    #[test]
    fn meta_only_filters_never_build_the_stat_tier() {
        let d = tempfile::tempdir().unwrap();
        for name in ["a.py", "b.unknownext", "c.sh"] {
            std::fs::write(d.path().join(name), b"x\n").unwrap();
        }
        std::os::unix::fs::symlink("a.py", d.path().join("link.py")).unwrap();
        std::fs::create_dir_all(d.path().join("dir.py")).unwrap();
        let names = [
            "a.py",
            "b.unknownext",
            "c.sh",
            "link.py",
            "dir.py",
            "gone.py",
        ];
        let table = Arc::new(FileTable::from_args(
            names.iter().map(OsString::from).collect(),
            d.path(),
        ));
        let cache = snp_pyre::RegexCache::new();
        let pool = JobPool::new(
            NonZeroUsize::new(2).unwrap(),
            snp_core::cancel::CancelToken::new(),
        );
        let c = classifier_of(&table, &cache, &pool);
        assert_eq!(c.kept.len(), 5, "gone.py does not exist");
        let files = c.for_hook(&types_filter(&["file"], &cache), &pool).unwrap();
        assert_eq!(files, [FileId(0), FileId(1), FileId(2)]);
        let links = c
            .for_hook(&types_filter(&["symlink", "directory"], &cache), &pool)
            .unwrap();
        assert!(links.is_empty(), "types is an intersection: {links:?}");
        let not_links = c
            .for_hook(
                &crate::HookFilter::compile(
                    "",
                    "^$",
                    TagSet::EMPTY,
                    tags_of(&["symlink", "directory"]),
                    tags_of(&["executable"]),
                    None,
                    &cache,
                )
                .unwrap(),
                &pool,
            )
            .unwrap();
        assert_eq!(not_links, [FileId(3), FileId(4)]);
        assert!(
            table.stat.0.get().is_none(),
            "a meta-only filter built the stat tier"
        );
        // A name-tag filter after a deletion: the stat tier comes from the memoised meta, not a fresh lstat.
        std::fs::remove_file(d.path().join("a.py")).unwrap();
        let py = c
            .for_hook(&types_filter(&["python"], &cache), &pool)
            .unwrap();
        assert_eq!(py, [FileId(0)], "link.py and dir.py are not python");
        assert!(table.stat.0.get().is_some());
    }

    /// Every kind file-identify knows today (`FileKind` is `#[non_exhaustive]`).
    const KINDS: [FileKind; 5] = [
        FileKind::Regular,
        FileKind::Directory,
        FileKind::Symlink,
        FileKind::Socket,
        FileKind::Other,
    ];

    /// The tags of a classification once its content is known (`content` stands for the file's head).
    fn completed(class: Classification, content: &[u8]) -> TagSet {
        match class {
            Classification::Done(t) => t,
            Classification::NeedsContent(p) => p.finish_bytes(content),
        }
    }

    proptest::proptest! {
        /// ADR-110: `meta_tags(m) == classify(m, name).tags() ∩ META_TAGS` for every kind, both executable bits, any
        /// name and any content, through file-identify's `classify` and through snp's (FIFOs and devices as regular
        /// files): no name, shebang or content tag is a meta tag.
        #[test]
        fn meta_tags_are_the_meta_part_of_full_identification(
            name in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..24),
            content in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..64),
            shebang in proptest::prelude::any::<bool>(),
        ) {
            let content = if shebang {
                [b"#!/usr/bin/env python3\n".as_slice(), &content].concat()
            } else {
                content
            };
            let t = FileTable::from_zbuf(name.clone(), vec![(0, u32::try_from(name.len()).unwrap())], Path::new("/r"));
            for kind in KINDS {
                for exec in [false, true] {
                    let m = Meta::new(kind, exec);
                    let want = meta_tags(m).unwrap();
                    for class in [file_identify::classify(m, &name), t.classify(0, m)] {
                        let initial = match class {
                            Classification::Done(t) => t,
                            Classification::NeedsContent(p) => p.tags(),
                        };
                        proptest::prop_assert_eq!(initial.intersection(META_TAGS), want);
                        proptest::prop_assert_eq!(completed(class, &content).intersection(META_TAGS), want);
                    }
                }
            }
        }
    }

    /// ADR-110: no name (`NAMES`, `EXTENSIONS`, `EXTENSIONS_NEED_BINARY_CHECK`) and no interpreter of identify's tables
    /// yields a meta tag, so `tags_from_filename` and the shebang tier never do.
    #[test]
    fn name_and_interpreter_tables_hold_no_meta_tag() {
        use file_identify::extensions::{EXTENSIONS, EXTENSIONS_NEED_BINARY_CHECK, NAMES};
        use file_identify::interpreters::INTERPRETERS;
        for (table, entries) in [
            ("NAMES", NAMES.iter().collect::<Vec<_>>()),
            ("EXTENSIONS", EXTENSIONS.iter().collect()),
            (
                "EXTENSIONS_NEED_BINARY_CHECK",
                EXTENSIONS_NEED_BINARY_CHECK.iter().collect(),
            ),
            ("INTERPRETERS", INTERPRETERS.iter().collect()),
        ] {
            assert!(!entries.is_empty(), "{table}");
            for (key, tags) in entries {
                assert!(!tags.intersects(&META_TAGS), "{table}[{key:?}] = {tags}");
            }
        }
        assert!(!crate::filter::needs_for(META_TAGS).shebang);
        assert_eq!(META_TAGS.len(), 6);
    }

    /// The query of a filter: meta-only exactly when every referenced tag is a meta tag and no content tier is
    /// needed; `exec` exactly when it references `executable` or `non-executable`.
    #[test]
    fn query_from_referenced_tags() {
        let q = |names: &[&str]| {
            let referenced = tags_of(names);
            Query::new(crate::filter::needs_for(referenced), referenced)
        };
        for (names, meta_only, exec) in [
            (&["file"][..], true, false),
            (&["symlink"], true, false),
            (&["directory", "socket"], true, false),
            (&["executable"], true, true),
            (&["file", "non-executable"], true, true),
            (&["file", "python"], false, false),
            (&["text"], false, false),
            (&["binary", "executable"], false, true),
            (&["yaml"], false, false),
            (&[], false, false),
        ] {
            let got = q(names);
            assert_eq!(
                (got.meta_only, got.exec),
                (meta_only, exec),
                "{names:?}: {got:?}"
            );
        }
        assert!(!Query::full(stat_needs()).meta_only);
        assert!(!Query::full(stat_needs()).exec);
        // a hand-built `Needs` that asks for content is never meta-only
        let content = Needs {
            stat: true,
            encoding: true,
            shebang: false,
        };
        assert!(!Query::new(content, tags_of(&["file"])).meta_only);
    }

    /// `Needs { stat }` alone.
    fn stat_needs() -> Needs {
        Needs {
            stat: true,
            ..Needs::default()
        }
    }

    /// The meta-only query of a filter that references `names`.
    #[cfg(unix)]
    fn meta_query(names: &[&str]) -> Query {
        let q = Query::new(stat_needs(), tags_of(names));
        assert!(q.meta_only, "{names:?}");
        q
    }

    /// ADR-110 differential on a real tree: for every kind of entry (regular files with and without name tags and
    /// executable bits, a shebang script, links, a broken link, a directory, a FIFO, a socket, an unreadable file and
    /// a missing name) a meta-only query gives the meta part of full identification, `Vanished` exactly where it
    /// does, and never `Unreadable` (the content is never read); the meta tier never builds the stat tier.
    #[cfg(unix)]
    #[test]
    fn meta_only_queries_agree_with_full_identification() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        for (name, mode, body) in [
            ("a.py", 0o644, &b"x\n"[..]),
            ("run.sh", 0o755, b"echo\n"),
            ("tool", 0o755, b"#!/usr/bin/env python3\n"),
            ("data.unknownext", 0o644, b"\0\x01"),
            ("secret.unknownext", 0o000, b"x"),
            ("r654", 0o654, b"x"),
        ] {
            std::fs::write(root.join(name), body).unwrap();
            std::fs::set_permissions(root.join(name), std::fs::Permissions::from_mode(mode))
                .unwrap();
        }
        std::os::unix::fs::symlink("a.py", root.join("link.py")).unwrap();
        std::os::unix::fs::symlink("missing", root.join("broken")).unwrap();
        std::fs::create_dir_all(root.join("dir")).unwrap();
        // mkfifo(1) is POSIX; rustix has no mknodat or O_PATH on Apple targets.
        let st = std::process::Command::new("mkfifo")
            .arg("-m")
            .arg("644")
            .arg(root.join("fifo"))
            .status()
            .unwrap();
        assert!(st.success(), "mkfifo: {st}");
        let _sock = std::os::unix::net::UnixListener::bind(root.join("sock")).unwrap();
        let names = [
            "a.py",
            "run.sh",
            "tool",
            "data.unknownext",
            "secret.unknownext",
            "r654",
            "link.py",
            "broken",
            "dir",
            "fifo",
            "sock",
            "gone",
            "",
            "/dev/null",
        ];
        let fresh = || FileTable::from_args(names.iter().map(OsString::from).collect(), root);
        let full_needs = Needs {
            stat: true,
            encoding: true,
            shebang: true,
        };
        let truth = fresh();
        for filter in [
            &["file"][..],
            &["executable"],
            &["non-executable", "file"],
            &["symlink"],
            &["directory"],
            &["socket"],
        ] {
            let q = meta_query(filter);
            let t = fresh();
            for (i, name) in names.iter().enumerate() {
                let id = FileId(u32::try_from(i).unwrap());
                let got = t.tags_for(id, q);
                // The stat tier alone decides existence: full identification may also fail at the content.
                match (got, truth.tags(id, stat_needs())) {
                    (TagOutcome::Tags(m), TagOutcome::Tags(full)) => {
                        assert_eq!(m, full.intersection(META_TAGS), "{name} {filter:?}");
                    }
                    (TagOutcome::Vanished, TagOutcome::Vanished) => {}
                    (got, want) => panic!("{name} {filter:?}: {got:?} vs {want:?}"),
                }
                if let TagOutcome::Tags(full) = truth.tags(id, full_needs) {
                    let TagOutcome::Tags(m) = got else {
                        panic!("{name}")
                    };
                    assert_eq!(m, full.intersection(META_TAGS), "{name} {filter:?}");
                }
                assert!(t.tiers_ready(id, q), "{name}: memoised");
            }
            assert!(t.stat.0.get().is_none(), "{filter:?} built the stat tier");
            assert!(!t.tiers_ready(FileId(0), Query::full(stat_needs())));
        }
        std::fs::set_permissions(
            root.join("secret.unknownext"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
    }

    /// ADR-110 with trusted index modes (ADR-106): a meta-only query that cannot observe the executable bit takes the
    /// kind from the mode with zero syscalls (the root does not exist); one that references `executable` takes the
    /// `lstat` answer (gone here), and so does a later one after the meta tier was taken from the index; the stat tier
    /// a later name-tag query builds from a trusted meta stays trusted, also after the flag was cleared.
    #[cfg(unix)]
    #[test]
    fn meta_tier_under_index_trust() {
        let mut t = table(&["a.py", "run.sh", "link", "subm", "tool"]);
        let valid = Arc::new(AtomicBool::new(true));
        let modes: Box<[u32]> = Box::new([0o100_644, 0o100_755, 0o120_000, 0o160_000, 0o100_755]);
        assert!(t.trust_index_modes(modes, Arc::clone(&valid), tags_of(&["file", "symlink"])));
        let file = meta_query(&["file"]);
        let expect = |id: u32, names: &[&str]| {
            assert!(
                matches!(t.tags_for(FileId(id), file), TagOutcome::Tags(x) if x == tags_of(names)),
                "{id}"
            );
        };
        expect(0, &["file", "non-executable"]);
        expect(1, &["file", "executable"]);
        expect(2, &["symlink"]);
        // a gitlink keeps its lstat: gone under the nonexistent root
        assert!(matches!(t.tags_for(FileId(3), file), TagOutcome::Vanished));
        assert!(matches!(t.meta.peek(0), Some(MetaTier::Trusted(_))));
        // the executable bit is never the index's
        let exe = meta_query(&["executable"]);
        assert!(matches!(t.tags_for(FileId(1), exe), TagOutcome::Vanished));
        assert!(
            matches!(t.meta.peek(1), Some(MetaTier::Trusted(_))),
            "the trusted cell stays for other queries"
        );
        assert!(
            t.exact_stat.0.get().is_some(),
            "answered from the exact cells"
        );
        assert!(matches!(t.tags_for(FileId(4), exe), TagOutcome::Vanished));
        assert!(matches!(t.meta.peek(4), Some(MetaTier::Vanished)));
        // after the flag is cleared a name-tag query builds the stat tier from the trusted meta, not an lstat
        valid.store(false, Ordering::Release);
        assert!(
            matches!(t.tags(FileId(0), stat_needs()), TagOutcome::Tags(x) if x.contains("python"))
        );
        assert!(matches!(t.stat.peek(0), Some(StatTier::Trusted(_))));
        // ... and a new meta tier takes an lstat
        let mut u = table(&["b.py"]);
        let valid = Arc::new(AtomicBool::new(false));
        assert!(u.trust_index_modes(Box::new([0o100_644]), valid, tags_of(&["file"])));
        assert!(matches!(u.tags_for(FileId(0), file), TagOutcome::Vanished));
    }

    /// ADR-110 under ADR-106 decision (4): when trusted index modes serve a run with a filter that needs shebang
    /// tags, a meta-only filter's query on that table is not meta-only, so it takes the stat tier exactly as without
    /// ADR-110: the trusted mode for a file whose name gives tags (zero syscalls: the root does not exist), the `lstat`
    /// for one whose name gives none (gone here) at that first identification, `tiers_ready` and adoption on the stat
    /// tier, and no meta column. The same filter on a table without shebang filters stays meta-only.
    #[cfg(unix)]
    #[test]
    fn shebang_runs_keep_meta_only_filters_on_the_stat_tier() {
        let cache = snp_pyre::RegexCache::new();
        let file_filter = types_filter(&["file"], &cache);
        let mut t = table(&["a.py", "tool", "link", "x"]);
        let modes: Box<[u32]> = Box::new([0o100_644, 0o100_755, 0o120_000, 0]);
        let valid = Arc::new(AtomicBool::new(true));
        assert!(t.trust_index_modes(modes, valid, tags_of(&["file", "python"])));
        let q = t.query(&file_filter);
        assert_eq!(
            q,
            Query {
                meta_only: false,
                ..file_filter.query()
            }
        );
        assert!(file_filter.query().meta_only);
        assert!(!t.tiers_ready(FileId(0), q));
        assert!(matches!(t.tags_for(FileId(0), q), TagOutcome::Tags(x) if x.contains("python")));
        assert!(matches!(t.stat.peek(0), Some(StatTier::Trusted(_))));
        assert!(t.tiers_ready(FileId(0), q));
        assert!(matches!(t.tags_for(FileId(1), q), TagOutcome::Vanished));
        assert!(matches!(t.stat.peek(1), Some(StatTier::Vanished)));
        assert!(
            matches!(t.tags_for(FileId(2), q), TagOutcome::Tags(x) if x == tags_of(&["symlink"]))
        );
        let pending: Box<[Option<Meta>]> = Box::new([None, None, None, Some(Meta::regular(true))]);
        t.adopt(FileId(3), &pending, q);
        assert!(matches!(t.stat.peek(3), Some(StatTier::Present(_))));
        assert!(
            t.meta.0.get().is_none(),
            "a shebang run built the meta tier"
        );
        // without a shebang filter (`text` needs the content, not the shebang) the query stays meta-only
        let mut u = table(&["tool"]);
        let valid = Arc::new(AtomicBool::new(true));
        assert!(u.trust_index_modes(Box::new([0o100_755]), valid, tags_of(&["file", "text"])));
        let q = u.query(&file_filter);
        assert_eq!(q, file_filter.query());
        assert!(
            matches!(u.tags_for(FileId(0), q), TagOutcome::Tags(x) if x == tags_of(&["file", "executable"]))
        );
        assert!(matches!(u.meta.peek(0), Some(MetaTier::Trusted(_))));
        assert!(u.stat.0.get().is_none());
        // untrusted: meta-only
        assert_eq!(table(&["tool"]).query(&file_filter), file_filter.query());
    }

    /// The meta tier follows `retain_lexists` and a pending `lexists` meta is adopted into it (never into the stat
    /// tier) for a meta-only query, and into the stat tier otherwise.
    #[cfg(unix)]
    #[test]
    fn meta_tier_is_compacted_and_adopted() {
        let d = tempfile::tempdir().unwrap();
        for name in ["keep.py", "keep2.sh"] {
            std::fs::write(d.path().join(name), b"x").unwrap();
        }
        let mut t = FileTable::from_args(
            ["keep.py", "gone.py", "keep2.sh"]
                .iter()
                .map(OsString::from)
                .collect(),
            d.path(),
        );
        let file = meta_query(&["file"]);
        assert!(matches!(t.tags_for(FileId(1), file), TagOutcome::Vanished));
        assert!(matches!(t.tags_for(FileId(2), file), TagOutcome::Tags(_)));
        t.retain_lexists();
        assert_eq!(t.len(), 2);
        assert!(t.meta.peek(0).is_none());
        assert!(matches!(t.meta.peek(1), Some(MetaTier::Present(_))));
        let pending: Box<[Option<Meta>]> = Box::new([Some(Meta::regular(true)), None]);
        t.adopt(FileId(0), &pending, file);
        assert!(matches!(t.meta.peek(0), Some(MetaTier::Present(m)) if m.is_executable()));
        assert!(t.stat.0.get().is_none());
        // adopted for a full query: the stat tier, unless the meta tier already answered
        t.adopt(FileId(0), &pending, Query::full(stat_needs()));
        assert!(t.stat.peek(0).is_none());
        let u = FileTable::from_args(vec!["x.py".into()], d.path());
        u.adopt(FileId(0), &pending, Query::full(stat_needs()));
        assert!(u.meta.0.get().is_none());
        assert!(matches!(u.stat.peek(0), Some(StatTier::Present(_))));
    }

    /// `Classifier::new` leaves no pending column when every kept file was trusted (nothing to adopt), and replaces
    /// an older column either way.
    #[cfg(unix)]
    #[test]
    fn no_pending_column_when_every_kept_file_is_trusted() {
        let cache = snp_pyre::RegexCache::new();
        let pool = JobPool::new(NonZeroUsize::MIN, snp_core::cancel::CancelToken::new());
        let trusted = |modes: Box<[u32]>| {
            let mut t = table(&["a.py", "b.py"]);
            assert!(t.trust_index_modes(modes, Arc::new(AtomicBool::new(true)), TagSet::EMPTY));
            Arc::new(t)
        };
        let all = trusted(Box::new([0o100_644, 0o100_644]));
        all.set_pending(Some(Box::new([Some(Meta::regular(false)), None])));
        let c = classifier_of(&all, &cache, &pool);
        assert_eq!(c.kept.len(), 2);
        assert!(
            all.take_pending().is_none(),
            "the older column is dropped too"
        );
        // `b.py` has no mode and does not exist under the nonexistent root: still nothing to adopt
        let one = trusted(Box::new([0o100_644, 0]));
        let c = classifier_of(&one, &cache, &pool);
        assert_eq!(c.kept, [FileId(0)]);
        assert!(one.take_pending().is_none());
        // an `lstat`ed file leaves its meta to adopt
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("x.py"), b"x").unwrap();
        let t = Arc::new(FileTable::from_args(vec!["x.py".into()], d.path()));
        let c = classifier_of(&t, &cache, &pool);
        assert_eq!(c.kept, [FileId(0)]);
        assert!(matches!(t.take_pending().as_deref(), Some([Some(_)])));
    }

    /// The meta tier's cell stays small: 8 bytes against the stat tier's 64 (Linux's futex `Once`).
    #[cfg(target_os = "linux")]
    #[test]
    fn meta_cells_are_small() {
        assert!(size_of::<OnceLock<MetaTier>>() <= 8);
        assert!(size_of::<OnceLock<StatTier>>() >= 48);
    }

    /// ADR-110: the concatenation reports the first error in index order however the partitions finished, and
    /// otherwise grows the first partition once to the exact total.
    #[test]
    fn concat_ordered_keeps_order_and_reserves_once() {
        let text = |e: Failure| match e {
            Failure::Exc { exc, .. } => String::from_utf8_lossy(&exc.text).into_owned(),
            other => format!("{other:?}"),
        };
        let out = concat_ordered(vec![
            Ok(vec![1, 2]),
            Ok(vec![]),
            Ok(vec![3]),
            Ok(vec![4, 5]),
        ])
        .unwrap();
        assert_eq!(out, [1, 2, 3, 4, 5]);
        assert_eq!(out.capacity(), 5);
        assert_eq!(concat_ordered::<u8>(Vec::new()).unwrap(), Vec::<u8>::new());
        let e = concat_ordered(vec![
            Ok(vec![1]),
            Err(Failure::fatal("t", "first")),
            Ok(vec![2]),
            Err(Failure::fatal("t", "second")),
        ])
        .unwrap_err();
        assert_eq!(text(e), "first");
        // a slow early partition that fails still wins over a fast later one
        let pool = JobPool::new(
            NonZeroUsize::new(8).unwrap(),
            snp_core::cancel::CancelToken::new(),
        );
        let e = par_chunks_on(&pool, 4_096, REGEX_THREADS, false, |r| {
            if r.contains(&10) {
                std::thread::sleep(std::time::Duration::from_millis(30));
                return Err(Failure::fatal("t", "early"));
            }
            if r.contains(&4_000) {
                return Err(Failure::fatal("t", "late"));
            }
            Ok(r.collect::<Vec<usize>>())
        })
        .unwrap_err();
        assert_eq!(text(e), "early");
    }
}
