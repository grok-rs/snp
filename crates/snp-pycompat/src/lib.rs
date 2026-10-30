//! CPython 3.12 stdlib semantics (+ the 3.13 delta) that pre-commit 4.6.2 relies on, verified by
//! committed vectors (docs/research/15-python-semantics-shims.md §A1–§A7).
//!
//! This is the frozen production contract of ARCHITECTURE §1.4.2 (ADR-056). The crate has **no
//! dependencies** besides std: it sits under every other snp crate, and `regex` lives only in
//! snp-pyre. The Unicode tables in `src/generated/` are produced by
//! `xtask/oracle/gen_pycompat_tables.py` under CPython 3.13 (UCD 15.1, ADR-020). No public function
//! returns `impl Trait`; iterators are named types. Every error type implements `Display` (=
//! Python's `str(exc)`), `std::error::Error` and `py_type()` (the Python class printed in crash
//! lines).
//!
//! Python profiles (ADR-020): string, path, shlex and codec semantics follow CPython 3.12 (the
//! `tests/vectors/pycompat-vectors.cpython312.json` vectors); `repr`/`isprintable` and the
//! east-asian widths follow the UCD 15.1 data of CPython 3.13
//! (`tests/vectors/pycompat-delta-cpython313.json`). `cargo xtask pycompat-vectors` regenerates
//! both files and the tables; `--check` proves the committed ones byte-identical.
//!
//! No function panics on any input (the crate is fuzzed by `fuzz/fuzz_targets/pycompat_*.rs`), and
//! the recursive Python originals (`posixpath._joinrealpath`) are iterative here.
//!
//! Owner: WP-11.
#![forbid(unsafe_code)]

use std::fmt;

pub mod chars;
pub mod codec;
pub mod eaw;
pub mod float;
mod generated;
pub mod int;
mod messages;
pub mod ntpath;
pub mod ospath;
pub mod posixpath;
pub mod precommit;
pub mod random;
pub mod repr;
pub mod shlex;
pub mod text;
pub mod textio;

/// Python `ValueError` texts raised by `os.path` functions (both flavours; `posixpath.py` and
/// `ntpath.py` of CPython 3.12).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathError {
    /// `relpath('')` → `no path specified`.
    NoPathSpecified,
    /// `commonpath([])` → `commonpath() arg is an empty sequence` (3.12 text).
    EmptySequence,
    /// `commonpath` over absolute and relative inputs → `Can't mix absolute and relative paths`.
    MixAbsRel,
    /// ntpath `commonpath` over different drives → `Paths don't have the same drive`.
    DifferentDrives,
    /// ntpath `relpath` across drives → `path is on mount %r, start on mount %r`.
    ///
    /// The fields are `String` (frozen contract, ARCH §1.4.2), so a drive holding an undecodable
    /// byte (u8 view) or a lone surrogate (u16 view) is stored with U+FFFD in its place and
    /// renders as `'\\\\�\\sh'` where CPython shows `'\\\\\udcff\\sh'`. Every decodable
    /// drive renders exactly.
    DifferentMounts {
        /// Drive of the path argument (`splitroot(abspath(path))[0]`).
        path_drive: String,
        /// Drive of the start argument.
        start_drive: String,
    },
}

impl PathError {
    /// The Python exception class: always `ValueError`.
    #[must_use]
    pub const fn py_type(&self) -> &'static str {
        messages::VALUE_ERROR
    }
}

impl fmt::Display for PathError {
    /// `str(exc)` of the CPython 3.12 `ValueError`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPathSpecified => f.write_str(messages::NO_PATH_SPECIFIED),
            Self::EmptySequence => f.write_str(messages::COMMONPATH_EMPTY),
            Self::MixAbsRel => f.write_str(messages::MIX_ABS_REL),
            Self::DifferentDrives => f.write_str(messages::DIFFERENT_DRIVES),
            Self::DifferentMounts {
                path_drive,
                start_drive,
            } => messages::different_mounts(
                f,
                &repr::repr_str(path_drive),
                &repr::repr_str(start_drive),
            ),
        }
    }
}

impl std::error::Error for PathError {}
