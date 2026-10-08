//! The store handle: lazy creation, locks, materialisation, install markers, tracking, sweep (§5).
//!
//! Owner: WP-22.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-22")]
#![expect(
    unused_variables,
    clippy::needless_pass_by_value,
    reason = "W0 stub parameters, removed by WP-22"
)]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::failure::Failure;
use snp_core::lang::LanguageId;
use snp_core::output::DiagSink;
use snp_os::lock::FileLock;

use crate::{
    MarkerKey, RepoHandle, RepoId, RepoKey, RepoMeta, StoreLocation, ToolRef, TrackedConfig,
};

/// The held global store lock (shared for runs, exclusive for gc/clean).
#[derive(Debug)]
pub struct StoreGuard(
    /// W0 placeholder; WP-22 stores the held `FileLock`.
    (),
);

/// snp's store (`~/.cache/snp`).
#[derive(Debug)]
pub struct Store {
    /// W0 placeholder; WP-22 stores the location, a lazy `OnceLock<Created>` and the read-only flag.
    _priv: (),
}

impl Store {
    /// No I/O (lazy, 03 Q12).
    pub fn new(loc: StoreLocation) -> Self {
        todo!()
    }

    /// try-repo's throwaway store rooted in its `TempDir` (ADR-059): repos-v1, keys-v1, configs-v1, patches and the
    /// global lock live under `root`; tools-v1 and cache-v1 come from `shared` (invisible: tool downloads are snp's
    /// own).
    pub fn ephemeral(root: &Path, shared: Option<&Self>) -> Self {
        todo!()
    }

    /// Where the store lives.
    pub fn location(&self) -> &StoreLocation {
        todo!()
    }

    /// Whether the store directory exists.
    pub fn exists(&self) -> bool {
        todo!()
    }

    /// 0700, README, store-v1.json.
    pub fn ensure_created(&self, brand: Brand, diag: &dyn DiagSink) -> Result<(), Failure> {
        todo!()
    }

    /// The shared global lock (runs using repos).
    pub fn shared(
        &self,
        on_wait: &mut dyn FnMut(),
        c: &CancelToken,
    ) -> Result<StoreGuard, Failure> {
        todo!()
    }

    /// The exclusive global lock (gc/clean).
    pub fn exclusive(
        &self,
        on_wait: &mut dyn FnMut(),
        c: &CancelToken,
    ) -> Result<StoreGuard, Failure> {
        todo!()
    }

    /// The key of a remote repository at `rev` with `deps`.
    pub fn remote_key(id: &RepoId, rev: &str, deps: &[Arc<str>]) -> RepoKey {
        todo!()
    }

    /// Includes `snp_core::profile::LOCAL_TEMPLATE_VERSION`.
    pub fn local_key(deps: &[Arc<str>]) -> RepoKey {
        todo!()
    }

    /// One stat of the marker; self-heals.
    pub fn lookup(&self, key: &RepoKey) -> Option<RepoHandle> {
        todo!()
    }

    /// Under the per-repo lock: re-lookup; else `fill(scratch/<rand>/src)`, write `.snp-repo.json`, rename into place.
    pub fn materialize(
        &self,
        key: &RepoKey,
        meta: RepoMeta,
        fill: &mut dyn FnMut(&Path) -> Result<(), Failure>,
        on_wait: &mut dyn FnMut(),
        c: &CancelToken,
    ) -> Result<RepoHandle, Failure> {
        todo!()
    }

    /// The per-repository lock.
    pub fn repo_lock(
        &self,
        key: &RepoKey,
        on_wait: &mut dyn FnMut(),
        c: &CancelToken,
    ) -> Result<FileLock, Failure> {
        todo!()
    }

    /// `tools-v1/<tool>.lock`.
    pub fn tool_lock(
        &self,
        tool: &str,
        on_wait: &mut dyn FnMut(),
        c: &CancelToken,
    ) -> Result<FileLock, Failure> {
        todo!()
    }

    /// Sanitized-or-hashed environment directory (§5.4).
    pub fn env_dir(&self, repo: &RepoHandle, lang: LanguageId, version: &str) -> PathBuf {
        todo!()
    }

    /// `.snp-install-state.json`.
    pub fn read_install_marker(env_dir: &Path) -> Option<Vec<u8>> {
        todo!()
    }

    /// Writes the store-owned `.snp-tools.json` sidecar (`tools`), then `.snp-install-state.json` (temp + fsync +
    /// rename: the commit point). `json` stays opaque to the store.
    pub fn commit_install_marker(env_dir: &Path, json: &[u8], tools: &[ToolRef]) -> io::Result<()> {
        todo!()
    }

    /// Whether a message-parity marker exists.
    pub fn marker_exists(&self, k: &MarkerKey) -> bool {
        todo!()
    }

    /// Best effort, `create_new`.
    pub fn set_marker(&self, k: &MarkerKey) {
        todo!()
    }

    /// Best effort, lock-free, never creates the store.
    pub fn mark_config_used(&self, cfg_realpath: &Path, project_root: &Path) {
        todo!()
    }

    /// Every tracked config (configs-v1).
    pub fn tracked_configs(&self) -> Vec<TrackedConfig> {
        todo!()
    }

    /// Forgets configs that no longer exist.
    pub fn forget_configs(&self, dead: &[PathBuf]) {
        todo!()
    }

    /// Every materialised repository.
    pub fn all_repos(&self) -> Vec<RepoHandle> {
        todo!()
    }

    /// Moves a repository to the trash.
    pub fn retire_repo(&self, key: &RepoKey) -> io::Result<()> {
        todo!()
    }

    /// Scratch/trash orphans, old restored patches, tools no `.snp-tools.json` references.
    pub fn sweep(&self, now: SystemTime) -> SweepReport {
        todo!()
    }

    /// Retires the whole store.
    pub fn clean(&self) -> Result<(), Failure> {
        todo!()
    }

    /// `patches/`.
    pub fn patches_dir(&self) -> PathBuf {
        todo!()
    }

    /// Output-budget spill files.
    pub fn spill_dir(&self) -> PathBuf {
        todo!()
    }

    /// `scratch/`.
    pub fn scratch_dir(&self) -> PathBuf {
        todo!()
    }

    /// `<store>/pre-commit.log` | `snp.log`.
    pub fn crash_log(&self, brand: Brand) -> PathBuf {
        todo!()
    }

    /// `tools-v1/<tool>/<version>`.
    pub fn tool_dir(&self, tool: &str, version: &str) -> PathBuf {
        todo!()
    }

    /// `cache-v1/<name>`.
    pub fn cache_dir(&self, name: &str) -> PathBuf {
        todo!()
    }

    /// `store-v1.json` path differs → the old path.
    pub fn relocated_from(&self) -> Option<PathBuf> {
        todo!()
    }
}

/// What `sweep` removed.
#[derive(Debug, Default)]
pub struct SweepReport {
    /// Orphaned scratch/trash directories.
    pub removed_dirs: usize,
    /// Old restored patches.
    pub removed_patches: usize,
    /// Whether the legacy v1 layout was removed.
    pub legacy_v1_removed: bool,
}
