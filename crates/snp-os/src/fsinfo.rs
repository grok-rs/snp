//! Filesystem-type probes (`DiffOracle` gate, ADR-078).
//!
//! Owner: WP-15.

use std::io;
use std::path::Path;

/// `DiffOracle` gate (ADR-078): an allow-list of filesystems with kernel-maintained ctime and ns mtime — Linux statfs
/// `f_type` ∈ {ext2/3/4, xfs, btrfs, tmpfs, zfs, f2fs, bcachefs}; macOS `f_fstypename` ∈ {apfs, hfs}. Everything else
/// (NFS, SMB, 9p, every FUSE incl. sshfs/rclone/virtiofs, overlayfs, vfat, exfat, ntfs-3g, unknown) → false.
///
/// snp-only (pre-commit always runs `git diff`). An error of `statfs` (missing path, permission) propagates; other
/// operating systems (and Windows, which always uses Git mode, ADR-027) answer `false`.
pub fn stat_oracle_safe(path: &Path) -> io::Result<bool> {
    imp::stat_oracle_safe(path)
}

/// The Linux `f_type` magic numbers of the allow-list (`linux/magic.h`).
#[cfg(any(test, target_os = "linux", target_os = "android"))]
const LINUX_SAFE_MAGICS: [(u32, &str); 7] = [
    (0x0000_EF53, "ext2/ext3/ext4"),
    (0x5846_5342, "xfs"),
    (0x9123_683E, "btrfs"),
    (0x0102_1994, "tmpfs"),
    (0x2FC1_2FC1, "zfs"),
    (0xF2F5_2010, "f2fs"),
    (0xCA45_1A4E, "bcachefs"),
];

/// Whether a Linux `f_type` is on the allow-list. 32-bit glibc reports it as a signed `int`, so the
/// sign-extended form of a magic above `i32::MAX` is accepted too.
#[cfg(any(test, target_os = "linux", target_os = "android"))]
fn linux_magic_is_safe(raw: i128) -> bool {
    let bits = u32::try_from(raw)
        .ok()
        .or_else(|| i32::try_from(raw).ok().map(i32::cast_unsigned));
    bits.is_some_and(|b| LINUX_SAFE_MAGICS.iter().any(|(m, _)| *m == b))
}

/// Whether a macOS `f_fstypename` (NUL-terminated) is on the allow-list.
#[cfg(any(test, target_os = "macos", target_os = "ios"))]
fn apple_fstypename_is_safe(name: &[u8]) -> bool {
    let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
    matches!(name.get(..end), Some(b"apfs" | b"hfs"))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use std::io;
    use std::path::Path;

    pub(super) fn stat_oracle_safe(path: &Path) -> io::Result<bool> {
        let st = rustix::fs::statfs(path)?;
        Ok(super::linux_magic_is_safe(i128::from(st.f_type)))
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use std::io;
    use std::path::Path;

    pub(super) fn stat_oracle_safe(path: &Path) -> io::Result<bool> {
        let st = rustix::fs::statfs(path)?;
        let name: Vec<u8> = st.f_fstypename.iter().map(|c| c.to_ne_bytes()[0]).collect();
        Ok(super::apple_fstypename_is_safe(&name))
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
mod imp {
    use std::io;
    use std::path::Path;

    pub(super) fn stat_oracle_safe(path: &Path) -> io::Result<bool> {
        // The path must still exist (an error is reported as on the other platforms).
        std::fs::metadata(path).map(|_| false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_and_name_tables() {
        for (magic, _) in LINUX_SAFE_MAGICS {
            assert!(linux_magic_is_safe(i128::from(magic)));
            // 32-bit glibc reports btrfs/f2fs/bcachefs magics as negative `int`s.
            assert!(linux_magic_is_safe(i128::from(magic.cast_signed())));
        }
        // overlayfs, FUSE, NFS, 9p, vfat, exfat, SMB2, proc, unknown.
        for magic in [
            0x794C_7630_u32,
            0x6573_5546,
            0x6969,
            0x0102_1997,
            0x4D44,
            0x2011_BAB0,
            0xFE53_4D42,
            0x9FA0,
            0,
        ] {
            assert!(!linux_magic_is_safe(i128::from(magic)), "{magic:#x}");
        }
        assert!(!linux_magic_is_safe(1 << 40));
        assert!(apple_fstypename_is_safe(b"apfs\0\0\0\0"));
        assert!(apple_fstypename_is_safe(b"hfs\0junk"));
        assert!(apple_fstypename_is_safe(b"apfs"));
        for name in [
            &b"nfs\0"[..],
            b"smbfs\0",
            b"msdos\0",
            b"exfat\0",
            b"macfuse\0",
            b"\0",
            b"apfsx\0",
        ] {
            assert!(!apple_fstypename_is_safe(name));
        }
    }

    #[test]
    fn missing_path_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        assert!(stat_oracle_safe(&d.path().join("missing")).is_err());
        assert!(stat_oracle_safe(d.path()).is_ok());
    }

    /// The classification of the filesystems actually mounted on this machine (`/proc/self/mounts`): ext4 and
    /// tmpfs are safe; overlay, FUSE, proc and sysfs are not.
    #[cfg(target_os = "linux")]
    #[test]
    fn mounted_filesystem_classification() {
        let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap();
        let mut checked = 0;
        for line in mounts.lines() {
            let mut f = line.split(' ');
            let (Some(_dev), Some(dir), Some(fstype)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            if dir.contains('\\') {
                continue; // octal escapes in the mount point
            }
            let expected = match fstype {
                "ext2" | "ext3" | "ext4" | "xfs" | "btrfs" | "tmpfs" | "zfs" | "f2fs"
                | "bcachefs" => true,
                t if t == "overlay" || t == "proc" || t == "sysfs" || t.starts_with("fuse") => {
                    false
                }
                _ => continue,
            };
            let Ok(got) = stat_oracle_safe(Path::new(dir)) else {
                continue; // not accessible to this user
            };
            assert_eq!(got, expected, "{fstype} at {dir}");
            checked += 1;
        }
        assert!(checked > 0, "no classifiable mount found");
    }
}
