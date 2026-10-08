//! P1 natives: pretty-format-json, file-contents-sorter, requirements-txt-fixer, check-case-conflict,
//! check-shebang-scripts-are-executable, check-symlinks, destroyed-symlinks, the pre-v5.0.0 check-byte-order-marker
//! (ARCHITECTURE §7.4, 09 §1).
//!
//! Owner: WP-27.

// Byte scanning for this group's natives (09 §9.1), pre-declared by WP-00 in the WP-26-owned manifest (WBS
// "Integration-lead files"); drop an anchor once the crate is used, or ask WP-26 to remove the dependency.
use bstr as _;
use memchr as _;

use crate::NativeHook;

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[];
