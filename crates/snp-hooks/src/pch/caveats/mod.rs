//! NATIVE-WITH-CAVEATS hooks behind their parity gates (09 §6.7): check-yaml, check-toml, check-xml, detect-aws-
//! credentials (ARCHITECTURE §7.4, 09 §1).
//!
//! The parsers are snp-pyparsers' ports of what the console scripts run (ruamel.yaml ≥ 0.19, CPython 3.13
//! `tomllib`, expat through `xml.sax`, `configparser`). Whether the runner uses them in `auto` mode is decided by
//! `snp_hooks_data::parity_gate`, the table `cargo xtask pch-gates` regenerates from the corpus comparison of
//! `tests/caveats` (zero acceptance differences required).
//!
//! Owner: WP-27.

pub(crate) mod check_toml;
pub(crate) mod check_xml;
pub(crate) mod check_yaml;
pub(crate) mod detect_aws_credentials;

use crate::NativeHook;

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[
    &check_yaml::UPSTREAM,
    &check_toml::UPSTREAM,
    &check_xml::UPSTREAM,
    &detect_aws_credentials::UPSTREAM,
];

/// The argparse parse of a caveat console script at `epoch` (`None` for a script that is not one): what the
/// native's `run` starts with (ADR-072), without a repository.
pub fn parse_console_script(
    script: &str,
    epoch: u16,
    argv: &[&std::ffi::OsStr],
    env: &snp_core::env::Env,
) -> Option<Result<snp_argparse::Namespace, crate::Outcome>> {
    let spec: &'static snp_argparse::ParserSpec = match script {
        "check-yaml" => check_yaml::spec(epoch),
        "check-toml" => &check_toml::SPEC,
        "check-xml" => &check_xml::SPEC,
        "detect-aws-credentials" => &detect_aws_credentials::SPEC,
        _ => return None,
    };
    Some(crate::framework::parse_args(spec, script, argv, 80, env))
}

/// `key.decode()[:4].ljust(28, '*')`: how detect-aws-credentials shows a secret it found.
pub fn hidden_key(key: &str) -> String {
    detect_aws_credentials::hidden(key)
}
