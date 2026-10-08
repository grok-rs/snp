//! `snp migrate lefthook|builtin|upstream` converters (ARCHITECTURE §1.4.23, §8.5; 05 §19, 09 §7.1).
//!
//! Owner: WP-34.

mod builtin;
mod lefthook;

use file_identify as _;
use snp_config as _;
use snp_hooks_data as _;
use snp_pyre as _;
use snp_yaml as _;

pub use builtin::{to_builtin, to_upstream};
pub use lefthook::{Converted, LefthookOpts, lefthook};
