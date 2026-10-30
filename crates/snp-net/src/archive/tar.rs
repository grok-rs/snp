//! The tar walker (pass 1): ustar, GNU (long names, sparse) and pax entries via the `tar` crate, which is only used
//! to parse — never to unpack (`Entry::unpack` follows the archive's own rules, not ADR-077's).
//!
//! The `tar` crate reads every GNU long name/link and pax header whole into memory before it yields the entry, so the
//! decompressed stream passes through a [`Gated`] reader: between two entries at most [`MAX_METADATA_RECORD`] bytes
//! may be read outside entry data, which bounds those allocations (a gzip-bombed `././@LongLink` record is
//! [`NetError::TooLarge`], never hundreds of MiB of RSS).
//!
//! Owner: WP-16.

use std::cell::Cell;
use std::io::{self, Read};
use std::path::Path;

use tar::EntryType;

use super::{Extraction, MAX_METADATA_RECORD};
use crate::NetError;

/// Shared state of the [`Gated`] reader and the walker.
#[derive(Debug, Default)]
struct Gate {
    /// Entry data is being read (not counted: the extraction caps it).
    in_data: Cell<bool>,
    /// Bytes read outside entry data since the last entry.
    gap: Cell<u64>,
    /// The gap exceeded [`MAX_METADATA_RECORD`].
    tripped: Cell<bool>,
}

/// A reader that fails once more than [`MAX_METADATA_RECORD`] bytes are read between two entries outside their data.
#[derive(Debug)]
struct Gated<'g, R> {
    /// The decompressed stream.
    inner: R,
    /// The shared state.
    gate: &'g Gate,
}

impl<R: Read> Read for Gated<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if !self.gate.in_data.get() {
            let gap = self
                .gate
                .gap
                .get()
                .saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
            self.gate.gap.set(gap);
            if gap > MAX_METADATA_RECORD {
                self.gate.tripped.set(true);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "tar metadata record too large",
                ));
            }
        }
        Ok(n)
    }
}

/// Runs `f` with entry-data reads not counted against the metadata gap.
fn in_data<T>(gate: &Gate, f: impl FnOnce() -> T) -> T {
    gate.in_data.set(true);
    let r = f();
    gate.in_data.set(false);
    r
}

/// The link name of a (sym or hard) link entry.
fn link_name<R: Read>(entry: &tar::Entry<'_, R>) -> Vec<u8> {
    entry
        .link_name_bytes()
        .map(std::borrow::Cow::into_owned)
        .unwrap_or_default()
}

/// Feeds every entry of the tar stream `r` (of the archive file `label`) to `x`.
pub(crate) fn walk<R: Read>(r: R, label: &Path, x: &mut Extraction<'_>) -> Result<(), NetError> {
    let gate = Gate::default();
    let too_large = || NetError::TooLarge {
        url: label.display().to_string(),
        limit: MAX_METADATA_RECORD,
    };
    let fail = |e: io::Error| {
        if gate.tripped.get() {
            too_large()
        } else {
            NetError::Io(e)
        }
    };
    let mut archive = tar::Archive::new(Gated {
        inner: r,
        gate: &gate,
    });
    for entry in archive.entries().map_err(fail)? {
        let mut entry = entry.map_err(fail)?;
        gate.gap.set(0);
        x.count()?;
        let name = entry.path_bytes().into_owned();
        let mode = entry.header().mode().ok();
        match entry.header().entry_type() {
            EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => {
                let size = entry.size();
                in_data(&gate, || x.file(&name, mode, size, &mut entry))?;
            }
            EntryType::Directory => x.dir(&name, mode)?,
            EntryType::Symlink => {
                let target = link_name(&entry);
                x.symlink(&name, &target)?;
            }
            EntryType::Link => {
                let target = link_name(&entry);
                x.hard_link(&name, &target, mode)?;
            }
            // Metadata records (normally consumed by the `tar` crate itself) carry no file.
            EntryType::XGlobalHeader
            | EntryType::XHeader
            | EntryType::GNULongName
            | EntryType::GNULongLink => {}
            // Devices, FIFOs and everything unknown are never created.
            _ => return Err(Extraction::refuse(&name)),
        }
        // Data of a non-file entry is skipped without being kept (not part of the metadata gap).
        in_data(&gate, || io::copy(&mut entry, &mut io::sink())).map_err(fail)?;
    }
    Ok(())
}
