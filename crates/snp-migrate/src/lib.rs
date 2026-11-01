//! `snp migrate lefthook|builtin|upstream` converters (ARCHITECTURE §1.4.23, §8.5; 05 §19, 09 §7.1).
//!
//! Owner: WP-34.

mod builtin;
mod lefthook;

pub use builtin::{not_converted as builtin_not_converted, to_builtin, to_upstream};
pub use lefthook::{Converted, LefthookOpts, lefthook};
