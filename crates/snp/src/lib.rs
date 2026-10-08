//! The snp binary library (ARCHITECTURE §1.4.24, §10.1): `entry()`, brand, `__hazmat` dispatch, parsing, dispatch,
//! the error envelope, the crash log, the panic guard and exit codes.
//!
//! Owner: WP-33 (W0 skeleton by WP-00).

mod commands;
mod dispatch;
mod entry;
mod envelope;
mod exit;
mod panic_hook;

#[cfg(feature = "dhat-heap")]
use dhat as _;
#[cfg(all(target_env = "musl", feature = "musl-mimalloc"))]
use mimalloc as _;
use snp_git as _;
use snp_migrate as _;
use snp_net as _;
use snp_proc as _;
use snp_pyre as _;
use snp_repo as _;
use snp_store as _;

pub use entry::entry;
pub use exit::Exit;
