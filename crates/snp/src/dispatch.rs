//! The parse outcomes and the top-level dispatch (§10.1, §9.4): help and `--version` to stdout (exit 0), a usage
//! error to stderr (exit 2), or a [`Cli`](snp_cli::Cli) handed to the command table in `commands`.
//!
//! Owner: WP-33 (W0 skeleton by WP-00).

use snp_cli::CliOutcome;
use snp_core::failure::Failure;

use crate::commands;
use crate::session::Session;

/// Runs the parsed command; `Ok(code)` is its exit status.
pub(crate) fn dispatch(outcome: CliOutcome, s: &Session) -> Result<i32, Failure> {
    match outcome {
        CliOutcome::Cli(cli) => commands::run(cli, s),
        CliOutcome::Stdout(text) => {
            // argparse's `_print_message(message, sys.stdout)`: the stdout text layer.
            match text.strip_suffix('\n') {
                Some(t) => s.printer.py_print(t),
                None => s.printer.write(text.as_bytes()),
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
