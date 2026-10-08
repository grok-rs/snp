//! The file table: byte arena + spans, git modes, tier columns allocated on first need (§2.3, ADR-079).
//!
//! Owner: WP-22.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-22")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-22"
)]

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use file_identify::TagSet;
use snp_core::failure::PyExc;
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

/// One byte arena + spans; tier columns allocated on first need (§2.3, ADR-079).
#[derive(Debug)]
pub struct FileTable {
    /// W0 placeholder; WP-22 stores the arena, spans, git modes and the name/stat/head/view columns.
    _priv: (),
}

impl FileTable {
    /// From git `-z` output (git order kept); Windows: `\` → `/` in the matching view (02 §10).
    pub fn from_zbuf(buf: Vec<u8>, spans: Vec<(u32, u32)>, root: &Path) -> Self {
        todo!()
    }

    /// `--files` / commit-msg / try-repo / stdin.
    pub fn from_args(paths: Vec<OsString>, root: &Path) -> Self {
        todo!()
    }

    /// git index modes (Unix, `core.fileMode=true`, `core.symlinks=true`, after the stash: worktree == index)
    /// (§2.3); matched by path against the table; the caller iterates `snp_git::IndexEntries` without copying paths
    /// (snp-files does not depend on snp-git).
    pub fn set_trusted_modes<'a>(&mut self, entries: impl IntoIterator<Item = (&'a [u8], u32)>) {
        todo!()
    }

    /// Once, before any hook (02 §10).
    pub fn retain_lexists(&mut self) {
        todo!()
    }

    /// Number of files.
    pub fn len(&self) -> usize {
        todo!()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        todo!()
    }

    /// The path bytes as git printed them.
    pub fn bytes(&self, id: FileId) -> &[u8] {
        todo!()
    }

    /// The path as an `OsStr` (argv).
    pub fn os(&self, id: FileId) -> Cow<'_, OsStr> {
        todo!()
    }

    /// Lossy UTF-8 (D-CRASH-12).
    pub fn match_view(&self, id: FileId) -> &str {
        todo!()
    }

    /// The path relative to `prefix` when it lies under it (boundary-aware: `a/b` is under `a`, `ab/c` is not); the
    /// `root` extension and the v1.1 workspace mode use this (§1.6).
    pub fn relative_to(&self, id: FileId, prefix: &RepoPath) -> Option<Cow<'_, [u8]>> {
        todo!()
    }

    /// Lazy, memoised, thread-safe tiered identify. Each tier has its own cell, so a later call needing a higher
    /// tier never reads a partial result: the outcome combines exactly the tiers `n` requires.
    pub fn tags(&self, id: FileId, n: Needs) -> TagOutcome<'_> {
        todo!()
    }

    /// Fills the required tiers on the pool (runs inline when the caller holds a permit, §1.4.8).
    pub fn prefetch(&self, ids: &[FileId], n: Needs, pool: &JobPool) {
        todo!()
    }
}
