//! Plain data returned by the git calls.
//!
//! Owner: WP-18.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::ZList;

/// `git rev-parse` discovery result.
#[derive(Clone, Debug)]
pub struct RepoInfo {
    /// Work-tree root.
    pub root: PathBuf,
    /// `--git-dir`.
    pub git_dir: PathBuf,
    /// `--git-common-dir`.
    pub common_dir: PathBuf,
    /// `--git-path index`.
    pub index_file: PathBuf,
    /// A linked worktree (`git_dir != common_dir`).
    pub is_worktree: bool,
}

/// A git object id (40 or 64 hex).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectId(
    /// W0 placeholder; WP-18 stores the hex (`Box<str>`).
    (),
);

/// `git diff-index --binary` outcome (02 §7).
#[derive(Debug)]
#[non_exhaustive]
pub enum DiffIndex {
    /// rc 0.
    Clean,
    /// rc 1 with blank output (CRLF quirk).
    CrlfQuirk,
    /// rc 1 with a patch streamed into the sink.
    Patch {
        /// Bytes written.
        bytes_written: u64,
    },
}

/// Streaming blake3 of `git diff --no-ext-diff --no-textconv --ignore-submodules` output (§3.7).
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

/// `MERGE_MSG` ∧ `MERGE_HEAD` (02 §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MergeState {
    /// Not merging.
    None,
    /// A merge is in progress.
    Merging,
}

/// `hook_impl._pre_push_ns` result (02 §16.3).
#[derive(Debug, Clone)]
pub struct PrePushNs {
    /// `--all-files` (new branch without remote history).
    pub all_files: bool,
    /// `--from-ref`.
    pub from_ref: Option<OsString>,
    /// `--to-ref`.
    pub to_ref: Option<OsString>,
    /// `--local-branch`.
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
    /// The modes.
    pub modes: Box<[u32]>,
}
