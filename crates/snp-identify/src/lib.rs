//! # file-identify
//!
//! File identification for Rust: a faithful port of Python's
//! [identify](https://github.com/pre-commit/identify), the library
//! [pre-commit](https://pre-commit.com) uses to decide which files a hook
//! runs on (`types`, `types_or`, `exclude_types`).
//!
//! Given a path, it returns a set of tags such as
//! `file, non-executable, python, text`, derived from the file type, its
//! permissions, its name or extension, its shebang and its first kilobyte.
//! The lookup tables are generated from identify
//! [`IDENTIFY_VERSION`] — [`ALL_TAGS`] is exactly identify's tag set.
//!
//! ## Quick start
//!
//! ```
//! use file_identify::{Tag, tags_from_filename, tags_from_path};
//!
//! // Name only, no I/O.
//! let tags = tags_from_filename("Dockerfile.xenial");
//! assert_eq!(tags.to_string(), "dockerfile, text");
//!
//! // Full identification (one lstat, at most one small read).
//! # let dir = tempfile::tempdir()?;
//! # let path = dir.path().join("main.rs");
//! # std::fs::write(&path, "fn main() {}\n")?;
//! let tags = tags_from_path(&path)?;
//! assert!(tags.contains(Tag::FILE));
//! assert!(tags.contains("rust"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Tags and tag sets
//!
//! [`Tag`] is a 2-byte handle to one of the [`TAG_COUNT`] known tags;
//! [`TagSet`] is a `Copy` bitset with allocation-free set operations and
//! pre-commit's filter in [`TagSet::matches_types`]. Parse user-supplied
//! names with [`Tag::from_name`] / [`TagSet::from_names`]; an unknown name is
//! exactly what pre-commit reports as `Type tag '…' is not recognized`.
//!
//! ## Identification API
//!
//! | Function | identify equivalent | I/O |
//! |---|---|---|
//! | [`tags_from_path`], [`tags_from_path_with`] | `tags_from_path` | 1 `lstat`, ≤ 1 open + 1 KiB read |
//! | [`tags_from_metadata`] | — (`lstat` metadata already at hand) | ≤ 1 open + 1 KiB read |
//! | [`tags_from_meta`] | — (known [`Meta`], e.g. from a git index mode) | ≤ 1 open |
//! | [`classify()`] + [`Pending`] | — | none / caller-provided content |
//! | [`tags_from_filename`] | `tags_from_filename` | none |
//! | [`tags_from_interpreter`] | `tags_from_interpreter` | none |
//! | [`file_is_text`], [`is_text`], [`is_text_bytes`] | `file_is_text`, `is_text` | ≤ 1 KiB |
//! | [`parse_shebang_from_file`], [`parse_shebang_from_file_with`], [`parse_shebang`], [`parse_shebang_bytes`] | `parse_shebang_from_file`, `parse_shebang` | first line(s) |
//!
//! [`extensions`] and [`interpreters`] expose the raw tables.
//!
//! ## Differences from identify
//!
//! Behaviour matches identify, including its quirks (Python `shlex` shebang
//! splitting, `nix-shell` shebangs, `os.path.splitext` rules, Windows
//! treating every file as executable). The deliberate exceptions keep the
//! library safe on hostile input:
//!
//! * FIFOs and devices are never opened (identify blocks on a FIFO); they are
//!   tagged `file` + mode + name tags, and `binary` if the name does not
//!   decide the encoding.
//! * The shebang parser reads at most [`MAX_SHEBANG_BYTES`]; a longer first
//!   line counts as "no shebang".
//! * On Unix, `os.access(X_OK)` is evaluated from the file's mode, owner and
//!   group (see [`ExecPolicy`]), ignoring ACLs and `noexec` mounts.
//!
//! Platform limitations (not deliberate):
//!
//! * On Windows a directory junction (and any other name-surrogate reparse
//!   point) is tagged `symlink`: the standard library reports it as a
//!   symlink and does not expose the reparse tag. identify (`os.lstat`)
//!   reports `directory` for a junction; true symlinks agree.
//!
//! The git index fast path ([`Meta::from_git_mode`], [`tags_from_meta`])
//! describes the index, not the work tree. It matches identify only when
//! the two agree: in particular a `120000` entry is a symlink only if git
//! created one. With `core.symlinks=false` (the usual case on Windows) git
//! checks the link out as a regular file holding the target path, which
//! identify tags `file`, …, `text`. On Windows, or whenever
//! `core.symlinks=false`, do not trust `120000`: `lstat` the file
//! ([`tags_from_path`], [`tags_from_metadata`]) instead.

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::pedantic)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::string_slice
    )
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod classify;
mod error;
mod filename;
#[rustfmt::skip]
mod generated;
mod meta;
mod shebang;
mod table;
mod text;

pub mod extensions;
pub mod interpreters;
pub mod tags;

pub use classify::{
    Classification, FileInfo, Pending, classify, file_is_text, parse_shebang_from_file,
    parse_shebang_from_file_with, tags_from_info, tags_from_meta, tags_from_metadata,
    tags_from_path, tags_from_path_with,
};
pub use error::{IdentifyError, Result};
pub use filename::{tags_from_filename, tags_from_interpreter};
pub use generated::{ALL_TAGS, IDENTIFY_VERSION, TAG_COUNT};
pub use meta::{ExecPolicy, FileKind, Meta};
pub use shebang::{MAX_SHEBANG_BYTES, ShlexError, parse_shebang, parse_shebang_bytes, shlex_split};
pub use table::TagTable;
pub use tags::{IntoTag, Tag, TagSet, UnknownTagError};
pub use text::{TEXT_PROBE_LEN, is_text, is_text_bytes};

// Compile and run the README examples as doctests.
#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDoctests;
