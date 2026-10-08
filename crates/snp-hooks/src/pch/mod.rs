//! pre-commit-hooks ports, one registry per group (§7.4); [`crate::native`] concatenates them.
//!
//! Owner: WP-26 (this file, `p0/`, `stubs/`); WP-27 owns `p1/`, `p2/` and `caveats/`.

pub mod caveats;
pub mod p0;
pub mod p1;
pub mod p2;
pub mod stubs;
