//! P2 natives: sort-simple-yaml, check-vcs-permalinks, forbid-new-submodules, name-tests-test, the pre-v5.0.0 fix-
//! encoding-pragma (ARCHITECTURE §7.4, 09 §1).
//!
//! Every hook reproduces its console script at every supported tag by epoch (`snp_hooks_data::Epoch`,
//! `generated/epochs.md`); the arguments are the console script's full argv (hook `args` + partition files) parsed
//! by `framework::parse_args` (ADR-072). The byte-level cores are public in [`kernels`].
//!
//! Owner: WP-27.

pub(crate) mod check_vcs_permalinks;
pub(crate) mod fix_encoding_pragma;
pub(crate) mod forbid_new_submodules;
pub mod kernels;
pub(crate) mod name_tests_test;
pub(crate) mod sort_simple_yaml;

use crate::NativeHook;

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[
    &sort_simple_yaml::UPSTREAM,
    &check_vcs_permalinks::UPSTREAM,
    &forbid_new_submodules::UPSTREAM,
    &name_tests_test::UPSTREAM,
    &fix_encoding_pragma::UPSTREAM,
];

/// The argparse parse of a P2 console script at `epoch` (`None` for a script that is not a P2 native, or one that
/// takes no arguments at that epoch): what the native's `run` starts with (ADR-072), without a repository.
pub fn parse_console_script(
    script: &str,
    epoch: u16,
    argv: &[&std::ffi::OsStr],
    env: &snp_core::env::Env,
) -> Option<Result<snp_argparse::Namespace, crate::Outcome>> {
    let (spec, prog): (&'static snp_argparse::ParserSpec, &str) = match script {
        "sort-simple-yaml" => (&sort_simple_yaml::SPEC, script),
        "check-vcs-permalinks" => (check_vcs_permalinks::spec(epoch), script),
        "forbid-new-submodules" if epoch >= 2 => (&forbid_new_submodules::SPEC, script),
        "name-tests-test" => (name_tests_test::spec(epoch), script),
        "fix-encoding-pragma" => (
            &fix_encoding_pragma::SPEC,
            "Fixes the encoding pragma of python files",
        ),
        _ => return None,
    };
    Some(crate::framework::parse_args(spec, prog, argv, 80, env))
}
