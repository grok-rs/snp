//! Non-run commands: install/uninstall/init-templatedir (hook scripts), autoupdate, gc/clean, validate-*,
//! migrate-config, sample-config, config dump/schema, cache, doctor and self (ARCHITECTURE §1.4.22; pre-commit's
//! `commands/*.py`; 11 §A.5–§A.19).
//!
//! Owner: WP-33.

mod autoupdate;
mod config_cmds;
mod ctx;
mod doctor;
mod install;
pub mod scripts;
mod self_cmd;
mod store_cmds;

use snp_config as _;
use snp_hooks_data as _;
use snp_os as _;
use snp_pycompat as _;
use snp_pyre as _;
use snp_stash as _;
use snp_yaml as _;

pub use autoupdate::autoupdate;
pub use config_cmds::{config, migrate_config, sample_config, validate_config, validate_manifest};
pub use ctx::CmdCtx;
pub use doctor::{DoctorCheck, DoctorLine, doctor};
pub use install::{init_templatedir, install, uninstall};
pub use self_cmd::self_cmd;
pub use store_cmds::{cache, clean, gc};
