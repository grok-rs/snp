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
use snp_core::cancel::CancelToken;
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
    // Only help and usage texts are wrapped to the terminal: a lone `--version`/`-V` skips the TIOCGWINSZ probe
    // (§12.2 T1, B1's syscall ceiling).
    let version_only = matches!(args, [a] if a == "--version" || a == "-V");
    panic::catch_unwind(AssertUnwindSafe(|| {
        let cols = if version_only {
            None
        } else {
            snp_os::term::stdout_tty_columns()
        };
        snp_cli::parse(brand, args, env, cols)
    }))
    .ok()
}

/// Help and version texts (argparse's `_print_message(message, sys.stdout)`, exit 0) need no session: they are
/// written before the cwd, `current_exe` and the tty probe (§12.2 T1 "`--version` before any init").
fn print_stdout_only(text: &str, brand: Brand, cancel: &CancelToken) -> Exit {
    let printer = Printer::new(
        Box::new(envelope::FailClosedStdout(std::io::stdout())),
        Box::new(std::io::stderr()),
        PrinterOpts {
            brand,
            color: false,
            stdout_is_tty: false,
            windows_text_mode: cfg!(windows),
            cancel: cancel.clone(),
        },
    );
    match text.strip_suffix('\n') {
        Some(t) => printer.py_print(t),
        None => printer.write(text.as_bytes()),
    }
    envelope::success(0, &printer, cancel)
}

/// `SNP_VERSION_PROBE=1 <file> --version` (exactly that argv): the brand-independent identification line that
/// `doctor` and `self alias` match against `^pre-commit [0-9.]+ \(snp ([^)]+)\)$` (11 §A.19.4). A plain `--version`
/// keeps the brand's exact text (`pre-commit 4.7.0` under the compat brand, ADR-023), which cannot tell an snp alias
/// from Python pre-commit; Python pre-commit ignores the variable.
fn is_version_probe(argv: &[OsString], env: &Env) -> bool {
    argv.len() == 2
        && argv.get(1).is_some_and(|a| a == "--version")
        && env.get("SNP_VERSION_PROBE").is_some_and(|v| v == "1")
}

/// `pre-commit 4.7.0 (snp 2.0.1)`.
fn version_probe_line() -> String {
    format!(
        "pre-commit {} (snp {})\n",
        snp_core::profile::PRE_COMMIT,
        snp_core::profile::SNP_VERSION
    )
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
    let parsed = if is_version_probe(&argv, &env) {
        Some(CliOutcome::Stdout(version_probe_line()))
    } else {
        parse(brand, &argv, &env)
    };
    // A panic while printing is reported like any other, with the full session (crash log).
    let mut stdout_panicked = false;
    if let Some(CliOutcome::Stdout(text)) = &parsed {
        match panic::catch_unwind(AssertUnwindSafe(|| print_stdout_only(text, brand, &cancel))) {
            Ok(exit) => return exit,
            Err(_) => stdout_panicked = true,
        }
    }
    let color = match &parsed {
        Some(CliOutcome::Cli(cli)) => snp_os::term::decide_color(color_setting(cli), &env),
        _ => false,
    };
    let printer = Printer::new(
        Box::new(envelope::FailClosedStdout(std::io::stdout())),
        Box::new(std::io::stderr()),
        PrinterOpts {
            brand,
            color,
            stdout_is_tty: snp_os::term::stdout_is_tty(),
            windows_text_mode: cfg!(windows),
            cancel: cancel.clone(),
        },
    );
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
    let outcome = parsed.filter(|_| !stdout_panicked).and_then(|p| {
        panic::catch_unwind(AssertUnwindSafe(|| dispatch::dispatch(p, &session))).ok()
    });
    let exit = match outcome {
        Some(Ok(code)) => envelope::success(code, &session.printer, &session.cancel),
        Some(Err(f)) => envelope::report(f, &session),
        None => envelope::report(panic_hook::take_failure(), &session),
    };
    // SIGTERM/SIGHUP: die by the re-raised signal after every restore (143/129 if the process survives it).
    if let Some(guard) = session.signals.get()
        && let Some(code) = guard.die_by_signal_if_terminated()
    {
        return envelope::exit(code);
    }
    exit
}
