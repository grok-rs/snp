//! Removed-hook stubs: check-byte-order-marker and fix-encoding-pragma (v6.0.0+), autopep8-wrapper, flake8,
//! pyflakes (v3.x) (ARCHITECTURE §7.4, 09 §1).
//!
//! Owner: WP-26.

use crate::NativeHook;

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[];
