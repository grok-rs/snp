//! P1 natives: pretty-format-json, file-contents-sorter, requirements-txt-fixer, check-case-conflict,
//! check-shebang-scripts-are-executable, check-symlinks, destroyed-symlinks, the pre-v5.0.0 check-byte-order-marker
//! (ARCHITECTURE §7.4, 09 §1).
//!
//! Owner: WP-27.

use crate::NativeHook;

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[];
