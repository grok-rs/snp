//! `os.path.samefile`.
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use std::io;
use std::path::Path;

/// `os.path.samefile(a, b)`: same device and inode (Windows: volume serial and file index).
pub fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    todo!()
}
