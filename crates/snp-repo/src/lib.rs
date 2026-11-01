//! Hook resolution behind the `Resolve` seam: clones with prefetch, manifest loading, merge, language defaults,
//! fast-path decisions and environment installs with ordered transcripts (ARCHITECTURE §1.4.20, §5.3, §7.2;
//! pre-commit's `repository.py`, `store.py`).
//!
//! Owner: WP-31.

mod fastpath;
mod install;
mod messages;
mod model;
mod prefetch;
mod resolve;
mod resolver;
mod seams;
mod stages;
mod tools;

use snp_files as _;
use snp_net as _;

pub use model::{
    ExecKind, FastPathPolicy, HookSrc, InstallKey, MetaHook, ResolveCtx, ResolvedHook,
};
pub use resolver::{StoreResolver, decision_table};
pub use seams::{Preflight, Resolve};
pub use tools::StoreTools;
