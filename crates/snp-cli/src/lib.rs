//! pre-commit 4.6.2 and snp-brand `ParserSpec` tables (verbatim help strings) and the typed [`Cli`] facade
//! (ARCHITECTURE §1.4.4, 12 §A.8.1, §9.3).
//!
//! Owner: WP-12.

mod args;
mod parse;
mod specs;

pub use args::{
    AutoupdateArgs, CacheAction, CacheArgs, Cli, CliOutcome, ColorArgs, ConfigArgs, ConfigCmd,
    DoctorArgs, HazmatArgs, HazmatTool, HookImplArgs, InitTemplatedirArgs, InstallArgs,
    InstallHooksArgs, ListArgs, MigrateArgs, RunArgs, RunExt, SelfArgs, Shell, StdinFormat,
    TryRepoArgs, UninstallArgs, ValidateArgs,
};
pub use parse::parse;
pub use specs::{COMPLETIONS, PRE_COMMIT_SPEC, SNP_SPEC};
