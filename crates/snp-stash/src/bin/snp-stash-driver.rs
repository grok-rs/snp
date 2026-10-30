//! `snp-stash-driver` (feature `test-driver`, never shipped): drives `enter` → hook → `stage_fixed` → `restore`
//! under snp-proc's signal handling, the way `snp run` will (ARCHITECTURE §4.3–§4.4), for the process-level signal,
//! kill -9 and transcript matrices of WP-23 (`tests/driver.rs`).
//!
//! ```text
//! snp-stash-driver <repo> <patches-dir> [--color] [--stage-fixed <path>]… [--abort-on-fault] -- <hook argv…>
//! ```
//!
//! The hook runs with the run environment plus `_SNP_STASH_LOCK` (inherited stdio, `Role::Hook`, the run token).
//! `SNP_FAULT` arms a fault point; with `--abort-on-fault` an injected fault aborts the process (no unwinding, no
//! destructors: a kill -9 at that transition). `SNP_STASH_VERIFY`, `SNP_STASH_LOCK_TIMEOUT` and
//! `SNP_STASH_DRIVER_OLD_GIT=1` (git < 2.19) configure the context. Exit status: 0, the envelope's 1/3, 130 after
//! SIGINT, death by SIGTERM/SIGHUP (143/129) after the restore.
//!
//! Owner: WP-23.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use snp_core::brand::Brand;
use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::clock::SystemClock;
use snp_core::env::{Env, EnvPatch};
use snp_core::failure::{ExcKind, Failure};
use snp_core::output::{Printer, PrinterOpts};
use snp_core::path::RepoPath;
use snp_git::{Git, GitBackend, GitExe, ProcBackend};
use snp_proc::spawn::{self, Group, Role, Spec, Stdin};
use snp_stash::StashCtx;

/// Parsed arguments.
#[derive(Debug, Default)]
struct Args {
    repo: PathBuf,
    patches: PathBuf,
    color: bool,
    stage_fixed: Vec<RepoPath>,
    abort_on_fault: bool,
    hook: Vec<OsString>,
}

fn parse(mut it: impl Iterator<Item = OsString>) -> Option<Args> {
    let mut a = Args {
        repo: PathBuf::from(it.next()?),
        patches: PathBuf::from(it.next()?),
        ..Args::default()
    };
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--color") => a.color = true,
            Some("--abort-on-fault") => a.abort_on_fault = true,
            Some("--stage-fixed") => {
                let p = it.next()?;
                a.stage_fixed
                    .push(RepoPath::new(p.to_string_lossy().as_bytes())?);
            }
            Some("--") => {
                a.hook = it.collect();
                break;
            }
            _ => return None,
        }
    }
    Some(a)
}

/// The envelope's line 1 (+ the K+ hint on stderr) and exit code (`snp::envelope::report`, 13 X.1).
fn report(f: &Failure, printer: &Printer) -> u8 {
    match f {
        Failure::Exc { kind, exc } => {
            let mut line: Vec<u8> = match kind {
                ExcKind::Fatal => b"An error has occurred: ".to_vec(),
                ExcKind::Unexpected => b"An unexpected error has occurred: ".to_vec(),
            };
            line.extend_from_slice(exc.ty.as_bytes());
            line.extend_from_slice(b": ");
            line.extend_from_slice(&exc.text);
            printer.write_line(&line);
            if let Some(h) = &exc.hint {
                printer.py_eprint(h);
            }
        }
        Failure::Interrupted(CancelReason::Interrupt) => {
            printer.write_line(b"Interrupted (^C): KeyboardInterrupt: ");
        }
        _ => {}
    }
    u8::try_from(f.exit_code()).unwrap_or(1)
}

fn main() -> ExitCode {
    let env = Env::capture_process();
    snp_core::fault::arm_from_env(&env);
    let Some(args) = parse(std::env::args_os().skip(1)) else {
        let p = Printer::stdio(opts(false, CancelToken::new()));
        p.py_eprint("usage: snp-stash-driver <repo> <patches-dir> [--color] [--stage-fixed P]… [--abort-on-fault] -- <hook…>");
        return ExitCode::from(2);
    };
    if args.abort_on_fault {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let injected = info
                .payload()
                .downcast_ref::<String>()
                .is_some_and(|s| s.starts_with("snp fault injection"));
            if injected {
                std::process::abort();
            }
            default(info);
        }));
    }
    let cancel = CancelToken::new();
    let signals = match snp_proc::signals::install(cancel.clone(), true) {
        Ok(s) => Some(Arc::new(s)),
        Err(_) => None,
    };
    let printer = Arc::new(Printer::stdio(opts(args.color, cancel.clone())));
    let code = run(&args, &env, &cancel, signals.as_ref(), &printer);
    printer.finish();
    if let Some(s) = &signals
        && let Some(c) = s.die_by_signal_if_terminated()
    {
        return ExitCode::from(u8::try_from(c).unwrap_or(1));
    }
    ExitCode::from(code)
}

fn opts(color: bool, cancel: CancelToken) -> PrinterOpts {
    PrinterOpts {
        brand: Brand::PreCommit,
        color,
        stdout_is_tty: false,
        windows_text_mode: cfg!(windows),
        cancel,
    }
}

/// `snp run`'s single-exit discipline (§4.4) reduced to the stash: `run_inner` + an unconditional restore.
fn run(
    args: &Args,
    env: &Env,
    cancel: &CancelToken,
    signals: Option<&Arc<snp_proc::signals::SignalGuard>>,
    printer: &Arc<Printer>,
) -> u8 {
    let tool: Arc<dyn GitBackend> = Arc::new(ProcBackend {
        cancel: cancel.clone(),
        role: Role::Tool,
    });
    let discovered = GitExe::resolve(env, &args.repo, None)
        .and_then(|exe| Git::discover(Arc::new(exe), env, &args.repo, tool));
    let (git, info) = match discovered {
        Ok(v) => v,
        Err(f) => return report(&f, printer),
    };
    let critical: Arc<dyn GitBackend> = Arc::new(ProcBackend {
        cancel: CancelToken::new(),
        role: Role::Critical,
    });
    let git = git.with_brand(Brand::PreCommit).with_backend(critical);
    let cx = StashCtx {
        git,
        info,
        patches_dir: args.patches.clone(),
        printer: Arc::clone(printer),
        cancel: cancel.clone(),
        clock: Arc::new(SystemClock),
        signals: signals.cloned(),
        lock_timeout: StashCtx::lock_timeout_from_env(env),
        verify: env.is_truthy_flag("SNP_STASH_VERIFY"),
        old_git: env.is_truthy_flag("SNP_STASH_DRIVER_OLD_GIT"),
    };
    let mut guard = match snp_stash::enter(cx) {
        Ok(g) => g,
        Err(f) => return report(&f, printer),
    };
    let mut hook_env = EnvPatch::default();
    if let Some((k, v)) = guard.child_env_token() {
        hook_env = hook_env.set(k, v);
    }
    let hook_env = env.patched(&hook_env);
    let mut result: Result<(), Failure> = Ok(());
    if !args.hook.is_empty() {
        printer.flush();
        let spec = Spec {
            argv: &args.hook,
            env: &hook_env,
            cwd: &args.repo,
            stdin: Stdin::Null,
            group: Group::Inherit,
            role: Role::Hook,
            deadline: None,
            cancel,
        };
        if let Err(e) = spawn::inherit(&spec) {
            printer.py_eprint(&e.python_text());
        }
    }
    if let Some(reason) = cancel.reason() {
        // The cancel path: kill what the cancelled hooks left behind before touching the worktree (§10.4).
        snp_proc::registry::quiesce(Duration::from_secs(2));
        result = Err(Failure::Interrupted(reason));
    } else if !args.stage_fixed.is_empty() {
        match guard.stage_fixed(&args.stage_fixed) {
            Ok(true) => {}
            Ok(false) => printer.py_eprint("driver: stage_fixed would not apply"),
            Err(e) => result = Err(e),
        }
    }
    let restored = guard.restore();
    match (restored, result) {
        (Err(e), _) | (Ok(()), Err(e)) => report(&e, printer),
        (Ok(()), Ok(())) => 0,
    }
}
