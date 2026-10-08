//! Checksums and network errors.
//!
//! Owner: WP-16.

use std::io;

/// An expected digest.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Checksum {
    /// SHA-256.
    Sha256([u8; 32]),
}

/// Every failure of this crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NetError {
    /// `SNP_OFFLINE` is set.
    #[error("network access disabled (SNP_OFFLINE)")]
    Offline,
    /// Non-success HTTP status.
    #[error("GET {url}: HTTP {status}")]
    Http {
        /// The URL (userinfo scrubbed).
        url: String,
        /// The status code.
        status: u16,
    },
    /// The downloaded bytes do not match the expected digest.
    #[error("checksum mismatch for {url}: expected {expected}, got {actual}")]
    Checksum {
        /// The URL.
        url: String,
        /// Expected hex digest.
        expected: String,
        /// Actual hex digest.
        actual: String,
    },
    /// The response exceeds the size cap.
    #[error("{url}: response exceeds {limit} bytes")]
    TooLarge {
        /// The URL.
        url: String,
        /// The cap in bytes.
        limit: u64,
    },
    /// Local I/O.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// TLS, DNS, proxy or connection failure.
    #[error("{url}: {msg}")]
    Transport {
        /// The URL.
        url: String,
        /// The transport error text.
        msg: String,
    },
    /// An archive entry would escape the destination.
    #[error("archive entry {0:?} escapes the destination")]
    UnsafeEntry(String),
    /// An `http://` URL without `SNP_ALLOW_INSECURE_MIRROR`.
    #[error("insecure URL refused: {0}")]
    Insecure(String),
    /// The cancel token fired.
    #[error("interrupted")]
    Cancelled,
}
