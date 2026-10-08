//! The resolved git executable, its version and capabilities (02 G.10, ADR-051, ADR-063).
//!
//! Owner: WP-18.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-18")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-18")]

use std::ffi::OsString;
use std::path::Path;

use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::output::Printer;
use snp_os as _;

/// `git --version` triple.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    /// Major.
    pub major: u16,
    /// Minor.
    pub minor: u16,
    /// Patch.
    pub patch: u16,
}

/// Capability fallbacks by version band (02 G.2).
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per git capability (02 G.2)"
)]
pub struct GitCaps {
    /// The parsed version (None: unparsable).
    pub version: Option<GitVersion>,
    /// `ls-files --deduplicate` (≥ 2.31).
    pub ls_files_dedup: bool,
    /// `rev-parse --path-format` (≥ 2.31).
    pub path_format: bool,
    /// `GIT_CONFIG_COUNT` (≥ 2.31).
    pub config_env: bool,
    /// `init --object-format`.
    pub init_object_format: bool,
    /// Config-based hooks (≥ 2.54).
    pub config_hooks: bool,
}

/// `git` resolved once via `normalize_cmd(["git"])` so `CalledProcessError` argv matches pre-commit (02 G.10).
#[derive(Debug)]
pub struct GitExe {
    /// The resolved argv prefix.
    pub argv0: Vec<OsString>,
    /// Capabilities.
    pub caps: GitCaps,
}

impl GitExe {
    /// Version from `cache` when (exe realpath, size, mtime, `GIT_EXEC_PATH`) matches a passing entry, else
    /// `git --version`. Never persisted for a known proxy (macOS `/usr/bin/git` xcrun shim, scoop/asdf shims; ADR-079).
    /// `Git::discover` runs after this call (it needs `caps` for the 2.19–2.30 fallback); a cache hit costs one stat.
    pub fn resolve(env: &Env, cwd: &Path, cache: Option<&Path>) -> Result<Self, Failure> {
        todo!()
    }

    /// Never refuses (ADR-063): < 2.31 → one stderr warning per process unless `SNP_ALLOW_OLD_GIT` (D-DIAG-01) and the
    /// capability fallbacks; the < 2.19 stash refusal lives in snp-stash (D-DATA-17); config hooks need ≥ 2.54.
    pub fn version_gate(&self, printer: &Printer, env: &Env) -> Result<(), Failure> {
        todo!()
    }
}
