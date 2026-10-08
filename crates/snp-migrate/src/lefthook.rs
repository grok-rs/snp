//! lefthook → snp conversion (05 §19).
//!
//! Owner: WP-34.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-34")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-34")]

use std::path::PathBuf;

use snp_core::failure::Failure;

/// `snp migrate lefthook` options.
#[derive(Debug, Clone)]
pub struct LefthookOpts {
    /// The repository root.
    pub root: PathBuf,
    /// The lefthook config (default: discovered).
    pub config: Option<PathBuf>,
    /// The output path (default: `.pre-commit-config.yaml`).
    pub output: Option<PathBuf>,
    /// Emit only pre-commit-compatible keys.
    pub portable: bool,
    /// Convert `lefthook-local.yml` into the local overlay.
    pub local: bool,
    /// Fail on any lossy conversion.
    pub strict: bool,
}

/// The conversion result.
#[derive(Debug)]
pub struct Converted {
    /// The new `.pre-commit-config.yaml`.
    pub config_yaml: String,
    /// The local overlay, from `lefthook-local.yml`.
    pub overlay_yaml: Option<String>,
    /// The conversion report.
    pub report: Vec<String>,
    /// Whether anything could not be converted exactly.
    pub lossy: bool,
}

/// 05 §19 (pure: no git, no writes). `snp migrate lefthook --install` is sequenced by the binary: write the outputs
/// atomically → remove lefthook shims (restore `<hook>.old`), delete `.git/info/lefthook*` → `git lfs update
/// --force` when LFS is detected → `snp_cmd::install` for every emitted stage (which chains the git-lfs hooks as
/// `.legacy`).
pub fn lefthook(o: &LefthookOpts) -> Result<Converted, Failure> {
    todo!()
}
