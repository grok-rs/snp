//! Parsing and the top-level dispatch (§10.1, §9): `snp_cli::parse` → help/version text, a usage error, or a
//! [`Cli`](snp_cli::Cli) handed to the command table in `commands`.
//!
//! Owner: WP-33 (W0 skeleton by WP-00).

use std::ffi::OsString;

use snp_cli::CliOutcome;
use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::output::Printer;

use crate::commands::{self, Cx};

/// Parses `argv[1..]` for `brand` and runs the selected command; `Ok(code)` is the command's exit status.
pub(crate) fn dispatch(
    brand: Brand,
    argv: &[OsString],
    env: &Env,
    printer: &Printer,
    cancel: &CancelToken,
) -> Result<i32, Failure> {
    let args = argv.get(1..).unwrap_or_default();
    if let Some(line) = w0_version(brand, args) {
        printer.py_print(&line);
        return Ok(0);
    }
    match snp_cli::parse(brand, args, env, snp_os::term::stdout_tty_columns()) {
        CliOutcome::Cli(cli) => {
            let cx = Cx {
                brand,
                env,
                printer,
                cancel,
                arg0: argv.first().map(OsString::as_os_str),
            };
            commands::run(cli, &cx)
        }
        CliOutcome::Stdout(text) => {
            // argparse's `_print_message(message, sys.stdout)`: the stdout text layer.
            match text.strip_suffix('\n') {
                Some(t) => printer.py_print(t),
                None => printer.write(text.as_bytes()),
            }
            Ok(0)
        }
        CliOutcome::Usage(stderr) => Err(Failure::Usage { stderr }),
        _ => Err(Failure::unexpected(
            "NotImplementedError",
            "unhandled parser outcome",
        )),
    }
}

/// W0 shim until `snp_cli::parse` lands (WP-12; WP-33 deletes it): `-V`/`--version` as the first argument is
/// argparse's `version` action, which prints `%(prog)s VERSION` and exits 0 before anything after it is parsed —
/// `pre-commit 4.6.2` (compat brand) / `snp 2.0.0` (snp brand, ADR-023).
fn w0_version(brand: Brand, args: &[OsString]) -> Option<String> {
    let first = args.first()?;
    (first == "-V" || first == "--version").then(|| brand.version_line())
}
