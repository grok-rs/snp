//! `entry()`: argv capture, panic hook, `__hazmat` raw dispatch, brand, parse, printer, dispatch, envelope,
//! die-by-signal (§10.1).
//!
//! Owner: WP-33 (W0 skeleton by WP-00).

use std::ffi::OsString;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::OnceLock;

use snp_cli::{Cli, CliOutcome};
use snp_core::brand::Brand;
use snp_core::env::Env;
use snp_core::fault;
use snp_core::output::{ColorSetting, Printer, PrinterOpts};

use crate::session::Session;
use crate::{Exit, dispatch, envelope, panic_hook};

/// The `--color` value of a parsed command (`help`, `list` and the snp-only commands without the option use
/// `auto`, which is also pre-commit's default when `PRE_COMMIT_COLOR` is unset).
fn color_setting(cli: &Cli) -> ColorSetting {
    match cli {
        Cli::Run(a) => a.color,
        Cli::TryRepo(a) => a.run.color,
        Cli::HookImpl(a) => a.color,
        Cli::Install(a) => a.color,
        Cli::Uninstall(a) => a.color,
        Cli::InstallHooks(a) => a.color,
        Cli::InitTemplatedir(a) => a.color,
        Cli::Autoupdate(a) => a.color,
        Cli::Clean(a) | Cli::Gc(a) | Cli::SampleConfig(a) => a.color,
        Cli::MigrateConfig(a) => a.color,
        Cli::ValidateConfig(a) | Cli::ValidateManifest(a) => a.color,
        Cli::Hazmat(a) => a.color,
        _ => ColorSetting::Auto,
    }
}

/// `snp_cli::parse` under its own `catch_unwind` (the parser is pure and never touches disk).
fn parse(brand: Brand, argv: &[OsString], env: &Env) -> Option<CliOutcome> {
    let args = argv.get(1..).unwrap_or_default();
    panic::catch_unwind(AssertUnwindSafe(|| {
        snp_cli::parse(brand, args, env, snp_os::term::stdout_tty_columns())
    }))
    .ok()
}

/// The whole program except `process::exit` (§10.1): every guard and destructor has run when it returns.
///
/// `pre_commit.main.main` + `error_handler.error_handler()`: a panic anywhere below `dispatch` becomes a
/// `PanicException` envelope with exit 3 (101 is never an exit code, ADR-015); SIGTERM/SIGHUP end the process by
/// the re-raised signal once everything was restored (ADR-026).
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
    let arg0 = argv.first().cloned();
    let brand = Brand::detect(arg0.as_deref());
    let env = Env::capture_process();
    fault::arm_from_env(&env);
    #[cfg(feature = "testing")]
    snp_proc::spawn_log::enable_from_env(&env);
    let parsed = parse(brand, &argv, &env);
    let color = match &parsed {
        Some(CliOutcome::Cli(cli)) => snp_os::term::decide_color(color_setting(cli), &env),
        _ => false,
    };
    let printer = Printer::stdio(PrinterOpts {
        brand,
        color,
        stdout_is_tty: snp_os::term::stdout_is_tty(),
        windows_text_mode: cfg!(windows),
        cancel: cancel.clone(),
    });
    let self_exe = std::env::current_exe().unwrap_or_else(|_| {
        arg0.as_ref()
            .map_or_else(|| PathBuf::from(brand.prog()), PathBuf::from)
    });
    let session = Session {
        brand,
        env,
        cwd: std::env::current_dir().ok(),
        arg0,
        printer,
        cancel,
        self_exe,
        signals: OnceLock::new(),
        git_version: OnceLock::new(),
    };
    let outcome = match parsed {
        Some(p) => panic::catch_unwind(AssertUnwindSafe(|| dispatch::dispatch(p, &session))),
        None => Err(Box::new("parser panic") as Box<dyn std::any::Any + Send>),
    };
    let exit = match outcome {
        Ok(Ok(code)) => envelope::success(code, &session.printer, &session.cancel),
        Ok(Err(f)) => envelope::report(f, &session),
        Err(_payload) => envelope::report(panic_hook::take_failure(), &session),
    };
    // SIGTERM/SIGHUP: die by the re-raised signal after every restore (143/129 if the process survives it).
    if let Some(guard) = session.signals.get()
        && let Some(code) = guard.die_by_signal_if_terminated()
    {
        return envelope::exit(code);
    }
    exit
}
