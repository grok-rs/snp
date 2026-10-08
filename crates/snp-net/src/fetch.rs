//! The `Fetch` seam (S12) and its ureq implementation.
//!
//! Owner: WP-16.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-16")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-16")]

use std::fmt::Debug;
use std::path::{Path, PathBuf};

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_os as _;

use crate::{Checksum, NetError};

/// Seam for tests (S12).
pub trait Fetch: Send + Sync + Debug {
    /// A small text resource (checksum manifests, version JSON) of at most `max` bytes.
    fn get_text(&self, url: &str, max: u64) -> Result<String, NetError>;
    /// Streams to `dest_dir/<rand>.part`, hashing on the fly; renames to `file_name` only after the checksum matches.
    fn get_verified(
        &self,
        url: &str,
        sum: &Checksum,
        dest_dir: &Path,
        file_name: &str,
        cancel: &CancelToken,
    ) -> Result<PathBuf, NetError>;
}

/// `ureq::Agent`: rustls + platform verifier; env proxies; 30 s connect, 60 s idle, 3 retries.
#[derive(Debug)]
pub struct HttpFetch {
    /// W0 placeholder; WP-16 stores the agent and the env-derived policy.
    _priv: (),
}

impl HttpFetch {
    /// `SSL_CERT_FILE`/`DIR`, `HTTPS_PROXY`/`NO_PROXY`, `SNP_OFFLINE`, `SNP_ALLOW_INSECURE_MIRROR`.
    pub fn from_env(env: &Env) -> Self {
        todo!()
    }
}

impl Fetch for HttpFetch {
    fn get_text(&self, url: &str, max: u64) -> Result<String, NetError> {
        todo!()
    }

    fn get_verified(
        &self,
        url: &str,
        sum: &Checksum,
        dest_dir: &Path,
        file_name: &str,
        cancel: &CancelToken,
    ) -> Result<PathBuf, NetError> {
        todo!()
    }
}
