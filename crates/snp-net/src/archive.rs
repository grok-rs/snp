//! Safe two-pass archive extraction (ADR-077).
//!
//! Owner: WP-16.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-16")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-16")]

use std::path::Path;

#[cfg(feature = "xz")]
use liblzma as _;

use crate::NetError;

/// Extraction options.
#[derive(Copy, Clone, Debug)]
pub struct Extract {
    /// Leading path components to strip (`tar --strip-components`).
    pub strip_components: u8,
    /// Allow symlinks whose target stays inside the extracted tree.
    pub allow_symlinks_inside: bool,
    /// Cap on the total extracted bytes.
    pub max_total_bytes: u64,
}

/// Extracts into `<dest>.part-<rand>` (same FS) in two passes — regular files and directories first, symlinks last —
/// then renames to `dest` (ADR-077). Rejected: absolute, `..` and drive-prefixed names; device, FIFO and socket
/// entries; hardlinks except to an already extracted regular file inside the tree; symlinks whose target, resolved
/// against the extracted tree, leaves it or passes through another symlink (no chains such as `x -> .`,
/// `y -> x/..`); parent directories are opened without following symlinks (Linux: openat2 `RESOLVE_BENEATH` |
/// `RESOLVE_NO_SYMLINKS` where available). setuid/setgid/sticky stripped. Never writes through pre-existing symlinks.
pub fn tar_gz(archive: &Path, dest: &Path, o: &Extract) -> Result<(), NetError> {
    todo!()
}

/// `.tar.xz` (node on Unix); the decoder exists only with feature `xz`.
pub fn tar_xz(archive: &Path, dest: &Path, o: &Extract) -> Result<(), NetError> {
    todo!()
}

/// `.zip`.
pub fn zip(archive: &Path, dest: &Path, o: &Extract) -> Result<(), NetError> {
    todo!()
}
