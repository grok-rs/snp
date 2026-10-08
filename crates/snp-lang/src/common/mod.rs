//! Shared helpers of every backend (`lang_base` equivalents), owned by the framework.
//!
//! Owner: WP-16.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-16")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-16")]

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use md5 as _;
use snp_core::env::Env;
use snp_pycompat as _;

use crate::{CmdRunner, LangError};

/// `lang_base.exe_exists`.
pub fn exe_exists(name: &OsStr, env: &Env, cwd: &Path) -> bool {
    todo!()
}

/// `lang_base.setup_cmd`: runs `argv`, returns stdout, `CalledProcessError` (format A) on a non-zero exit.
pub fn setup_cmd(
    r: &dyn CmdRunner,
    argv: &[OsString],
    env: &Env,
    cwd: &Path,
) -> Result<Vec<u8>, LangError> {
    todo!()
}

/// `bin` | `Scripts`.
pub fn bin_dir(env_dir: &Path) -> PathBuf {
    todo!()
}

/// "pre-commit-" + md5(hex) (ADR-035).
pub fn docker_tag(repo_key_hex: &str) -> String {
    todo!()
}

/// Readonly-aware, contained `rmtree`.
pub fn rmtree(path: &Path, root: &Path) -> io::Result<()> {
    todo!()
}
