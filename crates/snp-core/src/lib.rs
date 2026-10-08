//! snp's vocabulary: stages, hook types, languages, brand and profile constants, the immutable
//! [`env::Env`] snapshot with envcontext-style patches, [`cancel::CancelToken`], the
//! [`failure::Failure`] envelope with Python error texts, the [`output::Printer`] (the only writer of
//! stdout/stderr, reproducing CPython's two-layer stdout), [`path::RepoPath`], [`clock::Clock`],
//! poison-proof locking ([`sync`]), the spawn lock and test-only fault injection
//! (ARCHITECTURE §1.4.1).
//!
//! Owner: WP-00 (implemented completely in wave 0).
#![forbid(unsafe_code)]

pub mod brand;
pub mod cancel;
pub mod clock;
pub mod env;
pub mod failure;
pub mod fault;
pub mod lang;
pub mod output;
pub mod path;
pub mod profile;
pub mod spawnlock;
pub mod stage;
pub mod sync;
