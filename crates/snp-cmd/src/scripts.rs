//! Hook scripts: classification, the executable to embed, rendering and migration notes (11 §A.5–§A.6, §A.19;
//! §9.7).
//!
//! Owner: WP-33.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-33")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-33")]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use snp_core::brand::Brand;
use snp_core::env::Env;
use snp_core::stage::HookType;

pub use snp_git::hookscripts::{Owner, classify};

/// 11 §A.19.3: the stable executable to embed as `SNP_EXE`, never canonicalised. Candidates in order:
/// `absolute(arg0)` when arg0 contains a separator; then each absolute PATH entry joined with the brand's name —
/// `snp[.exe]` (snp brand) or `pre-commit[.exe]` (compat brand, so hooks keep the compat brand, ADR-060); then
/// `current`. The first candidate that is a file and `same_file` as `current` wins (Unix (dev, ino); Windows volume
/// serial + file index).
pub fn select_exe(brand: Brand, arg0: Option<&OsStr>, env: &Env, current: &Path) -> PathBuf {
    todo!()
}

/// 11 §A.19.4: same file, or a copy with equal length and BLAKE3, or `--version` matching `pre-commit X (snp Y)`.
pub fn is_snp(file: &Path, current: &Path) -> bool {
    todo!()
}

/// The §9.7 template, LF.
pub fn render(cfg: &OsStr, t: HookType, skip_missing: bool, snp_exe: &Path) -> Vec<u8> {
    todo!()
}

/// M1/M1b/M2/M3/M12 of 11 §A.5.4 — written to **stderr** in both brands (D-DIAG-02).
pub fn migration_notes(brand: Brand) -> &'static [&'static str] {
    todo!()
}
