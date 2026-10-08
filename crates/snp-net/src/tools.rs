//! Toolchain catalogue: where to download and how to verify (§6.3).
//!
//! Owner: WP-16.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-16")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-16")]

use crate::{Checksum, Fetch, NetError};

/// A downloadable toolchain.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Tool {
    /// uv (Python environments).
    Uv,
    /// Node.js.
    Node,
    /// Go.
    Go,
    /// rustup-init.
    Rustup,
    /// ruby-build.
    RubyBuild,
    /// rbenv.
    Rbenv,
}

/// What to download and how to verify it.
#[derive(Clone, Debug)]
pub struct Artifact {
    /// Download URL.
    pub url: String,
    /// Expected digest.
    pub checksum: Checksum,
    /// How to unpack it.
    pub layout: Layout,
}

/// Archive layout of an [`Artifact`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Layout {
    /// `.tar.gz` with leading components stripped.
    TarGz {
        /// Components to strip.
        strip: u8,
    },
    /// `.tar.xz` with leading components stripped.
    TarXz {
        /// Components to strip.
        strip: u8,
    },
    /// `.zip` with leading components stripped.
    Zip {
        /// Components to strip.
        strip: u8,
    },
    /// A single executable.
    Executable,
}

/// Checksum manifests (`SHASUMS256.txt`, go.dev JSON, `.sha256`) are always fetched over https from the canonical
/// origin (or an explicit https `SNP_NODE_SHASUMS_URL`/`SNP_GO_JSON_URL`), never from an `http://` mirror allowed by
/// `SNP_ALLOW_INSECURE_MIRROR` — a non-TLS mirror then only serves bytes that the TLS manifest verifies.
pub fn resolve(
    tool: Tool,
    version: &str,
    target: &str,
    f: &dyn Fetch,
) -> Result<Artifact, NetError> {
    todo!()
}

/// The pinned uv version (+ embedded sha256 per target triple, generated table). W0 placeholder: WP-16 pins it.
pub const UV_PINNED: &str = "";

/// e.g. "x86_64-unknown-linux-gnu" (musl/riscv64 aware).
pub fn host_target() -> &'static str {
    todo!()
}
