//! P0 natives: trailing-whitespace, end-of-file-fixer, mixed-line-ending, fix-byte-order-marker,
//! check-merge-conflict, detect-private-key, check-added-large-files, no-commit-to-branch, check-json,
//! check-executables-have-shebangs (ARCHITECTURE §7.4, 09 §1).
//!
//! Owner: WP-26.

use crate::NativeHook;

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[];
