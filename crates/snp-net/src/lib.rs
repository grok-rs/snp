//! The only crate that touches the network: verified HTTPS downloads, safe archive extraction and the toolchain
//! catalogue (ARCHITECTURE §1.4.9, §6.3, ADR-034, ADR-077).
//!
//! * [`HttpFetch`] (the real [`Fetch`]): TLS (rustls with the platform verifier, or `SSL_CERT_FILE`/`SSL_CERT_DIR`),
//!   proxies from the captured environment, 30 s connect / 60 s idle timeouts, 3 retries with backoff on 5xx and I/O
//!   errors, size caps, `SNP_OFFLINE`, `http://` refused unless `SNP_ALLOW_INSECURE_MIRROR` (archives only), sha256
//!   verified while streaming and a rename only after the digest matched.
//! * [`archive`]: two-pass extraction into `<dest>.part-<rand>` that refuses escaping names, links and special files.
//! * [`tools`]: where each toolchain comes from and how it is verified.
//!
//! pre-commit 4.6.2 verifies none of its toolchain downloads (D-SEC-08, `docs/divergences/D-SEC-08.md`).
//!
//! Owner: WP-16.

pub mod archive;
mod error;
mod fetch;
mod generated;
mod http;
mod policy;
pub mod tools;
mod util;

pub use error::{Checksum, NetError};
pub use fetch::{DOWNLOAD_CAP, Fetch, HttpFetch};
