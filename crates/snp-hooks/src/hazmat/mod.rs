//! `hazmat` in-hook (`__hazmat`, path H) and CLI (path C) entry points (`commands/hazmat.py`; ADR-006).
//!
//! Owner: WP-26.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-26")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-26")]

use std::ffi::OsString;

use snp_argparse::ParserSpec;
use snp_cli::HazmatTool;
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::failure::Failure;

/// W0 placeholder tree (no actions); WP-26 transcribes `hazmat.py`'s parser.
static HAZMAT_PY: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &[],
    allow_abbrev: true,
    add_help: true,
};

/// `hazmat.py`'s parser (prog `hazmat.py`, CPython-3.13 texts).
pub static HAZMAT_PY_SPEC: &ParserSpec = &HAZMAT_PY;

/// `snp __hazmat …` (path H, a subprocess per partition): prog `hazmat.py`, CPython-3.13 texts, exit 0/1/2/child.
pub fn in_hook_main(args: &[OsString], env: &Env) -> i32 {
    todo!()
}

/// Public `{prog} hazmat …` (path C): `ExecutableNotFoundError` → `Failure` (exit 3).
pub fn cli(tool: &HazmatTool, env: &Env, cancel: &CancelToken) -> Result<i32, Failure> {
    todo!()
}
