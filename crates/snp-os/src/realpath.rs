//! Non-strict `os.path.realpath`.
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use std::io;
use std::path::{Path, PathBuf};

/// posix `_joinrealpath` / nt rules (15 §A4.9-10), with an explicit cwd.
pub fn realpath_nonstrict(p: &Path, cwd: &Path) -> io::Result<PathBuf> {
    todo!()
}
