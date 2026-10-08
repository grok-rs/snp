//! Content-addressed repository keys, repository identity and markers (§5.2–§5.3, ADR-032, D-DATA-08).
//!
//! Owner: WP-22.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-22")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-22")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The key of a materialised repository (`repos-v1/<hex>`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RepoKey(pub [u8; 16]);

impl RepoKey {
    /// `hex(32)`, the on-disk name.
    pub fn hex(&self) -> String {
        todo!()
    }
}

/// What a `repo:` value names.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum RepoId {
    /// A URL (or anything that is not an existing local directory), verbatim.
    Url(Arc<str>),
    /// An existing local directory: its canonical absolute path (D-DATA-08).
    LocalPath(PathBuf),
}

impl RepoId {
    /// The single derivation used by snp-repo (resolution) and snp-cmd (gc marking): `repo` names an existing local
    /// directory (relative values resolved against `project_root` = `ConfigSource::project_root` at run time, the
    /// recorded project root of a tracked config at gc time) ⇒ `LocalPath(realpath)`, else `Url(repo verbatim)`
    /// (ADR-070).
    pub fn from_config(repo: &str, project_root: &Path) -> Self {
        todo!()
    }
}

/// `store.db_repo_name`: `repo` or `repo:dep1,dep2` (deps in config order, no sorting); local repos use
/// `local[:deps]`.
pub fn db_repo_name(repo: &str, deps: &[Arc<str>]) -> String {
    todo!()
}

/// A shared toolchain an env links to (store-owned reference format; `sweep` reads only these, never
/// `InstallState`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ToolRef {
    /// The tool (`node`, `go`, `uv`, …).
    pub tool: Arc<str>,
    /// Its version.
    pub version: Arc<str>,
}

/// A tracked config and the project root its relative `repo:` paths resolve against (configs-v1 records both).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackedConfig {
    /// The config's realpath.
    pub path: PathBuf,
    /// Its project root.
    pub project_root: PathBuf,
}

/// Remote clone or the `repo: local` template.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum RepoKind {
    /// A clone of `id` at `rev`.
    Remote {
        /// The repository identity.
        id: RepoId,
        /// The revision.
        rev: Arc<str>,
    },
    /// The local-hooks template (`LOCAL_TEMPLATE_VERSION`).
    Local,
}

/// What `.snp-repo.json` records.
#[derive(Clone, Debug)]
pub struct RepoMeta {
    /// Remote or local.
    pub kind: RepoKind,
    /// `additional_dependencies` in config order.
    pub deps: Arc<[Arc<str>]>,
}

/// A materialised repository.
#[derive(Clone, Debug)]
pub struct RepoHandle {
    /// Its key.
    pub key: RepoKey,
    /// The checkout (pre-commit's `prefix`).
    pub src: PathBuf,
    /// The environments directory.
    pub envs: PathBuf,
    /// The recorded metadata.
    pub meta: RepoMeta,
}

/// Message-parity markers (ADR-032): `Initializing environment for {db_repo_name}.` and the virtual install
/// triplet.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum MarkerKey {
    /// pre-commit would have cloned `db_repo_name` at `rev` (keys-v1).
    Initialized {
        /// `store.db_repo_name`.
        db_repo_name: Arc<str>,
        /// The revision.
        rev: Arc<str>,
    },
    /// pre-commit would have installed this environment.
    Installed {
        /// The install key digest.
        install_key: [u8; 16],
    },
}
