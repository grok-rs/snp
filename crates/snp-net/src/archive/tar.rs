//! The tar walker (pass 1): ustar, GNU (long names, sparse) and pax entries via the `tar` crate, which is only used
//! to parse — never to unpack (`Entry::unpack` follows the archive's own rules, not ADR-077's).
//!
//! Owner: WP-16.

use std::io::Read;

use tar::EntryType;

use super::Extraction;
use crate::NetError;

/// Feeds every entry of the tar stream `r` to `x`.
pub(crate) fn walk<R: Read>(r: R, x: &mut Extraction<'_>) -> Result<(), NetError> {
    let mut archive = tar::Archive::new(r);
    for entry in archive.entries()? {
        let mut entry = entry?;
        x.count()?;
        let name = entry.path_bytes().into_owned();
        let mode = entry.header().mode().ok();
        match entry.header().entry_type() {
            EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => {
                let size = entry.size();
                x.file(&name, mode, size, &mut entry)?;
            }
            EntryType::Directory => x.dir(&name, mode)?,
            EntryType::Symlink => {
                let target = entry
                    .link_name_bytes()
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
                x.symlink(&name, &target)?;
            }
            EntryType::Link => {
                let target = entry
                    .link_name_bytes()
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
                x.hard_link(&name, &target)?;
            }
            // Metadata records (normally consumed by the `tar` crate itself) carry no file.
            EntryType::XGlobalHeader
            | EntryType::XHeader
            | EntryType::GNULongName
            | EntryType::GNULongLink => {}
            // Devices, FIFOs and everything unknown are never created.
            _ => return Err(Extraction::refuse(&name)),
        }
    }
    Ok(())
}
