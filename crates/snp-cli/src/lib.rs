//! pre-commit 4.6.2 and snp-brand `ParserSpec` tables (verbatim help strings) and the typed [`Cli`] facade
//! (ARCHITECTURE §1.4.4, §9, 12 §A.8.1, COMPATIBILITY §3, §8.1).
//!
//! [`parse`] picks the tree by brand: the compat brand (`argv[0]` named `pre-commit`) accepts exactly
//! pre-commit's 16 commands and options with byte-identical help and errors; the snp brand adds the extension
//! commands and flags, guarded by `snp_argparse::prefix_stable` so every abbreviation pre-commit accepts keeps its
//! meaning (ADR-023, ADR-057, ADR-058).
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

#[cfg(test)]
mod tests;
