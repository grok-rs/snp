//! Reference prototype of snp's `pycompat` layer: byte-exact ports of the
//! CPython 3.12 standard-library semantics that pre-commit 4.6.2 relies on.
//!
//! Every function documents the Python expression it reproduces. The golden
//! vectors in `research/vectors/pycompat-vectors.cpython312.json` are replayed
//! by `tests/vectors.rs`.
#![forbid(unsafe_code)]

pub mod codec;
pub mod int;
pub mod ntpath;
pub mod posixpath;
pub mod precommit;
pub mod repr;
pub mod shlex;
mod tables;
pub mod text;
pub mod textio;
pub mod unicode;

pub use tables::UNICODE_VERSION;
