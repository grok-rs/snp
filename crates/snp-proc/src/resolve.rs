//! `parse_shebang.find_executable` / `normexe` / `normalize_cmd` (15 §A4.11, ADR-014).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use file_identify as _;
use snp_core::env::Env;

/// `parse_shebang.find_executable(name, env=…)` with an explicit cwd.
pub fn find_executable(name: &OsStr, env: &Env, cwd: &Path) -> Option<PathBuf> {
    todo!()
}

/// `parse_shebang.ExecutableNotFoundError` cases.
#[derive(Debug)]
#[non_exhaustive]
pub enum ExeError {
    /// "Executable `x` not found".
    NotFound(OsString),
    /// "`x` is a directory".
    IsDirectory(OsString),
    /// "Executable `x` is not executable".
    NotExecutable(OsString),
    /// Reading the shebang failed.
    Io(io::Error),
}

impl ExeError {
    /// "Executable `x` not found" (no trailing \n).
    pub fn message(&self) -> String {
        todo!()
    }
}

/// `parse_shebang.normexe(orig, env=…)`.
pub fn normexe(orig: &OsStr, env: &Env, cwd: &Path) -> Result<PathBuf, ExeError> {
    todo!()
}

/// normexe → shebang (`file_identify::parse_shebang_from_file_with`, empty for non-UTF-8 = identify parity) →
/// normexe again; Windows-only basename fallback for absolute shebang interpreters (D-PLAT-02).
pub fn normalize_cmd(cmd: &[OsString], env: &Env, cwd: &Path) -> Result<Vec<OsString>, ExeError> {
    todo!()
}

/// `current_exe` → absolute, not canonicalised (03 H.8.1).
pub fn self_exe() -> io::Result<PathBuf> {
    todo!()
}
