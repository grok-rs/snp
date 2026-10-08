//! Filesystem-type probes (`DiffOracle` gate, ADR-078).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use std::io;
use std::path::Path;

/// `DiffOracle` gate (ADR-078): an allow-list of filesystems with kernel-maintained ctime and ns mtime — Linux statfs
/// `f_type` ∈ {ext2/3/4, xfs, btrfs, tmpfs, zfs, f2fs, bcachefs}; macOS `f_fstypename` ∈ {apfs, hfs}. Everything else
/// (NFS, SMB, 9p, every FUSE incl. sshfs/rclone/virtiofs, overlayfs, vfat, exfat, ntfs-3g, unknown) → false.
pub fn stat_oracle_safe(path: &Path) -> io::Result<bool> {
    todo!()
}
