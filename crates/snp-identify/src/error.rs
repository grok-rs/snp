//! Error type of the path-based functions.

use std::io;
use std::path::PathBuf;

/// Result type of the path-based functions.
pub type Result<T, E = IdentifyError> = std::result::Result<T, E>;

/// Errors from identifying a path.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IdentifyError {
    /// The path could not be `lstat`-ed (or vanished before it was opened).
    ///
    /// Like identify, every `lstat` failure maps here; the message is
    /// identify's `ValueError` text, `{path} does not exist.`, and the
    /// underlying error is available as the [`source`](std::error::Error::source).
    #[error("{} does not exist.", path.display())]
    PathNotFound {
        /// The path as given by the caller.
        path: PathBuf,
        /// The error reported by the operating system.
        #[source]
        source: io::Error,
    },

    /// Opening or reading the file failed (identify lets the `OSError`
    /// propagate, e.g. `PermissionError` for an unreadable file).
    #[error("IO error: {source}")]
    IoError {
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
}

impl From<io::Error> for IdentifyError {
    fn from(source: io::Error) -> Self {
        Self::IoError { source }
    }
}

impl IdentifyError {
    pub(crate) fn not_found(path: &std::path::Path, source: io::Error) -> Self {
        Self::PathNotFound {
            path: path.to_path_buf(),
            source,
        }
    }
}
