# D-PLAT-12

tagged symlink (reading the reparse tag needs unsafe)

* **Class:** PLAT (COMPATIBILITY §5.5)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-PLAT-12.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.5)

| pre-commit 4.6.2 | snp |
|---|---|
| Windows: a directory junction is tagged `directory` | tagged `symlink` (reading the reparse tag needs `unsafe`) |

## Rationale

On Windows, CPython's `os.lstat` reports `S_IFLNK` only for `IO_REPARSE_TAG_SYMLINK`; a directory junction
(`IO_REPARSE_TAG_MOUNT_POINT`) is a directory, so identify tags it `directory` (14 P-69). Rust's
`symlink_metadata().file_type().is_symlink()` is true for every name-surrogate reparse point, junctions included, and
the reparse tag that tells them apart is reachable only through `FindFirstFileW`/`GetFileInformationByHandleEx` FFI
(`unsafe`, excluded by FD-5) or the unstable `windows_by_handle`. snp (file-identify's `FileKind::from_file_type`)
therefore tags a junction `symlink`.

Safe and narrow: git never lists a junction as a file (it stores symlinks, and junctions are created by tools outside
git), so the difference is reachable only through `--files`/stdin lists naming a junction; such a path is not a
regular file under either classification, so the default `types: [file]` excludes it in both tools.

## Examples

Windows only. A junction `j` (`mklink /J j subdir`) passed with `run --files j` to two hooks, `types: [directory]` and
`types: [symlink]` (`tests/differential/divergences/D-PLAT-12.yaml`): pre-commit 4.6.2 passes `j` to the
`directory` hook and skips the `symlink` hook (`(no files to check)`); snp does the opposite. Both skip it for a
default `types: [file]` hook. Recorded on a Windows runner by WP-35's harness (no Windows oracle on the development
machine).
