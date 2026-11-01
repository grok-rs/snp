//! `snp_lang::ToolDirs` over the store.
//!
//! Owner: WP-31.

use std::path::PathBuf;

use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::failure::Failure;
use snp_core::output::{Level, Printer};
use snp_lang::ToolDirs;
use snp_os::lock::FileLock;
use snp_store::Store;

/// `snp_lang::ToolDirs` over the store (tools-v1 dirs and locks, cache-v1 dirs).
///
/// The shared toolchains (uv, node, go, rustup, ruby-build) are snp's own (pre-commit downloads per environment or
/// uses the system ones), so their directories live in the store's `tools-v1/<tool>/<version>` and are bootstrapped
/// under the exclusive `tools-v1/<tool>.lock` (ARCHITECTURE §5.2, §5.5): one download per tool and version across
/// processes. An ephemeral (try-repo) store resolves both to its main store (ADR-059).
///
/// `StoreTools` alone has no printer, so its lock waits silently; the resolver hands the language backends a
/// wrapper that prints the brand's `[INFO] Locking … directory` line once per process on contention
/// (`store.py:105-106`), as pre-commit does for its store lock.
#[derive(Debug, Clone, Copy)]
pub struct StoreTools<'a>(pub &'a Store);

impl ToolDirs for StoreTools<'_> {
    /// `tools-v1/<tool>/<version>` (an unsafe name is hashed, never a path).
    fn tool_dir(&self, tool: &str, version: &str) -> PathBuf {
        self.0.tool_dir(tool, version)
    }

    /// `tools-v1/<tool>.lock`, exclusive; the wait observes `c` every 50 ms.
    ///
    /// # Errors
    ///
    /// `Failure::Interrupted` when cancelled while waiting; the `OSError` of the lock file otherwise.
    fn lock_tool(&self, tool: &str, c: &CancelToken) -> Result<FileLock, Failure> {
        self.0.tool_lock(tool, &mut || {}, c)
    }

    /// `cache-v1/<name>` (isolated tool caches: uv, go, npm, …).
    fn cache_dir(&self, name: &str) -> PathBuf {
        self.0.cache_dir(name)
    }
}

/// The `ToolDirs` the resolver gives the language backends during installs: the run's own `ToolDirs` for the
/// directories, the store's tool lock with the `Locking … directory` notice (once per process across every store
/// lock, `snp_store::locking_message`).
#[derive(Debug)]
pub(crate) struct NoticeTools<'a> {
    /// The run's directories.
    pub(crate) inner: &'a dyn ToolDirs,
    /// The store whose tool locks are taken.
    pub(crate) store: &'a Store,
    /// Where the notice goes.
    pub(crate) printer: &'a Printer,
    /// The brand of the notice.
    pub(crate) brand: Brand,
}

impl ToolDirs for NoticeTools<'_> {
    fn tool_dir(&self, tool: &str, version: &str) -> PathBuf {
        self.inner.tool_dir(tool, version)
    }

    fn lock_tool(&self, tool: &str, c: &CancelToken) -> Result<FileLock, Failure> {
        let (printer, brand) = (self.printer, self.brand);
        self.store.tool_lock(
            tool,
            &mut || printer.log(Level::Info, snp_store::locking_message(brand)),
            c,
        )
    }

    fn cache_dir(&self, name: &str) -> PathBuf {
        self.inner.cache_dir(name)
    }
}
