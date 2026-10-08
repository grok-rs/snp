//! P2 natives: sort-simple-yaml, check-vcs-permalinks, forbid-new-submodules, name-tests-test, the pre-v5.0.0 fix-
//! encoding-pragma (ARCHITECTURE §7.4, 09 §1).
//!
//! Owner: WP-27.

use crate::NativeHook;

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[];
