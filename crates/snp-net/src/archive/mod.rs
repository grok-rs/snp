//! Safe two-pass archive extraction (ADR-077).
//!
//! pre-commit 4.7.0 unpacks go archives with `tarfile.extractall`/`zipfile.extractall` and the ruby helpers with
//! `tarfile.extractall` (`golang.py:108-110`, `ruby.py:81-84`), without a filter: absolute names, `..`, links that
//! leave the destination and device entries are all honoured (D-SEC-08). snp extracts into `<dest>.part-<rand>` (same
//! filesystem) in two passes — directories, regular files and hard links first, symlinks last — and renames the
//! finished tree to `dest`; on any refusal or error the `.part` tree is removed and `dest` is untouched (a process
//! killed mid-extraction leaves only the `.part` directory, which the store's sweep collects).
//!
//! Refused (the whole extraction fails with [`NetError::UnsafeEntry`]): absolute, drive-prefixed and `..` names;
//! character/block devices, FIFOs, sockets and unknown entry types; hard links except to a regular file extracted
//! earlier from the same archive; symlinks when [`Extract::allow_symlinks_inside`] is false, and otherwise symlinks
//! whose target — resolved lexically against the extracted tree — leaves it, is absolute, passes through (or names)
//! another symlink of the archive, or steps back (`..`) over anything but a real directory of the tree (which also
//! catches symlinks reached through a case or normalisation variant of their name on macOS and Windows). setuid/setgid/sticky bits are stripped from every mode. Nothing is ever
//! written through a symlink: the extraction tree (`archive/tree.rs`) uses descriptor-relative, `O_NOFOLLOW` file
//! operations.
//!
//! Owner: WP-16.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::Path;

use crate::NetError;

mod entry;
mod path;
mod tar;
mod tree;
mod zip;

use path::{RelPath, entry_path, resolve_symlink, unsafe_entry};
use tree::{Tree, remove_part, set_file_mode};

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

/// Cap on the number of entries of one archive (inode exhaustion).
const MAX_ENTRIES: u64 = 1 << 20;
/// Cap on the components of one entry name (bounds every walk, including the cleanup).
const MAX_DEPTH: usize = 128;
/// Cap on one symlink target.
const MAX_LINK_TARGET: usize = 4096;
/// Default modes when an archive carries none (zip from Windows) or an unparsable one.
const DEFAULT_FILE_MODE: u32 = 0o644;
/// See [`DEFAULT_FILE_MODE`].
const DEFAULT_DIR_MODE: u32 = 0o755;
/// Cap on what an extraction keeps in memory besides file data: the paths, link names and targets it records, and
/// every metadata record (GNU long name/link, pax header) the `tar` crate reads into memory between two entries.
/// node, go and the ruby helpers need well under 8 MiB (go ≈ 15k entries).
pub(crate) const METADATA_BUDGET: u64 = 64 << 20;
/// Cap on the tar bytes between two entries that are not entry data: headers, padding and metadata records
/// (`tar::Archive` reads a long name or pax header whole into memory). Path names are ≤ 4096 bytes in practice.
pub(crate) const MAX_METADATA_RECORD: u64 = 1 << 20;

/// Extracts into `<dest>.part-<rand>` (same FS) in two passes — regular files and directories first, symlinks last —
/// then renames to `dest` (ADR-077). Rejected: absolute, `..` and drive-prefixed names; device, FIFO and socket
/// entries; hardlinks except to an already extracted regular file inside the tree; symlinks whose target, resolved
/// against the extracted tree, leaves it or passes through another symlink (no chains such as `x -> .`,
/// `y -> x/..`); parent directories are opened without following symlinks (Linux: openat2 `RESOLVE_BENEATH` |
/// `RESOLVE_NO_SYMLINKS` where available). setuid/setgid/sticky stripped. Never writes through pre-existing symlinks.
pub fn tar_gz(archive: &Path, dest: &Path, o: &Extract) -> Result<(), NetError> {
    let f = File::open(archive)?;
    extract(dest, o, |x| {
        tar::walk(flate2::read::GzDecoder::new(BufReader::new(f)), archive, x)
    })
}

/// `.tar.xz` (node on Unix); the decoder exists only with feature `xz`.
///
/// Without the feature this fails with `io::ErrorKind::Unsupported` and touches nothing (`tools::resolve` then
/// never selects a `.tar.xz` artifact).
pub fn tar_xz(archive: &Path, dest: &Path, o: &Extract) -> Result<(), NetError> {
    #[cfg(feature = "xz")]
    {
        let f = File::open(archive)?;
        extract(dest, o, |x| {
            tar::walk(liblzma::read::XzDecoder::new(BufReader::new(f)), archive, x)
        })
    }
    #[cfg(not(feature = "xz"))]
    {
        let _: (&Path, &Path, &Extract) = (archive, dest, o);
        Err(NetError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            "snp was built without .tar.xz support (feature `xz`)",
        )))
    }
}

/// `.zip`.
///
/// Unix modes come from the entries' external attributes when present (symlinks included); `\` in a name is
/// refused (the zip specification mandates `/`).
pub fn zip(archive: &Path, dest: &Path, o: &Extract) -> Result<(), NetError> {
    let f = File::open(archive)?;
    extract(dest, o, |x| zip::walk(f, x))
}

/// Creates the `.part` tree next to `dest`, runs `walk` (pass 1) and the symlink pass, applies the directory modes
/// and renames the tree to `dest`; removes the `.part` tree on any failure.
fn extract(
    dest: &Path,
    o: &Extract,
    walk: impl FnOnce(&mut Extraction<'_>) -> Result<(), NetError>,
) -> Result<(), NetError> {
    let name = dest.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid extraction destination {}", dest.display()),
        )
    })?;
    let parent = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let (part, mut tree) = create_part(parent, name)?;
    let result = (|| {
        let mut x = Extraction {
            tree: &mut tree,
            o,
            entries: 0,
            total: 0,
            files: HashSet::new(),
            dirs: Vec::new(),
            symlinks: Vec::new(),
            metadata: 0,
            implicit_seen: 0,
        };
        walk(&mut x)?;
        x.finish()
    })();
    drop(tree);
    let result =
        result.and_then(|()| crate::fetch::rename_into_place(&part, dest).map_err(NetError::from));
    if result.is_err() {
        remove_part(&part);
    }
    result
}

/// A fresh `<name>.part-<rand>` directory in `parent`.
fn create_part(parent: &Path, name: &std::ffi::OsStr) -> io::Result<(std::path::PathBuf, Tree)> {
    let mut last = io::Error::new(io::ErrorKind::AlreadyExists, "no free .part name");
    for _ in 0..16 {
        let mut n = name.to_os_string();
        n.push(format!(".part-{}", crate::util::random_suffix()));
        let part = parent.join(n);
        match Tree::create(&part) {
            Ok(t) => return Ok((part, t)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

/// A symlink entry deferred to the second pass.
#[derive(Debug)]
struct PendingLink {
    /// Where the link goes.
    path: RelPath,
    /// The raw target.
    target: Vec<u8>,
    /// The entry name (messages).
    name: Vec<u8>,
}

/// The state of one extraction, fed entry by entry by the format walkers.
#[derive(Debug)]
pub(crate) struct Extraction<'a> {
    /// The tree being populated.
    tree: &'a mut Tree,
    /// The options.
    o: &'a Extract,
    /// Entries seen.
    entries: u64,
    /// Bytes written so far.
    total: u64,
    /// Regular files extracted so far (hard-link targets).
    files: HashSet<RelPath>,
    /// Explicit directory entries and their modes.
    dirs: Vec<(RelPath, u32)>,
    /// Symlinks for the second pass.
    symlinks: Vec<PendingLink>,
    /// Bytes of recorded metadata (paths, link names and targets), against [`METADATA_BUDGET`].
    metadata: u64,
    /// The implicit directories of the tree already charged to `metadata`.
    implicit_seen: usize,
}

/// The memory a recorded path costs (an estimate including the per-component `OsString` overhead).
fn path_cost(p: &[OsString]) -> u64 {
    p.iter().fold(48_u64, |n, c| {
        n.saturating_add(u64::try_from(c.len()).unwrap_or(u64::MAX))
            .saturating_add(32)
    })
}

impl Extraction<'_> {
    /// Counts one entry against [`MAX_ENTRIES`].
    pub(crate) fn count(&mut self) -> Result<(), NetError> {
        self.entries = self.entries.saturating_add(1);
        if self.entries > MAX_ENTRIES {
            return Err(NetError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("archive has more than {MAX_ENTRIES} entries"),
            )));
        }
        Ok(())
    }

    /// Charges `bytes` of recorded metadata, plus the implicit directories the tree created since the last charge.
    fn charge(&mut self, bytes: u64) -> Result<(), NetError> {
        let implicit = self.tree.implicit_dirs();
        let new = implicit
            .get(self.implicit_seen..)
            .unwrap_or_default()
            .iter()
            .fold(0_u64, |n, p| n.saturating_add(path_cost(p)));
        self.implicit_seen = implicit.len();
        self.metadata = self.metadata.saturating_add(bytes).saturating_add(new);
        if self.metadata > METADATA_BUDGET {
            return Err(NetError::TooLarge {
                url: "archive metadata (names and link targets)".to_owned(),
                limit: METADATA_BUDGET,
            });
        }
        Ok(())
    }

    /// The validated path of an entry (`None`: stripped away).
    pub(crate) fn path(&self, raw: &[u8]) -> Result<Option<RelPath>, NetError> {
        let p = entry_path(raw, self.o.strip_components)?;
        if p.as_ref().is_some_and(|p| p.len() > MAX_DEPTH) {
            return Err(unsafe_entry(raw));
        }
        Ok(p)
    }

    /// A directory entry.
    pub(crate) fn dir(&mut self, raw: &[u8], mode: Option<u32>) -> Result<(), NetError> {
        let Some(rel) = self.path(raw)? else {
            return Ok(());
        };
        self.tree.mkdirs(&rel)?;
        self.charge(path_cost(&rel))?;
        self.dirs
            .push((rel, mode.unwrap_or(DEFAULT_DIR_MODE) & 0o777));
        Ok(())
    }

    /// A regular file entry of `size` bytes read from `data`.
    pub(crate) fn file(
        &mut self,
        raw: &[u8],
        mode: Option<u32>,
        size: u64,
        data: &mut dyn Read,
    ) -> Result<(), NetError> {
        let Some(rel) = self.path(raw)? else {
            return Ok(());
        };
        let limit = self.o.max_total_bytes;
        let too_large = || NetError::TooLarge {
            url: String::from_utf8_lossy(raw).into_owned(),
            limit,
        };
        let budget = limit.saturating_sub(self.total);
        if size > budget {
            return Err(too_large());
        }
        let mut f = self.tree.create_file(&rel)?;
        let copied = io::copy(&mut data.take(budget.saturating_add(1)), &mut f)?;
        if copied > budget {
            return Err(too_large());
        }
        f.flush()?;
        set_file_mode(&f, mode.unwrap_or(DEFAULT_FILE_MODE) & 0o777)?;
        drop(f);
        self.total = self.total.saturating_add(copied);
        self.charge(path_cost(&rel))?;
        self.files.insert(rel);
        Ok(())
    }

    /// A hard link entry: only to a regular file extracted earlier from this archive. Its mode (when the format has
    /// one) applies to the shared file, as CPython `tarfile` `chmod`s the link after `os.link`.
    pub(crate) fn hard_link(
        &mut self,
        raw: &[u8],
        target_raw: &[u8],
        mode: Option<u32>,
    ) -> Result<(), NetError> {
        let Some(rel) = self.path(raw)? else {
            return Ok(());
        };
        let target = self
            .path(target_raw)
            .ok()
            .flatten()
            .filter(|t| self.files.contains(t))
            .ok_or_else(|| unsafe_entry(raw))?;
        self.tree.hard_link(&target, &rel)?;
        if let Some(m) = mode {
            self.tree.set_link_mode(&rel, m & 0o777)?;
        }
        self.charge(path_cost(&rel))?;
        self.files.insert(rel);
        Ok(())
    }

    /// A symlink entry, deferred to the second pass.
    pub(crate) fn symlink(&mut self, raw: &[u8], target: &[u8]) -> Result<(), NetError> {
        let Some(rel) = self.path(raw)? else {
            return Ok(());
        };
        if !self.o.allow_symlinks_inside || target.len() > MAX_LINK_TARGET {
            return Err(unsafe_entry(raw));
        }
        let bytes = [target.len(), raw.len()]
            .iter()
            .fold(path_cost(&rel).saturating_mul(2), |n, l| {
                n.saturating_add(u64::try_from(*l).unwrap_or(u64::MAX))
            });
        self.charge(bytes)?;
        self.symlinks.push(PendingLink {
            path: rel,
            target: target.to_vec(),
            name: raw.to_vec(),
        });
        Ok(())
    }

    /// An entry type that is never extracted (device, FIFO, socket, unknown).
    pub(crate) fn refuse(raw: &[u8]) -> NetError {
        unsafe_entry(raw)
    }

    /// Pass 2 (symlinks) and the final directory modes (deepest first, so a read-only directory never blocks work
    /// below it).
    fn finish(mut self) -> Result<(), NetError> {
        let links: HashSet<RelPath> = self.symlinks.iter().map(|l| l.path.clone()).collect();
        let is_link = |p: &[OsString]| links.contains(p);
        for l in &self.symlinks {
            // The link's parent exists before its target is judged (`..` over it must be a real directory).
            // (A parent below an archive symlink is refused by `resolve_symlink` instead.)
            if let Some((_, parent)) = l.path.split_last()
                && !(1..=parent.len()).any(|i| parent.get(..i).is_some_and(is_link))
            {
                self.tree.mkdirs(parent)?;
            }
            let tree = &mut *self.tree;
            let resolved = resolve_symlink(&l.path, &l.target, &is_link, &mut |p| tree.is_dir(p))
                .ok_or_else(|| unsafe_entry(&l.name))?;
            let target = os_target(&l.target).ok_or_else(|| unsafe_entry(&l.name))?;
            let target_is_dir = self.tree.is_dir(&resolved);
            self.tree.symlink(&target, &l.path, target_is_dir)?;
        }
        // Implicit directories get `0o777 & !umask`, explicit entries their own mode (the last entry for a path
        // wins); applied deepest first, the root last.
        let implicit_mode = self.tree.implicit_dir_mode();
        let mut modes: Vec<(RelPath, u32, usize)> = self
            .tree
            .implicit_dirs()
            .iter()
            .map(|d| (d.clone(), implicit_mode))
            .chain(std::mem::take(&mut self.dirs))
            .enumerate()
            .map(|(seq, (p, m))| (p, m, seq))
            .collect();
        modes.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
        let mut last: Vec<(RelPath, u32)> = Vec::new();
        for (p, m, _) in modes {
            match last.last_mut() {
                Some((lp, lm)) if *lp == p => *lm = m,
                _ => last.push((p, m)),
            }
        }
        last.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        for (rel, mode) in &last {
            self.tree.set_dir_mode(rel, *mode)?;
        }
        self.tree.set_dir_mode(&[], implicit_mode)?;
        Ok(())
    }
}

/// A symlink target as an OS string (Unix: raw bytes; elsewhere: UTF-8 only).
#[cfg_attr(
    unix,
    expect(clippy::unnecessary_wraps, reason = "fallible on Windows only")
)]
fn os_target(t: &[u8]) -> Option<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(std::ffi::OsStr::from_bytes(t).to_os_string())
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(t).ok().map(OsString::from)
    }
}
