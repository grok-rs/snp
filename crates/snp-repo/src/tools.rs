//! `snp_lang::ToolDirs` over the store.
//!
//! Owner: WP-31.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-31")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-31")]

use std::path::PathBuf;

use snp_core::cancel::CancelToken;
use snp_core::failure::Failure;
use snp_lang::ToolDirs;
use snp_os::lock::FileLock;
use snp_store::Store;

/// `snp_lang::ToolDirs` over the store (tools-v1 dirs and locks, cache-v1 dirs).
#[derive(Debug, Clone, Copy)]
pub struct StoreTools<'a>(pub &'a Store);

impl ToolDirs for StoreTools<'_> {
    fn tool_dir(&self, tool: &str, version: &str) -> PathBuf {
        todo!()
    }

    fn lock_tool(&self, tool: &str, c: &CancelToken) -> Result<FileLock, Failure> {
        todo!()
    }

    fn cache_dir(&self, name: &str) -> PathBuf {
        todo!()
    }
}
