//! The run pipeline, scheduler, executor, renderer, hook-impl, try-repo, install-hooks, list and the meta hooks
//! (ARCHITECTURE §1.4.21, §3, §4.4; pre-commit's `commands/run.py`, `hook_impl.py`, `try_repo.py`,
//! `install_uninstall.py:install_hooks`).
//!
//! Owner: WP-32.

mod ctx;
mod doctor;
mod hook_impl;
mod install_hooks;
mod list;
pub mod render;
mod run;
mod try_repo;

use snp_config as _;
use snp_files as _;
use snp_hooks as _;
use snp_os as _;
use snp_pycompat as _;
use snp_stash as _;
use snp_yaml as _;

pub use ctx::RunCtx;
pub use doctor::doctor_fast_path;
pub use hook_impl::hook_impl;
pub use install_hooks::install_hooks;
pub use list::list;
pub use run::run;
pub use try_repo::try_repo;
