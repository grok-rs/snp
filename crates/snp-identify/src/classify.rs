//! Path classification: identify's `tags_from_path`, split into a pure
//! planning step ([`classify()`]) and at most one bounded read.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use crate::error::{IdentifyError, Result};
use crate::filename::{tags_from_filename, tags_from_interpreter};
use crate::meta::{ExecPolicy, FileKind, Meta};
use crate::shebang::{Source, parse_shebang_buf};
use crate::tags::{Tag, TagSet};
use crate::text::{TEXT_PROBE_LEN, is_text, is_text_bytes, read_full};

/// Outcome of [`classify()`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classification {
    /// The tags are final; the file does not need to be read.
    Done(TagSet),
    /// The file's leading bytes are needed; finish with [`Pending`].
    NeedsContent(Pending),
}

/// A classification waiting for file content (see [`classify()`]).
///
/// Content is needed when the name gives no `text`/`binary` tag (unknown
/// names and [`EXTENSIONS_NEED_BINARY_CHECK`](crate::extensions::EXTENSIONS_NEED_BINARY_CHECK)
/// entries) and, for executable files with an unknown name, to read the
/// shebang.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending {
    tags: TagSet,
    shebang: bool,
}

impl Pending {
    /// The tags known so far (type, mode and name tags).
    #[must_use]
    pub const fn tags(&self) -> TagSet {
        self.tags
    }

    /// Whether the shebang must be parsed (an executable file whose name
    /// gave no tags).
    #[must_use]
    pub const fn needs_shebang(&self) -> bool {
        self.shebang
    }

    /// Whether `text`/`binary` must be decided from the content (always
    /// true when [`needs_shebang`](Pending::needs_shebang) is).
    #[must_use]
    pub const fn needs_encoding(&self) -> bool {
        !self.tags.has_encoding()
    }

    /// Finish from the file's content read from its start.
    ///
    /// Reads [`TEXT_PROBE_LEN`] bytes (fewer at end of input); only a shebang
    /// line longer than that reads further, bounded by
    /// [`MAX_SHEBANG_BYTES`](crate::MAX_SHEBANG_BYTES).
    ///
    /// # Errors
    ///
    /// Propagates read errors.
    pub fn finish<R: Read>(self, mut reader: R) -> io::Result<TagSet> {
        let mut head = [0u8; TEXT_PROBE_LEN];
        let n = read_full(&mut reader, &mut head)?;
        self.finish_with_head(head.get(..n).unwrap_or_default(), reader)
    }

    /// Finish from in-memory content (the whole file or at least its first
    /// [`TEXT_PROBE_LEN`] bytes plus the shebang line).
    ///
    /// ```
    /// use file_identify::{Classification, Meta, classify};
    ///
    /// let Classification::NeedsContent(pending) = classify(Meta::regular(true), "bin/tool") else {
    ///     unreachable!()
    /// };
    /// let tags = pending.finish_bytes(b"#!/usr/bin/env python3\nprint('hi')\n");
    /// assert_eq!(tags.to_string(), "executable, file, python, python3, text");
    /// ```
    #[must_use]
    pub fn finish_bytes(self, content: &[u8]) -> TagSet {
        let head = content.get(..TEXT_PROBE_LEN).unwrap_or(content);
        let rest = content.get(head.len()..).unwrap_or_default();
        self.finish_with_head(head, rest).unwrap_or(self.tags)
    }

    /// `head` is the first `min(len, TEXT_PROBE_LEN)` bytes; `rest` continues
    /// right after it.
    fn finish_with_head<R: Read>(self, head: &[u8], rest: R) -> io::Result<TagSet> {
        let mut tags = self.tags;
        if self.shebang {
            let cmd = parse_shebang_buf(&mut Source::new(head, rest))?;
            if let Some(interpreter) = cmd.first() {
                tags = tags.union(tags_from_interpreter(interpreter));
            }
        }
        if !tags.has_encoding() {
            tags.insert(if is_text_bytes(head) {
                Tag::TEXT
            } else {
                Tag::BINARY
            });
        }
        Ok(tags)
    }
}

/// The I/O-free part of `identify.tags_from_path`: everything that follows
/// from the entry's [`Meta`] and name.
///
/// `path` may include directories (only the base name is used); see
/// [`tags_from_filename`] for the name rules. Directories, symlinks and
/// sockets are [`Done`](Classification::Done) with a single type tag. A
/// regular file gets `file`, `executable`/`non-executable` and its name tags;
/// it needs content only if that leaves no `text`/`binary` tag.
///
/// [`FileKind::Other`] (FIFOs, devices) is never read: it is classified like
/// a regular file whose content is unavailable and tagged `binary` when the
/// name does not decide the encoding. (identify would open it, which blocks
/// forever on a FIFO.)
///
/// ```
/// use file_identify::{Classification, Meta, classify};
///
/// // Known extension: no I/O needed at all.
/// let Classification::Done(tags) = classify(Meta::regular(false), "src/lib.rs") else {
///     unreachable!()
/// };
/// assert_eq!(tags.to_string(), "file, non-executable, rust, text");
///
/// // Unknown name: the first bytes decide text vs binary.
/// let Classification::NeedsContent(pending) = classify(Meta::regular(false), "data.bin2") else {
///     unreachable!()
/// };
/// assert!(pending.needs_encoding() && !pending.needs_shebang());
/// assert_eq!(pending.finish_bytes(b"\0\x01").to_string(), "binary, file, non-executable");
/// ```
#[must_use]
pub fn classify(meta: Meta, path: impl AsRef<[u8]>) -> Classification {
    let kind = meta.kind();
    let tags = match kind {
        FileKind::Directory => return Classification::Done(TagSet::from_tag(Tag::DIRECTORY)),
        FileKind::Symlink => return Classification::Done(TagSet::from_tag(Tag::SYMLINK)),
        FileKind::Socket => return Classification::Done(TagSet::from_tag(Tag::SOCKET)),
        FileKind::Regular | FileKind::Other => {
            TagSet::from_tag(Tag::FILE).with(if meta.is_executable() {
                Tag::EXECUTABLE
            } else {
                Tag::NON_EXECUTABLE
            })
        }
    };
    let name_tags = tags_from_filename(path);
    let shebang = name_tags.is_empty() && meta.is_executable();
    let mut tags = tags.union(name_tags);
    if kind == FileKind::Other {
        if !tags.has_encoding() {
            tags.insert(Tag::BINARY);
        }
        return Classification::Done(tags);
    }
    if !shebang && tags.has_encoding() {
        Classification::Done(tags)
    } else {
        Classification::NeedsContent(Pending { tags, shebang })
    }
}

/// The bytes of a path as Python sees them (raw bytes on Unix, WTF-8 on
/// Windows).
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

/// Open for reading without blocking on FIFOs (defence in depth: callers
/// only open entries believed to be regular files).
fn open_for_read(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let nonblock = i32::try_from(rustix::fs::OFlags::NONBLOCK.bits()).unwrap_or(0);
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(nonblock)
            .open(path)
    }
    #[cfg(not(unix))]
    {
        File::open(path)
    }
}

/// Run the content step for `path`, opening it once.
fn finish_from_file(path: &Path, pending: Pending, len_hint: Option<u64>) -> Result<TagSet> {
    let file = open_for_read(path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            // identify re-checks `os.path.lexists` right before opening.
            IdentifyError::not_found(path, e)
        } else {
            IdentifyError::from(e)
        }
    })?;
    Ok(finish_sized(pending, file, len_hint)?)
}

/// [`Pending::finish`] for a file believed to be `len_hint` bytes long: a
/// file smaller than [`TEXT_PROBE_LEN`] is read with a single `read` (no
/// extra `read` just to observe the end of the file). Without a hint, or for
/// an empty file (whose size may be unknown, as in `/proc`), up to
/// [`TEXT_PROBE_LEN`] bytes are requested.
fn finish_sized<R: Read>(
    pending: Pending,
    mut file: R,
    len_hint: Option<u64>,
) -> io::Result<TagSet> {
    let want = match len_hint {
        Some(len) if len > 0 => {
            usize::try_from(len).map_or(TEXT_PROBE_LEN, |l| l.min(TEXT_PROBE_LEN))
        }
        _ => TEXT_PROBE_LEN,
    };
    let mut head = [0u8; TEXT_PROBE_LEN];
    let n = read_full(&mut file, head.get_mut(..want).unwrap_or_default())?;
    pending.finish_with_head(head.get(..n).unwrap_or_default(), file)
}

/// Identify a path, exactly like `identify.tags_from_path`, with
/// [`ExecPolicy::PythonCompat`].
///
/// Cost: one `lstat`; the file is opened at most once and only when its name
/// does not determine `text`/`binary` (or it is an executable with an
/// unknown name, whose shebang is then read from the same buffer). At most
/// 1 KiB is read, plus the rest of an unusually long shebang line.
///
/// Differences from identify, all deliberate: FIFOs and devices are never
/// opened (see [`classify()`]); shebang lines longer than
/// [`MAX_SHEBANG_BYTES`](crate::MAX_SHEBANG_BYTES) are ignored.
///
/// # Errors
///
/// * [`IdentifyError::PathNotFound`] (`"{path} does not exist."`) if `lstat`
///   fails, or the file vanishes before it is opened;
/// * [`IdentifyError::IoError`] if opening or reading the file fails.
///
/// ```
/// # let dir = tempfile::tempdir()?;
/// # let path = dir.path().join("setup.py");
/// # std::fs::write(&path, "print('hello')\n")?;
/// use file_identify::{Tag, tags_from_path};
///
/// let tags = tags_from_path(&path)?;
/// // "file, non-executable, python, text" (on Windows identify reports
/// // every file as executable)
/// assert!(tags.contains(Tag::FILE) && tags.contains("python") && tags.contains(Tag::TEXT));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn tags_from_path(path: impl AsRef<Path>) -> Result<TagSet> {
    tags_from_path_with(path, ExecPolicy::PythonCompat)
}

/// [`tags_from_path`] with an explicit [`ExecPolicy`].
///
/// # Errors
///
/// As for [`tags_from_path`].
pub fn tags_from_path_with(path: impl AsRef<Path>, policy: ExecPolicy) -> Result<TagSet> {
    let path = path.as_ref();
    let md = fs::symlink_metadata(path).map_err(|e| IdentifyError::not_found(path, e))?;
    tags_from_metadata(path, &md, policy)
}

/// Identify a path whose `lstat` metadata ([`fs::symlink_metadata`]) the
/// caller already holds, e.g. from an existence check (pre-commit filters
/// files with `os.path.lexists` before identifying them).
///
/// This is [`tags_from_path_with`] without its `lstat`: the result is the
/// same, and the file size from `metadata` lets a small file be read with a
/// single `read` (open, read, close) instead of a second `read` to observe
/// the end of the file. Pass metadata that does not follow symlinks;
/// metadata from [`fs::metadata`] describes the link target instead.
///
/// # Errors
///
/// [`IdentifyError::PathNotFound`] if the file has to be opened but no
/// longer exists; [`IdentifyError::IoError`] for other open or read
/// failures.
///
/// ```
/// # let dir = tempfile::tempdir()?;
/// # let path = dir.path().join("notes.unknownext");
/// # std::fs::write(&path, "hello\n")?;
/// use file_identify::{ExecPolicy, tags_from_metadata, tags_from_path};
///
/// // The caller's own existence check ...
/// let md = std::fs::symlink_metadata(&path)?;
/// // ... is reused for identification: no second lstat.
/// let tags = tags_from_metadata(&path, &md, ExecPolicy::PythonCompat)?;
/// assert_eq!(tags, tags_from_path(&path)?);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn tags_from_metadata(
    path: impl AsRef<Path>,
    metadata: &fs::Metadata,
    policy: ExecPolicy,
) -> Result<TagSet> {
    let path = path.as_ref();
    let meta = Meta::from_metadata(metadata, policy);
    match classify(meta, path_bytes(path)) {
        Classification::Done(tags) => Ok(tags),
        Classification::NeedsContent(pending) => {
            finish_from_file(path, pending, Some(metadata.len()))
        }
    }
}

/// Identify a path whose [`Meta`] is already known (e.g. from
/// [`Meta::from_git_mode`]), without `lstat`.
///
/// No system call is made when the name decides everything; otherwise the
/// file is opened once, as in [`tags_from_path`]. The meta is trusted: a
/// path that does not exist is only reported when it has to be opened.
///
/// A [`Meta`] carries no file size, so a file that has to be read costs one
/// more `read` (to observe its end) than with [`tags_from_metadata`]; use
/// that when you hold `lstat` metadata.
///
/// A meta from [`Meta::from_git_mode`] describes the index; the result
/// matches identify only where the work tree agrees. A `120000` entry
/// yields `symlink`, but with `core.symlinks=false` (usual on Windows) the
/// work tree has a regular file there: on Windows, or whenever
/// `core.symlinks` is false, identify such entries with [`tags_from_path`].
///
/// # Errors
///
/// [`IdentifyError::PathNotFound`] if the file has to be opened but does not
/// exist; [`IdentifyError::IoError`] for other open or read failures.
///
/// ```
/// use file_identify::{Meta, tags_from_meta};
///
/// // 100644 from `git ls-files -s`: no file access needed for a .py file.
/// let meta = Meta::from_git_mode(0o100644).expect("regular file");
/// let tags = tags_from_meta("does/not/even/exist.py", meta)?;
/// assert_eq!(tags.to_string(), "file, non-executable, python, text");
/// # Ok::<(), file_identify::IdentifyError>(())
/// ```
pub fn tags_from_meta(path: impl AsRef<Path>, meta: Meta) -> Result<TagSet> {
    let path = path.as_ref();
    match classify(meta, path_bytes(path)) {
        Classification::Done(tags) => Ok(tags),
        Classification::NeedsContent(pending) => finish_from_file(path, pending, None),
    }
}

/// Whether the file at `path` looks like text, like `identify.file_is_text`
/// (reads at most [`TEXT_PROBE_LEN`] bytes).
///
/// Symlinks are followed for reading, as in Python. FIFOs and devices are
/// never opened and report `false` (binary).
///
/// # Errors
///
/// [`IdentifyError::PathNotFound`] if the path does not exist (`lstat`
/// fails); [`IdentifyError::IoError`] if opening or reading fails (e.g. a
/// dangling symlink or a directory).
pub fn file_is_text(path: impl AsRef<Path>) -> Result<bool> {
    let path = path.as_ref();
    let md = fs::symlink_metadata(path).map_err(|e| IdentifyError::not_found(path, e))?;
    let file_type = if md.file_type().is_symlink() {
        fs::metadata(path)?.file_type()
    } else {
        md.file_type()
    };
    if FileKind::from_file_type(file_type) == FileKind::Other {
        return Ok(false);
    }
    Ok(is_text(open_for_read(path)?)?)
}

/// Parse the shebang of the file at `path`, like
/// `identify.parse_shebang_from_file`: empty unless the file is executable
/// ([`ExecPolicy::PythonCompat`]: always on Windows), otherwise
/// [`parse_shebang`](crate::parse_shebang) on its content.
///
/// The result is the full command (interpreter and arguments), as used by
/// pre-commit to build the argv of hook scripts.
///
/// # Errors
///
/// [`IdentifyError::PathNotFound`] if the path does not exist;
/// [`IdentifyError::IoError`] if opening or reading fails, except for an
/// error Python reports as `EINVAL`, which yields an empty result as in
/// identify (on Windows that is every error the C runtime does not map to
/// another `errno`, e.g. `ERROR_INVALID_NAME` or `ERROR_CANT_ACCESS_FILE`).
/// FIFOs and devices are never opened (empty result).
///
/// ```
/// # let dir = tempfile::tempdir()?;
/// # let path = dir.path().join("tool");
/// # std::fs::write(&path, "#!/usr/bin/env -S python3 -u\n")?;
/// # #[cfg(unix)] {
/// # use std::os::unix::fs::PermissionsExt;
/// # std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
/// # }
/// use file_identify::parse_shebang_from_file;
///
/// assert_eq!(parse_shebang_from_file(&path)?, ["python3", "-u"]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn parse_shebang_from_file(path: impl AsRef<Path>) -> Result<Vec<String>> {
    parse_shebang_from_file_with(path, ExecPolicy::PythonCompat)
}

/// [`parse_shebang_from_file`] with an explicit [`ExecPolicy`] deciding
/// whether the file is executable (and therefore read at all).
///
/// With [`ExecPolicy::ModeBits`] only files with the owner `x` bit are read,
/// on every platform (so on Windows the result is always empty), which
/// agrees with [`tags_from_path_with`] and with a git index mode.
///
/// # Errors
///
/// As for [`parse_shebang_from_file`].
///
/// ```
/// # let dir = tempfile::tempdir()?;
/// # let path = dir.path().join("tool");
/// # std::fs::write(&path, "#!/bin/sh\n")?;
/// use file_identify::{ExecPolicy, parse_shebang_from_file_with};
///
/// # #[cfg(unix)] {
/// # use std::os::unix::fs::PermissionsExt;
/// // Owner x bit set: the shebang is read under both policies.
/// std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o744))?;
/// assert_eq!(parse_shebang_from_file_with(&path, ExecPolicy::ModeBits)?, ["/bin/sh"]);
/// // Group and other x bits only: git stages 100644, so ModeBits says no.
/// std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o655))?;
/// assert!(parse_shebang_from_file_with(&path, ExecPolicy::ModeBits)?.is_empty());
/// # }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn parse_shebang_from_file_with(
    path: impl AsRef<Path>,
    policy: ExecPolicy,
) -> Result<Vec<String>> {
    let path = path.as_ref();
    let md = fs::symlink_metadata(path).map_err(|e| IdentifyError::not_found(path, e))?;
    let md = if md.file_type().is_symlink() {
        match fs::metadata(path) {
            Ok(target) => target,
            // os.access() is False for a dangling symlink.
            Err(_) => return Ok(Vec::new()),
        }
    } else {
        md
    };
    if !policy.is_executable(&md) || FileKind::from_file_type(md.file_type()) == FileKind::Other {
        return Ok(Vec::new());
    }
    shebang_unless_einval(open_for_read(path))
}

/// The body of identify's
/// `try: with open(path, 'rb') as f: return parse_shebang(f)` /
/// `except OSError as e: if e.errno == errno.EINVAL: return ()`: an `EINVAL`
/// from opening *or reading* gives an empty command.
fn shebang_unless_einval<R: Read>(file: io::Result<R>) -> Result<Vec<String>> {
    match file.and_then(|file| parse_shebang_buf(&mut Source::new(&[], file))) {
        Ok(cmd) => Ok(cmd),
        Err(e) if is_python_einval(&e) => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

/// Whether Python would raise `err` as an `OSError` with `errno == EINVAL`.
#[cfg(unix)]
fn is_python_einval(err: &io::Error) -> bool {
    err.raw_os_error() == Some(rustix::io::Errno::INVAL.raw_os_error())
}

/// Whether Python would raise `err` as an `OSError` with `errno == EINVAL`.
///
/// `open` and `read` go through the C runtime there (`_wopen`, `_read`),
/// which maps the Win32 error code to `errno`.
#[cfg(windows)]
fn is_python_einval(err: &io::Error) -> bool {
    err.raw_os_error()
        .and_then(|code| u32::try_from(code).ok())
        .is_some_and(crt_errno_is_einval)
}

/// Whether Python would raise `err` as an `OSError` with `errno == EINVAL`.
#[cfg(not(any(unix, windows)))]
fn is_python_einval(err: &io::Error) -> bool {
    err.raw_os_error().is_some() && err.kind() == io::ErrorKind::InvalidInput
}

/// Whether the Microsoft C runtime maps the Win32 error `code` to `EINVAL`
/// (`__acrt_errno_from_os_error`): every code that is neither in its table
/// of other `errno` values nor in its `EACCES` (19..=36) or `ENOEXEC`
/// (188..=202) ranges.
#[cfg(any(windows, test))]
const fn crt_errno_is_einval(code: u32) -> bool {
    !matches!(
        code,
        2..=11
            | 15..=36
            | 53
            | 65
            | 67
            | 80
            | 82
            | 83
            | 89
            | 108
            | 109
            | 112
            | 114
            | 128..=130
            | 132
            | 145
            | 158
            | 161
            | 164
            | 167
            | 183
            | 188..=202
            | 206
            | 215
            | 1816
    )
}

/// Pre-loaded file information for I/O-free identification (kept for
/// compatibility; [`classify()`] + [`Pending::finish_bytes`] is the more
/// flexible equivalent).
#[derive(Clone, Copy, Debug)]
pub struct FileInfo<'a> {
    /// The file name (directories are ignored).
    pub filename: &'a str,
    /// The kind of file-system entry.
    pub file_kind: FileKind,
    /// Whether the file is executable.
    pub is_executable: bool,
    /// The file content (at least the first [`TEXT_PROBE_LEN`] bytes), or
    /// `None` if unavailable — then the result may lack a `text`/`binary`
    /// tag.
    pub content: Option<&'a [u8]>,
}

/// Identify a file from [`FileInfo`] without any I/O.
///
/// ```
/// use file_identify::{FileInfo, FileKind, tags_from_info};
///
/// let info = FileInfo {
///     filename: "script",
///     file_kind: FileKind::Regular,
///     is_executable: true,
///     content: Some(b"#!/bin/bash\necho hi\n"),
/// };
/// assert_eq!(tags_from_info(&info).to_string(), "bash, executable, file, shell, text");
/// ```
#[must_use]
pub fn tags_from_info(info: &FileInfo<'_>) -> TagSet {
    match classify(Meta::new(info.file_kind, info.is_executable), info.filename) {
        Classification::Done(tags) => tags,
        Classification::NeedsContent(pending) => match info.content {
            Some(content) => pending.finish_bytes(content),
            None => pending.tags(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(meta: Meta, name: &str) -> Option<String> {
        match classify(meta, name) {
            Classification::Done(tags) => Some(tags.to_string()),
            Classification::NeedsContent(_) => None,
        }
    }

    #[test]
    fn classify_kinds() {
        assert_eq!(
            done(Meta::new(FileKind::Directory, true), "x.py").as_deref(),
            Some("directory")
        );
        assert_eq!(
            done(Meta::new(FileKind::Symlink, false), "x.py").as_deref(),
            Some("symlink")
        );
        assert_eq!(
            done(Meta::new(FileKind::Socket, false), "x").as_deref(),
            Some("socket")
        );
        assert_eq!(
            done(Meta::new(FileKind::Other, true), "fifo").as_deref(),
            Some("binary, executable, file")
        );
        assert_eq!(
            done(Meta::new(FileKind::Other, false), "x.txt").as_deref(),
            Some("file, non-executable, plain-text, text")
        );
        assert_eq!(
            done(Meta::regular(true), "x.py").as_deref(),
            Some("executable, file, python, text")
        );
        // Name tags present: the shebang is not consulted even if executable.
        assert_eq!(
            done(Meta::regular(true), "x.sh").as_deref(),
            Some("executable, file, shell, text")
        );
    }

    #[test]
    fn pending_rules() {
        let Classification::NeedsContent(p) = classify(Meta::regular(false), "f.plist") else {
            panic!("plist needs a binary check");
        };
        assert!(!p.needs_shebang() && p.needs_encoding());
        assert_eq!(
            p.finish_bytes(b"bplist00\xd1\x01").to_string(),
            "binary, file, non-executable, plist"
        );
        assert_eq!(
            p.finish_bytes(b"<?xml").to_string(),
            "file, non-executable, plist, text"
        );

        let Classification::NeedsContent(p) = classify(Meta::regular(true), "run") else {
            panic!("unknown executable needs content");
        };
        assert!(p.needs_shebang() && p.needs_encoding());
        assert_eq!(
            p.finish_bytes(b"#!   \n").to_string(),
            "executable, file, text"
        );
        assert_eq!(
            p.finish_bytes(b"\x7fELF\x02").to_string(),
            "binary, executable, file"
        );
        assert_eq!(
            p.finish(&b"#!/bin/zsh\n"[..])
                .map(|t| t.to_string())
                .ok()
                .as_deref(),
            Some("executable, file, shell, text, zsh")
        );

        // A shebang line longer than the head is still read from the rest.
        let mut long = b"#!/usr/bin/env ".to_vec();
        long.extend(std::iter::repeat_n(b' ', 2000));
        long.extend_from_slice(b"python3\n\0");
        assert_eq!(
            p.finish_bytes(&long).to_string(),
            "executable, file, python, python3, text"
        );
        assert_eq!(
            p.finish(&long[..]).map(|t| t.to_string()).ok().as_deref(),
            Some("executable, file, python, python3, text")
        );
    }

    /// A reader that serves `data` in one chunk per call, then fails with
    /// `error` (if any) instead of reporting the end of input; counts calls.
    struct Scripted {
        data: &'static [u8],
        error: Option<fn() -> io::Error>,
        reads: usize,
    }

    impl Read for Scripted {
        #[expect(
            clippy::disallowed_methods,
            reason = "test double: `n` is at most both lengths"
        )]
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            if self.data.is_empty() {
                return self.error.map_or(Ok(0), |error| Err(error()));
            }
            let n = buf.len().min(self.data.len());
            let (now, later) = self.data.split_at(n);
            buf[..n].copy_from_slice(now);
            self.data = later;
            Ok(n)
        }
    }

    fn scripted(data: &'static [u8], error: Option<fn() -> io::Error>) -> Scripted {
        Scripted {
            data,
            error,
            reads: 0,
        }
    }

    #[cfg(unix)]
    fn einval() -> io::Error {
        io::Error::from_raw_os_error(rustix::io::Errno::INVAL.raw_os_error())
    }
    #[cfg(unix)]
    fn eio() -> io::Error {
        io::Error::from_raw_os_error(rustix::io::Errno::IO.raw_os_error())
    }
    #[cfg(windows)]
    fn einval() -> io::Error {
        io::Error::from_raw_os_error(1920) // ERROR_CANT_ACCESS_FILE
    }
    #[cfg(windows)]
    fn eio() -> io::Error {
        io::Error::from_raw_os_error(5) // ERROR_ACCESS_DENIED: EACCES
    }

    /// identify returns `()` for an `OSError` with `EINVAL` raised by
    /// `open()` *or* while `parse_shebang` reads the file.
    #[cfg(any(unix, windows))]
    #[test]
    fn shebang_einval_is_empty_on_open_and_on_read() {
        assert_eq!(
            shebang_unless_einval(Ok(scripted(b"#!/bin/sh\n", None))).ok(),
            Some(vec!["/bin/sh".to_owned()])
        );
        // EINVAL from open().
        assert_eq!(
            shebang_unless_einval::<Scripted>(Err(einval())).ok(),
            Some(vec![])
        );
        // EINVAL from the first read, and after `#!` was read.
        assert_eq!(
            shebang_unless_einval(Ok(scripted(b"", Some(einval)))).ok(),
            Some(vec![])
        );
        assert_eq!(
            shebang_unless_einval(Ok(scripted(b"#!/bin/", Some(einval)))).ok(),
            Some(vec![])
        );
        // Any other OS error propagates, from open() or from read().
        for result in [
            shebang_unless_einval::<Scripted>(Err(eio())),
            shebang_unless_einval(Ok(scripted(b"#!/bin/", Some(eio)))),
        ] {
            let Err(IdentifyError::IoError { source }) = result else {
                panic!("expected an I/O error, got {result:?}");
            };
            assert_eq!(source.raw_os_error(), eio().raw_os_error());
        }
        // A non-OS error has no errno, so it is never EINVAL.
        let custom = shebang_unless_einval(Ok(scripted(
            b"",
            Some(|| io::Error::new(io::ErrorKind::InvalidInput, "synthetic")),
        )));
        assert!(custom.is_err());
    }

    #[test]
    fn crt_errno_mapping() {
        // Explicitly EINVAL in the C runtime's table, or not in it at all.
        for code in [1, 12, 13, 14, 87, 123, 131, 362, 1920, 4390, 0xffff_ffff] {
            assert!(crt_errno_is_einval(code), "{code}");
        }
        // Mapped to another errno (ENOENT, EACCES, EBADF, ENOEXEC, ...).
        for code in [
            2, 3, 4, 5, 6, 8, 11, 15, 18, 19, 32, 33, 36, 53, 67, 80, 112, 145, 161, 183, 188, 202,
            206, 1816,
        ] {
            assert!(!crt_errno_is_einval(code), "{code}");
        }
    }

    /// A small file whose size is known is read with one `read` call; without
    /// the size a second `read` observes the end of the file.
    #[test]
    fn size_hint_saves_the_eof_read() {
        let Classification::NeedsContent(p) = classify(Meta::regular(false), "x.unknownext") else {
            panic!("unknown name needs content");
        };
        let mut file = scripted(b"hello\n", None);
        let tags = finish_sized(p, &mut file, Some(6)).ok();
        assert_eq!(
            tags.map(|t| t.to_string()).as_deref(),
            Some("file, non-executable, text")
        );
        assert_eq!(file.reads, 1);

        let mut file = scripted(b"hello\n", None);
        assert!(finish_sized(p, &mut file, None).is_ok());
        assert_eq!(file.reads, 2);

        // A size of zero may be a lie (procfs): read normally.
        let mut file = scripted(b"\0", None);
        let tags = finish_sized(p, &mut file, Some(0)).ok();
        assert_eq!(
            tags.map(|t| t.to_string()).as_deref(),
            Some("binary, file, non-executable")
        );
        assert_eq!(file.reads, 2);

        // An unterminated shebang still reads past the hinted size.
        let Classification::NeedsContent(p) = classify(Meta::regular(true), "tool") else {
            panic!("unknown executable needs content");
        };
        let mut file = scripted(b"#!/bin/zsh", None);
        let tags = finish_sized(p, &mut file, Some(10)).ok();
        assert_eq!(
            tags.map(|t| t.to_string()).as_deref(),
            Some("executable, file, shell, text, zsh")
        );
    }

    #[test]
    fn file_info_compat() {
        let info = FileInfo {
            filename: "noext",
            file_kind: FileKind::Regular,
            is_executable: false,
            content: None,
        };
        assert_eq!(tags_from_info(&info).to_string(), "file, non-executable");
        let info = FileInfo {
            content: Some(b"text"),
            ..info
        };
        assert_eq!(
            tags_from_info(&info).to_string(),
            "file, non-executable, text"
        );
    }

    #[test]
    fn send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Classification>();
        assert_send_sync::<Pending>();
        assert_send_sync::<Meta>();
        assert_send_sync::<FileInfo<'static>>();
        assert_send_sync::<IdentifyError>();
        assert_send_sync::<crate::TagTable>();
    }
}
