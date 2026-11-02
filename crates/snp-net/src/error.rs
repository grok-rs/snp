//! Checksums and network errors (ARCHITECTURE §1.4.9).
//!
//! Owner: WP-16.

use std::fmt::Write as _;
use std::io;

/// An expected digest.
///
/// pre-commit 4.7.0 verifies no download at all (D-SEC-08); every artifact snp fetches carries one of these, taken
/// from an embedded table (uv, the ruby helpers) or from a checksum manifest fetched over TLS (node `SHASUMS256.txt`,
/// the go.dev JSON, rustup's `.sha256` sidecars), ADR-034/ADR-077.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Checksum {
    /// SHA-256.
    Sha256([u8; 32]),
}

impl Checksum {
    /// Parses a SHA-256 digest written as 64 hex digits (either case), as in `SHASUMS256.txt`, the go.dev JSON and
    /// rustup's `.sha256` sidecars. Anything else is `None`.
    pub fn sha256_hex(hex: &str) -> Option<Self> {
        let bytes = hex.as_bytes();
        if bytes.len() != 64 {
            return None;
        }
        let mut out = [0_u8; 32];
        let mut rest = bytes;
        for slot in &mut out {
            let (&[hi, lo], tail) = rest.split_first_chunk::<2>()?;
            *slot = hex_value(hi)?
                .checked_mul(16)?
                .checked_add(hex_value(lo)?)?;
            rest = tail;
        }
        Some(Self::Sha256(out))
    }

    /// The digest as lowercase hex (`hashlib.sha256(…).hexdigest()` form).
    pub fn hex(&self) -> String {
        match self {
            Self::Sha256(d) => to_hex(d),
        }
    }
}

/// The value of one ASCII hex digit.
const fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => b.checked_sub(b'0'),
        b'a'..=b'f' => match b.checked_sub(b'a') {
            Some(v) => v.checked_add(10),
            None => None,
        },
        b'A'..=b'F' => match b.checked_sub(b'A') {
            Some(v) => v.checked_add(10),
            None => None,
        },
        _ => None,
    }
}

/// Lowercase hex of `bytes`.
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::new();
    for b in bytes {
        // Writing into a String cannot fail.
        let _ignored = write!(s, "{b:02x}");
    }
    s
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

/// `url` with any `user:password@` userinfo removed, for error texts and logs (credentials never leave the process
/// through an error message, 04a §14.16).
pub(crate) fn scrub_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at_checked(authority_end).unwrap_or((rest, ""));
    match authority.rfind('@') {
        Some(at) => {
            let host = authority.get(at.saturating_add(1)..).unwrap_or("");
            format!("{scheme}://{host}{tail}")
        }
        None => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_round_trip_and_rejects() {
        let hex = "9167d72b3319674b6303c4cbe071854bba13ebdf3d76b1a7cbdc175471fb66d6";
        let c = Checksum::sha256_hex(hex).unwrap();
        assert_eq!(c.hex(), hex);
        assert_eq!(Checksum::sha256_hex(&hex.to_ascii_uppercase()).unwrap(), c);
        assert!(Checksum::sha256_hex("").is_none());
        let tail = hex.get(1..).unwrap();
        assert!(Checksum::sha256_hex(tail).is_none());
        assert!(Checksum::sha256_hex(&format!("{tail}g")).is_none());
        assert!(Checksum::sha256_hex(&format!("{hex}0")).is_none());
        let accented: String = std::iter::repeat_n('é', 32).collect();
        assert!(Checksum::sha256_hex(&accented).is_none());
    }

    #[test]
    fn scrub_url_removes_userinfo_only() {
        assert_eq!(
            scrub_url("https://u:p@example.com/a@b?x=@#@"),
            "https://example.com/a@b?x=@#@"
        );
        assert_eq!(scrub_url("https://example.com/x"), "https://example.com/x");
        assert_eq!(scrub_url("http://tok@h:8080"), "http://h:8080");
        assert_eq!(scrub_url("not a url"), "not a url");
    }

    #[test]
    fn display_texts() {
        assert_eq!(
            NetError::Offline.to_string(),
            "network access disabled (SNP_OFFLINE)"
        );
        assert_eq!(
            NetError::Http {
                url: "https://x/y".into(),
                status: 404
            }
            .to_string(),
            "GET https://x/y: HTTP 404"
        );
        assert_eq!(
            NetError::UnsafeEntry("../x".into()).to_string(),
            "archive entry \"../x\" escapes the destination"
        );
        assert_eq!(
            NetError::Insecure("http://m/x".into()).to_string(),
            "insecure URL refused: http://m/x"
        );
    }
}
