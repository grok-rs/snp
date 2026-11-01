//! The single integration-test binary of `snp` (ARCHITECTURE §13.1 L2): one module per W3 owner. Tests reach
//! `snp`/`snp-probe` through `snp_testkit::Bins`, run in the hermetic `snp_testkit::TestEnv`, and every catalog row
//! is a test named with its namespaced ID (`t10_run_07_…`, `x13_eh_03_…`, `m11_mig_01_…`; §13.8). The modules are
//! declared `#[cfg(test)]` so their `#[test]` functions and helpers count as test code for clippy.
//!
//! Owner: WP-00 (this file); each module file belongs to the package named next to its declaration.

// The shared test harness of every module (`cargo machete --with-metadata` needs a real reference).
use snp_testkit as _;

#[cfg(test)]
mod alias; // WP-33
#[cfg(test)]
mod autoupdate; // WP-33
#[cfg(test)]
mod budgets; // WP-36
#[cfg(test)]
mod cache; // WP-33
#[cfg(test)]
mod clean; // WP-33
#[cfg(test)]
mod cli; // WP-33
#[cfg(test)]
mod config; // WP-33
#[cfg(test)]
mod doctor; // WP-33
#[cfg(test)]
mod errors; // WP-33
#[cfg(test)]
mod extensions; // WP-32
#[cfg(test)]
mod gc; // WP-33
#[cfg(test)]
mod hazmat_cli; // WP-33
#[cfg(test)]
mod hook_impl; // WP-32
#[cfg(test)]
mod init_templatedir; // WP-33
#[cfg(test)]
mod install; // WP-33
#[cfg(test)]
#[cfg(unix)]
mod interactive; // WP-32 (ADR-085)
#[cfg(test)]
mod list; // WP-32
#[cfg(test)]
mod meta; // WP-32
#[cfg(test)]
mod migrate_config; // WP-33
#[cfg(test)]
mod migrate_lefthook; // WP-34
#[cfg(test)]
mod output; // WP-32
#[cfg(test)]
mod repository; // WP-31
#[cfg(test)]
mod run; // WP-32
#[cfg(test)]
mod sample_config; // WP-33
#[cfg(test)]
mod stash; // WP-32
#[cfg(test)]
mod try_repo; // WP-32
#[cfg(test)]
mod uninstall; // WP-33
#[cfg(test)]
mod validate; // WP-33
#[cfg(test)]
mod xargs; // WP-32
