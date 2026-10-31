//! `repo: builtin` native variants (`NativeSource::Builtin`; 09 §7, ARCHITECTURE §7.3).
//!
//! The builtin hooks accept exactly the upstream CLI (same flags, messages and exit codes) plus extra flags, and
//! fix the upstream defects of 09 §7.2:
//!
//! | id | fixes / extras |
//! |---|---|
//! | trailing-whitespace | a file named `x.` is not markdown by default; `--check` |
//! | end-of-file-fixer | read-only files that need no change pass (as in substitution mode); `--check` |
//! | mixed-line-ending | `--check` (report what `--fix` would do) |
//! | fix-byte-order-marker | byte-level (no UTF-8 validation crash); `--check` |
//! | check-json | a document nested deeper than CPython allows is reported, not a crash |
//! | check-added-large-files | `-z` git plumbing (non-ASCII names are checked), input order, `--include-renames` |
//! | check-executables-have-shebangs | `:(literal)` pathspecs; one message per path |
//! | check-merge-conflict | also `CHERRY_PICK_HEAD` / `REVERT_HEAD`; `--diff3` (`||||||| `) |
//! | detect-private-key, no-commit-to-branch | as upstream v6.0.0 |
//!
//! The other ids of `snp_hooks_data::BUILTIN_MANIFEST_YAML` (the P1/P2/caveat hooks and the `language: fail`
//! ids) have no dedicated variant: [`crate::native`] resolves them to the pre-commit-hooks native, driven with
//! the v6.0.0 epoch. Validation of `repo: builtin` blocks is snp-config's (ADR-070); defaults (name, types,
//! stages, …) come from the manifest.
//!
//! Owner: WP-26.

use crate::NativeHook;
use crate::pch::p0::{
    check_added_large_files, check_executables_have_shebangs, check_json, check_merge_conflict,
    detect_private_key, end_of_file_fixer, fix_byte_order_marker, mixed_line_ending,
    no_commit_to_branch, trailing_whitespace,
};

/// The builtin variants.
pub(crate) static HOOKS: &[&dyn NativeHook] = &[
    &trailing_whitespace::BUILTIN,
    &end_of_file_fixer::BUILTIN,
    &mixed_line_ending::BUILTIN,
    &fix_byte_order_marker::BUILTIN,
    &check_merge_conflict::BUILTIN,
    &detect_private_key::BUILTIN,
    &check_added_large_files::BUILTIN,
    &no_commit_to_branch::BUILTIN,
    &check_json::BUILTIN,
    &check_executables_have_shebangs::BUILTIN,
];

/// Whether `id` is a hook of the builtin manifest (`-   id: <id>` lines of `BUILTIN_MANIFEST_YAML`).
pub(crate) fn listed(id: &str) -> bool {
    snp_hooks_data::BUILTIN_MANIFEST_YAML
        .lines()
        .filter_map(|l| l.strip_prefix("-   id: "))
        .any(|i| i.trim() == id)
}
