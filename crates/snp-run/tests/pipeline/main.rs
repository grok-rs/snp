//! End-to-end tests of the run pipeline through `snp_run::{run, hook_impl, install_hooks, list}` against real git
//! repositories, with a fake `Resolve` for `repo: local` / `repo: meta` (WP-31's resolver is exercised by the binary
//! tests in `crates/snp/tests/it`). Catalog rows are named after their IDs (`t10_run_10_…`, ARCHITECTURE §13.8).
//!
//! Owner: WP-32.
#![cfg(unix)]

// declared `#[cfg(test)]` so their `#[test]` functions and helpers count as test code for clippy
#[cfg(test)]
mod harness;

#[cfg(test)]
mod extensions;
#[cfg(test)]
mod hook_impl;
#[cfg(test)]
mod meta;
#[cfg(test)]
mod prefetch;
#[cfg(test)]
mod run;
#[cfg(test)]
mod try_repo;
