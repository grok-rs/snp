//! P2 natives: sort-simple-yaml, check-vcs-permalinks, forbid-new-submodules, name-tests-test, the pre-v5.0.0 fix-
//! encoding-pragma (ARCHITECTURE §7.4, 09 §1).
//!
//! Owner: WP-27.

// Byte scanning for this group's natives (09 §9.1), pre-declared by WP-00 in the WP-26-owned manifest (WBS
// "Integration-lead files"); drop an anchor once the crate is used, or ask WP-26 to remove the dependency.
use bstr as _;
use memchr as _;

use crate::NativeHook;

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[];
