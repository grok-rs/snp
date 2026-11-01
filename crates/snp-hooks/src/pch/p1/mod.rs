//! P1 natives: pretty-format-json, file-contents-sorter, requirements-txt-fixer, check-case-conflict,
//! check-shebang-scripts-are-executable, check-symlinks, destroyed-symlinks, the pre-v5.0.0 check-byte-order-marker
//! (ARCHITECTURE §7.4, 09 §1).
//!
//! Every hook reproduces its console script at every supported tag by epoch (`snp_hooks_data::Epoch`,
//! `generated/epochs.md`); the arguments are the console script's full argv (hook `args` + partition files) parsed
//! by `framework::parse_args` (ADR-072). The byte-level cores are public in [`kernels`] (fuzz targets and tests drive
//! them without a repository). `repo: builtin` ids of this group resolve to these natives at the v6.0.0 epoch.
//!
//! Owner: WP-27.

pub(crate) mod check_byte_order_marker;
pub(crate) mod check_case_conflict;
pub(crate) mod check_shebang_scripts_are_executable;
pub(crate) mod check_symlinks;
pub(crate) mod destroyed_symlinks;
pub(crate) mod file_contents_sorter;
pub mod kernels;
mod lower_table;
pub(crate) mod pretty_format_json;
pub(crate) mod requirements_txt_fixer;
pub(crate) mod util;

// `bstr` is pre-declared for this group (WBS "Integration-lead files"); the natives need only `memchr`, so it is
// anchored here until WP-26 drops it from the manifest.
use bstr as _;

use crate::NativeHook;

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[
    &pretty_format_json::UPSTREAM,
    &file_contents_sorter::UPSTREAM,
    &requirements_txt_fixer::UPSTREAM,
    &check_case_conflict::UPSTREAM,
    &check_shebang_scripts_are_executable::UPSTREAM,
    &check_symlinks::UPSTREAM,
    &destroyed_symlinks::UPSTREAM,
    &check_byte_order_marker::UPSTREAM,
];

/// The argparse parse of a P1 console script at `epoch` (`None` for a script that is not a P1 native): what the
/// native's `run` starts with (ADR-072), without a repository.
pub fn parse_console_script(
    script: &str,
    epoch: u16,
    argv: &[&std::ffi::OsStr],
    env: &snp_core::env::Env,
) -> Option<Result<snp_argparse::Namespace, crate::Outcome>> {
    let spec: &'static snp_argparse::ParserSpec = match script {
        "pretty-format-json" => &pretty_format_json::SPEC,
        "file-contents-sorter" => file_contents_sorter::spec(epoch),
        "requirements-txt-fixer" => &requirements_txt_fixer::SPEC,
        "check-case-conflict" => &check_case_conflict::SPEC,
        "check-shebang-scripts-are-executable" => &check_shebang_scripts_are_executable::SPEC,
        "check-symlinks" => &check_symlinks::SPEC,
        "destroyed-symlinks" => &destroyed_symlinks::SPEC,
        "check-byte-order-marker" => &check_byte_order_marker::SPEC,
        _ => return None,
    };
    Some(crate::framework::parse_args(spec, script, argv, 80, env))
}
