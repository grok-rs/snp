//! Default-version and executable-lookup memoisation (§6.1, ADR-070, ADR-079).
//!
//! Owner: WP-16.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-16")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-16"
)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::Env;
use snp_core::lang::LanguageId;

use crate::{Fingerprint, LangError};

/// Default-version and exe-lookup memoisation for one process, plus persistent facts in `cache-v1/probes` only where
/// sound (§6.1, ADR-079): a fact is persisted only when the resolved executable is a native binary (not a `#!` script)
/// and not a known proxy (pyenv/asdf/rbenv/nodenv shims, mise and rustup proxies, macOS `/usr/bin/*` xcrun shims); its
/// key is the realpath + (len, `mtime_ns`, ino) of that binary plus a digest of the env that steers it (PATH,
/// `PYENV_VERSION`, `ASDF_*`, `MISE_*`, `RUSTUP_TOOLCHAIN`, `DEVELOPER_DIR`, `VIRTUAL_ENV`, `CONDA_PREFIX`). Everything
/// else is memoised per process.
#[derive(Debug, Default)]
pub struct ToolProbe {
    /// W0 placeholder; WP-16 stores the `OnceLock` caches and the optional persistence dir.
    _priv: (),
}

impl ToolProbe {
    /// A probe cache persisting native facts under `persist_dir` (None: memoise per process only).
    pub fn new(persist_dir: Option<PathBuf>) -> Self {
        todo!()
    }

    /// Memoised `Language::default_version` result for `lang` under `env`; `probe` runs at most once per process.
    pub fn default_version(
        &self,
        lang: LanguageId,
        env: &Env,
        cwd: &Path,
        probe: &mut dyn FnMut() -> Result<Arc<str>, LangError>,
    ) -> Result<Arc<str>, LangError> {
        todo!()
    }

    /// `find_executable` memoised per (PATH, PATHEXT, name).
    pub fn find_exe(&self, name: &OsStr, env: &Env, cwd: &Path) -> Option<PathBuf> {
        todo!()
    }

    /// (len, mtime, ino/index) of an executable for health fast paths; None when it cannot be stat'ed.
    pub fn fingerprint(&self, exe: &Path) -> Option<Fingerprint> {
        todo!()
    }
}
