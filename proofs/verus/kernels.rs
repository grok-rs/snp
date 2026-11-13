//! Verification root (ADR-095, docs/VERIFICATION.md §2.4): mounts the production kernel files unchanged, so every
//! item in them is verified exactly as it ships. Run with `proofs/verus/run.sh` (later `cargo xtask proofs verus`).
//!
//! Each mount carries `#[verus_verify]` (without it Verus silently skips unannotated functions) and a `#[path]`
//! under `crates/`. One mount per `[[kernel]]` entry of `proofs/registry.toml`; the mount name is the entry's
//! `mount`. Keep each `#[path = "../../…"]` on one line: `run.sh --mutant` copies the mounted files from it.
#![cfg_attr(verus_keep_ghost, feature(proc_macro_hygiene))]
// Also makes `assume_specification` impossible in mounted files (it declares an `unsafe` function).
#![forbid(unsafe_code)]

#[allow(unused_imports)]
use vstd::prelude::*;

pub mod spec;
