//! `entry()`: argv capture, panic hook, `__hazmat` raw dispatch, brand, parse, dispatch, envelope (§10.1).
//!
//! Owner: WP-33 (W0 skeleton by WP-00).

use std::ffi::OsString;
use std::io::IsTerminal;
use std::panic::{self, AssertUnwindSafe};

use snp_core::brand::Brand;
use snp_core::env::Env;
use snp_core::fault;
use snp_core::output::{Printer, PrinterOpts};

use crate::{Exit, dispatch, envelope, panic_hook};

/// The whole program except `process::exit` (§10.1): every guard and destructor has run when it returns.
///
/// `pre_commit.main.main` + `error_handler.error_handler()`: a panic anywhere below `dispatch` becomes a
/// `PanicException` envelope with exit 3 (101 is never an exit code, ADR-015).
pub fn entry() -> Exit {
    let argv: Vec<OsString> = std::env::args_os().collect(); // the only argv read
    let cancel = panic_hook::install();
    if argv.get(1).is_some_and(|a| a == "__hazmat") {
        // No store, no logging, no parser (03 H.8.3).
        let env = Env::capture_process();
        let rest = argv.get(2..).unwrap_or_default();
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            snp_hooks::hazmat::in_hook_main(rest, &env)
        }));
        return envelope::hazmat_exit(outcome.ok(), &cancel);
    }
    let brand = Brand::detect(argv.first().map(OsString::as_os_str));
    let env = Env::capture_process();
    fault::arm_from_env(&env);
    let printer = Printer::stdio(PrinterOpts {
        brand,
        color: false,
        stdout_is_tty: std::io::stdout().is_terminal(),
        windows_text_mode: cfg!(windows),
        cancel: cancel.clone(),
    });
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        dispatch::dispatch(brand, &argv, &env, &printer, &cancel)
    }));
    let failure = match outcome {
        Ok(Ok(code)) => return envelope::success(code, &printer),
        Ok(Err(f)) => f,
        Err(_payload) => panic_hook::take_failure(),
    };
    envelope::report(&failure, &printer)
}
