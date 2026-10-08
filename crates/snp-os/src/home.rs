//! `os.path.expanduser` over the captured environment, and the default cache root.
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use snp_core::env::Env;
use snp_pycompat as _;

/// Python semantics incl. `HOME=""` and Windows `USERPROFILE` (15 §A4.9-10).
pub fn expanduser(p: &OsStr, env: &Env) -> OsString {
    todo!()
}

/// `XDG_CACHE_HOME` (non-empty) else `expanduser("~/.cache")`.
pub fn default_cache_root(env: &Env) -> Option<PathBuf> {
    todo!()
}
