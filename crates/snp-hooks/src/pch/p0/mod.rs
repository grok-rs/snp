//! P0 natives: trailing-whitespace, end-of-file-fixer, mixed-line-ending, fix-byte-order-marker,
//! check-merge-conflict, detect-private-key, check-added-large-files, no-commit-to-branch, check-json,
//! check-executables-have-shebangs (ARCHITECTURE §7.4, 09 §1).
//!
//! Every hook reproduces its console script of every supported tag (v3.0.0…v6.0.0) by epoch
//! (`snp_hooks_data::Epoch`, `generated/epochs.md`); the same types serve the `repo: builtin` variants
//! (`--check` flags and the upstream fixes of 09 §7.2), registered under `NativeSource::Builtin`. The byte-level
//! cores are public in [`kernels`] (fuzz targets and tests drive them without a repository).
//!
//! Owner: WP-26.

pub(crate) mod check_added_large_files;
pub(crate) mod check_executables_have_shebangs;
pub(crate) mod check_json;
pub(crate) mod check_merge_conflict;
pub(crate) mod common;
pub(crate) mod detect_private_key;
pub(crate) mod end_of_file_fixer;
pub(crate) mod fix_byte_order_marker;
pub mod kernels;
pub(crate) mod mixed_line_ending;
pub(crate) mod no_commit_to_branch;
pub(crate) mod trailing_whitespace;

use crate::NativeHook;

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[
    &trailing_whitespace::UPSTREAM,
    &end_of_file_fixer::UPSTREAM,
    &mixed_line_ending::UPSTREAM,
    &fix_byte_order_marker::UPSTREAM,
    &check_merge_conflict::UPSTREAM,
    &detect_private_key::UPSTREAM,
    &check_added_large_files::UPSTREAM,
    &no_commit_to_branch::UPSTREAM,
    &check_json::UPSTREAM,
    &check_executables_have_shebangs::UPSTREAM,
];
