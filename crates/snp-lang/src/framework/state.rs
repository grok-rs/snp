//! Health, install outcomes and the persisted install state (ADR-012, X13-ENV-01).
//!
//! Owner: WP-16.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

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
///
/// Replaces pre-commit's `.install_state_v1` (`{"additional_dependencies": [...]}`) and the empty `.install_state_v2`
/// (`repository.py:93-104`), which snp never writes or reads (ADR-012): the state also records the resolved version,
/// the installer and the interpreter fingerprint of the spawn-free health fast path (04a §14.9).
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
    /// The format version this snp writes and accepts ([`InstallState::from_json`] rejects every other one, so a
    /// state written by a newer snp means "not installed" and the env is rebuilt).
    pub const VERSION: u32 = 1;

    /// JSON bytes of the state file.
    ///
    /// Compact `serde_json`; never fails (non-UTF-8 paths are encoded losslessly, see [`Fingerprint`]).
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Parses a state file; None when malformed.
    ///
    /// Malformed covers invalid JSON, missing or mistyped fields and any `v` other than [`InstallState::VERSION`]
    /// (X13-ENV-01: pre-commit crashes on a malformed `.install_state_v1`; snp reinstalls).
    pub fn from_json(b: &[u8]) -> Option<Self> {
        let s: Self = serde_json::from_slice(b).ok()?;
        (s.v == Self::VERSION).then_some(s)
    }
}

/// (len, mtime, ino/index) of an executable for health fast paths.
///
/// `exe` is stored as a JSON string when it is valid Unicode, else as `{"unix": [bytes]}` / `{"windows": [u16]}`,
/// so a non-UTF-8 interpreter path never makes the state unwritable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    /// The executable.
    #[serde(with = "os_path")]
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

/// Lossless (de)serialisation of a path (string when it is Unicode).
mod os_path {
    use std::path::{Path, PathBuf};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// The non-Unicode forms.
    #[derive(Serialize, Deserialize)]
    enum Raw {
        /// Unix bytes.
        #[serde(rename = "unix")]
        Unix(Vec<u8>),
        /// Windows UTF-16 units (possibly unpaired surrogates).
        #[serde(rename = "windows")]
        Windows(Vec<u16>),
    }

    /// Either form.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        /// Unicode.
        Str(String),
        /// Raw.
        Raw(Raw),
    }

    pub(super) fn serialize<S: Serializer>(p: &Path, s: S) -> Result<S::Ok, S::Error> {
        if let Some(text) = p.to_str() {
            return s.serialize_str(text);
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Raw::Unix(p.as_os_str().as_bytes().to_vec()).serialize(s)
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Raw::Windows(p.as_os_str().encode_wide().collect()).serialize(s)
        }
        #[cfg(not(any(unix, windows)))]
        {
            s.serialize_str(&p.to_string_lossy())
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
        match Repr::deserialize(d)? {
            Repr::Str(s) => Ok(PathBuf::from(s)),
            #[cfg(unix)]
            Repr::Raw(Raw::Unix(b)) => {
                use std::os::unix::ffi::OsStringExt;
                Ok(PathBuf::from(std::ffi::OsString::from_vec(b)))
            }
            #[cfg(windows)]
            Repr::Raw(Raw::Windows(w)) => {
                use std::os::windows::ffi::OsStringExt;
                Ok(PathBuf::from(std::ffi::OsString::from_wide(&w)))
            }
            Repr::Raw(_) => Err(serde::de::Error::custom("path from another OS")),
        }
    }
}
