//! Store location (§5.1, FD-2, ADR-011).
//!
//! Owner: WP-22.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-22")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-22")]

use std::path::{Path, PathBuf};

use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_pycompat as _;

/// Which rule chose the store directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreSource {
    /// `SNP_HOME`.
    SnpHome,
    /// `$XDG_CACHE_HOME/snp`.
    XdgCacheHome,
    /// `expanduser("~/.cache")/snp` (Windows `%USERPROFILE%\.cache\snp`).
    PlatformDefault,
}

/// The resolved store directory.
#[derive(Clone, Debug)]
pub struct StoreLocation {
    /// The store root.
    pub path: PathBuf,
    /// The rule that chose it.
    pub source: StoreSource,
}

/// `SNP_HOME` > `$XDG_CACHE_HOME/snp` > `expanduser("~/.cache")/snp` (Windows `%USERPROFILE%\.cache\snp`); relative
/// values resolved against the invocation cwd; `realpath_nonstrict`'d. `PRE_COMMIT_HOME` is read ONLY to compute
/// pre-commit's store and refuse it (path equality or README signature) with `FatalError` M7 (ADR-011).
pub fn resolve_location(env: &Env, invocation_cwd: &Path) -> Result<StoreLocation, Failure> {
    todo!()
}
