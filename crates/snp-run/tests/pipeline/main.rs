//! End-to-end tests of the run pipeline through `snp_run::{run, hook_impl, install_hooks, list}` against real git
//! repositories, with a fake `Resolve` for `repo: local` / `repo: meta` (WP-31's resolver is exercised by the binary
//! tests in `crates/snp/tests/it`). Catalog rows are named after their IDs (`t10_run_10_…`, ARCHITECTURE §13.8).
//!
//! Owner: WP-32.
#![cfg(unix)]

mod harness;

mod extensions;
mod hook_impl;
mod meta;
mod run;
