//! Verification root (ADR-095, docs/VERIFICATION.md §2.4): mounts the production kernel files unchanged, so every
//! item in them is verified exactly as it ships. Run with `cargo xtask proofs verus`.
//!
//! Each mount carries `#[verus_verify]` (without it Verus silently skips unannotated functions) and a `#[path]`
//! under `crates/`. One mount per `[[kernel]]` entry of `proofs/registry.toml`; the mount name is the entry's
//! `mount`; `cargo xtask proofs check` (G1) matches the mounts against the registry.
#![cfg_attr(verus_keep_ghost, feature(proc_macro_hygiene))]
// Also makes `assume_specification` impossible in mounted files (it declares an `unsafe` function).
#![forbid(unsafe_code)]

#[allow(unused_imports)]
use vstd::prelude::*;

pub mod spec;

/// K1: xargs partition core (snp-proc).
#[verus_verify]
#[path = "../../crates/snp-proc/src/xargs/partition.rs"]
pub mod snp_proc_xargs_partition;

/// K2: archive entry-name check (snp-net).
#[verus_verify]
#[path = "../../crates/snp-net/src/archive/entry.rs"]
pub mod snp_net_archive_entry;

/// K3: end-of-file-fixer (snp-hooks).
#[verus_verify]
#[path = "../../crates/snp-hooks/src/pch/p0/kernels/eof.rs"]
pub mod snp_hooks_pch_p0_kernels_eof;

/// K5a: `git.zsplit` byte loops (snp-pycompat).
#[verus_verify]
#[path = "../../crates/snp-pycompat/src/precommit/zsplit.rs"]
pub mod snp_pycompat_precommit_zsplit;

/// K5b: `ZList::split` spans and the `FindByte` kernel trait (snp-git).
#[verus_verify]
#[path = "../../crates/snp-git/src/zlist/split.rs"]
pub mod snp_git_zlist_split;

/// K6: end-of-file-fixer tail scan (snp-hooks).
#[verus_verify]
#[path = "../../crates/snp-hooks/src/pch/p0/kernels/eof_tail.rs"]
pub mod snp_hooks_pch_p0_kernels_eof_tail;
