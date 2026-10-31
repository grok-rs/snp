//! Native hooks: the `NativeHook` framework, pygrep, fail, hazmat (in-hook and CLI), the `repo: builtin` variants
//! and the pre-commit-hooks ports (P0/P1/P2/caveats) (ARCHITECTURE §1.4.19, §7; 09).
//!
//! Owner: WP-26 (`lib.rs`, `hook.rs`, `registry.rs`, `framework/`, `pygrep/`, `fail.rs`, `hazmat/`, `builtin/`,
//! `pch/mod.rs`, `pch/p0/`, `pch/stubs/`); WP-27 owns `pch/p1/`, `pch/p2/` and `pch/caveats/`.

mod builtin;
pub mod fail;
pub mod framework;
pub mod hazmat;
mod hook;
pub mod pch;
pub mod pygrep;
mod registry;

use cfgv as _;
use file_identify as _;

pub use hook::{HookCx, NativeHook, Outcome};
pub use registry::native;
/// argparse's column rule for native-hook help/errors (`COLUMNS` via Python `int()`, else 80: a hook's stdout is a
/// pipe or a 0×0 pty); re-exported so snp-run needs no snp-argparse edge. Natives call `columns(env, None)`.
pub use snp_argparse::columns;
pub use snp_hooks_data::{Epoch, NativeSource};
