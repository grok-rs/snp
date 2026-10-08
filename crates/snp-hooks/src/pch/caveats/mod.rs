//! NATIVE-WITH-CAVEATS hooks behind their parity gates (09 §6.7): check-yaml, check-toml, check-xml, detect-aws-
//! credentials (ARCHITECTURE §7.4, 09 §1).
//!
//! Owner: WP-27.

use crate::NativeHook;

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[];
