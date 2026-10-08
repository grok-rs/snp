//! Health, install outcomes and the persisted install state (ADR-012, X13-ENV-01).
//!
//! Owner: WP-16.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-16")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-16")]

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json as _;

/// `health_check` result.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Health {
    /// Usable.
    Healthy,
    /// Needs a reinstall, with pre-commit's reason text.
    Unhealthy(String),
}

/// `tools`: the shared toolchains this env links to (`("node", "22.11.0")`, `("uv", …)`) — passed to
/// `Store::commit_install_marker` as store-owned references, so `sweep` never parses `InstallState` (ADR-070).
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// Installer that built the env (`uv`, `pip`, `npm`, …).
    pub installer: Arc<str>,
    /// Fingerprint of the env's interpreter.
    pub fingerprint: Option<Fingerprint>,
    /// (tool, version) references.
    pub tools: Vec<(Arc<str>, Arc<str>)>,
}

/// Persisted as `.snp-install-state.json` (committed last by snp-store); malformed ⇒ not installed (X13-ENV-01).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallState {
    /// Format version.
    pub v: u32,
    /// Language name.
    pub language: Arc<str>,
    /// `language_version`.
    pub language_version: Arc<str>,
    /// `additional_dependencies`.
    pub deps: Arc<[Arc<str>]>,
    /// Installer that built the env.
    pub installer: Arc<str>,
    /// Fingerprint of the env's interpreter.
    pub fingerprint: Option<Fingerprint>,
    /// snp version that wrote the state.
    pub snp_version: Arc<str>,
}

impl InstallState {
    /// JSON bytes of the state file.
    pub fn to_json(&self) -> Vec<u8> {
        todo!()
    }

    /// Parses a state file; None when malformed.
    pub fn from_json(b: &[u8]) -> Option<Self> {
        todo!()
    }
}

/// (len, mtime, ino/index) of an executable for health fast paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    /// The executable.
    pub exe: PathBuf,
    /// File length.
    pub len: u64,
    /// mtime in nanoseconds.
    pub mtime_ns: i128,
    /// inode (Unix) / file index (Windows).
    pub ino_or_index: u64,
    /// Probe detail (e.g. the interpreter version line).
    pub detail: Arc<str>,
}
