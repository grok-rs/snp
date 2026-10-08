//! `ArgumentParser.parse_args` and `HelpFormatter` of CPython 3.13.14 (12 §A.8, §A.13, §A.16).
//!
//! Owner: WP-12.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-12")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-12")]

use std::ffi::OsString;

use snp_core::env::Env;
use snp_pycompat as _;

use crate::{Outcome, ParseCfg, ParserSpec};

/// argparse 3.13.14 `_parse_known_args` + `parse_args` (12 §A.8.2): unknown args deferred to the top level, ambiguity
/// at consumption, env-string default converted at the end of the owning subparser only when the option was absent.
pub fn parse(spec: &'static ParserSpec, argv: &[OsString], cfg: &ParseCfg<'_>) -> Outcome {
    todo!()
}

/// `parser.format_help()` of the parser at `path` (sub-command names from the top level).
pub fn format_help(spec: &'static ParserSpec, path: &[&str], cfg: &ParseCfg<'_>) -> String {
    todo!()
}

/// `parser.format_usage()` of the parser at `path`.
pub fn format_usage(spec: &'static ParserSpec, path: &[&str], cfg: &ParseCfg<'_>) -> String {
    todo!()
}

/// `COLUMNS` (Python `int()`), else the stdout terminal width, else 80 (the formatter subtracts 2).
pub fn columns(env: &Env, stdout_tty_cols: Option<usize>) -> usize {
    todo!()
}

/// Pure; every unique-prefix → option mapping of `base` must be unchanged in `extended` (12 §A.17 item 6).
pub fn prefix_stable(
    base: &'static ParserSpec,
    extended: &'static ParserSpec,
) -> Result<(), String> {
    todo!()
}

/// JSON description of a spec (prog included) from which `xtask cli-goldens` builds a real CPython argparse parser to
/// cross-check the snp-brand goldens (ADR-058).
pub fn export_json(spec: &'static ParserSpec, prog: &str) -> String {
    todo!()
}
