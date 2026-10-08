//! CPython 3.12 stdlib semantics (+ the 3.13 delta) that pre-commit 4.6.2 relies on, verified by
//! committed vectors (docs/research/15-python-semantics-shims.md §A1–§A7).
//!
//! This is the frozen production contract of ARCHITECTURE §1.4.2 (ADR-056). The crate has **no
//! dependencies** besides std: it sits under every other snp crate, and `regex` lives only in
//! snp-pyre. The Unicode tables in `src/generated/` are produced by
//! `xtask/oracle/gen_pycompat_tables.py` under CPython 3.13 (UCD 15.1, ADR-020). No public function
//! returns `impl Trait`; iterators are named types. Every error type implements `Display` (= Python's
//! `str(exc)`), `std::error::Error` and `py_type()` (the Python class printed in crash lines).
//!
//! W0 (WP-00) vendored the verified prototype (`docs/research/pycompat-proto`) adapted to these
//! signatures; WP-11 completes the crate and regenerates the vectors.
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
pub mod ntpath;
pub mod ospath;
pub mod posixpath;
pub mod precommit;
pub mod random;
pub mod repr;
pub mod shlex;
pub mod text;
pub mod textio;

/// Python `ValueError` texts raised by `os.path` functions (both flavours).
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
    DifferentMounts {
        /// Drive of the path argument.
        path_drive: String,
        /// Drive of the start argument.
        start_drive: String,
    },
}

impl PathError {
    /// The Python exception class: always `ValueError`.
    pub const fn py_type(&self) -> &'static str {
        "ValueError"
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPathSpecified => f.write_str("no path specified"),
            Self::EmptySequence => f.write_str("commonpath() arg is an empty sequence"),
            Self::MixAbsRel => f.write_str("Can't mix absolute and relative paths"),
            Self::DifferentDrives => f.write_str("Paths don't have the same drive"),
            Self::DifferentMounts {
                path_drive,
                start_drive,
            } => write!(
                f,
                "path is on mount {}, start on mount {}",
                repr::repr_str(path_drive),
                repr::repr_str(start_drive)
            ),
        }
    }
}

impl std::error::Error for PathError {}
