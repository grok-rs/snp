//! The zip walker (pass 1): stored and deflated entries via the `zip` crate (only used to parse; `ZipArchive::extract`
//! is never called). Unix modes come from the external attributes when the archive was made on Unix.
//!
//! Owner: WP-16.

use std::fs::File;
use std::io::{self, BufReader, Read};

use super::{Extraction, MAX_LINK_TARGET};
use crate::NetError;

/// `S_IFMT` and the file types of a zip entry's Unix mode.
const S_IFMT: u32 = 0o170_000;
/// Directory.
const S_IFDIR: u32 = 0o040_000;
/// Regular file.
const S_IFREG: u32 = 0o100_000;
/// Symbolic link.
const S_IFLNK: u32 = 0o120_000;

/// A zip error as an I/O error (`InvalidData`).
fn zip_error(e: &::zip::result::ZipError) -> NetError {
    NetError::Io(io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

/// Feeds every entry of the zip `file` to `x`.
pub(crate) fn walk(file: File, x: &mut Extraction<'_>) -> Result<(), NetError> {
    let mut archive = ::zip::ZipArchive::new(BufReader::new(file)).map_err(|e| zip_error(&e))?;
    for i in 0..archive.len() {
        x.count()?;
        let mut entry = archive.by_index(i).map_err(|e| zip_error(&e))?;
        let name = entry.name().map_err(|e| zip_error(&e))?.as_bytes().to_vec();
        if name.contains(&b'\\') {
            return Err(Extraction::refuse(&name));
        }
        if entry.encrypted() {
            return Err(NetError::Io(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("encrypted zip entry {:?}", String::from_utf8_lossy(&name)),
            )));
        }
        let unix = entry.unix_mode();
        let kind = unix.map_or(0, |m| m & S_IFMT);
        let mode = unix.map(|m| m & 0o777).filter(|m| *m != 0);
        match kind {
            S_IFLNK => {
                let mut target = Vec::new();
                (&mut entry)
                    .take(
                        u64::try_from(MAX_LINK_TARGET)
                            .unwrap_or(u64::MAX)
                            .saturating_add(1),
                    )
                    .read_to_end(&mut target)?;
                x.symlink(&name, &target)?;
            }
            S_IFDIR => x.dir(&name, mode)?,
            0 | S_IFREG if entry.is_dir() => x.dir(&name, mode)?,
            0 | S_IFREG => {
                let size = entry.size();
                x.file(&name, mode, size, &mut entry)?;
            }
            // Devices, FIFOs and sockets are never created.
            _ => return Err(Extraction::refuse(&name)),
        }
    }
    Ok(())
}
