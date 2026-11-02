//! `hazmat` in-hook (`__hazmat`, path H) and CLI (path C) entry points (`commands/hazmat.py`; ADR-006).
//!
//! Both paths run `hazmat.py`'s tools exactly (03 Addendum §H.4): `_cmd_filenames` splits on the **last** `--`
//! (``hazmat entry must end with `--` ``), `cd` checks every file for the `{subdir}/` prefix before resolving the
//! command (`unexpected file without prefix='sub/': x`), `normalize_cmd` resolves against the current directory
//! and environment, the children inherit stdio (`subprocess.call`), `n1` ORs the return codes of one call per
//! file and `ignore-exit-code` returns 0 for any child status. A signal death is `-N`; the process status is
//! `ret & 0xFF` on POSIX (SIGKILL alone → 247).
//!
//! * Path H ([`in_hook_main`]): prog `hazmat.py`, CPython 3.13 argparse texts, exit 0/1/2/child. Python has no
//!   error handler there: an exception prints a traceback and exits 1; snp prints only its final line
//!   (`pre_commit.parse_shebang.ExecutableNotFoundError: Executable `x` not found`, D-MISC-07). SIGINT waits
//!   250 ms for the child, SIGKILLs it and dies by SIGINT (`Popen.wait`'s `_sigint_wait_secs`, §H.7.4).
//! * Path C ([`cli`]): the parsed `{prog} hazmat …` tool; exceptions are `Failure`s for the envelope
//!   (`ExecutableNotFoundError` exit 3), `SystemExit` texts exit 1.
//!
//! Owner: WP-26.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use snp_argparse::{ActionSpec, Nargs, ParseCfg, ParserSpec, SubCommand};
use snp_cli::HazmatTool;
use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::env::Env;
use snp_core::failure::{Failure, PyExc, pyerr};
use snp_proc::resolve::ExeError;
use snp_proc::spawn::{Group, Role, Spec, Stdin};

use crate::framework::{self, positional};

/// `hazmat.py cd`'s parser.
static CD: ParserSpec = ParserSpec {
    prog_suffix: " cd",
    description: None,
    epilog: None,
    actions: &[
        positional("subdir", Nargs::One),
        positional("cmd", Nargs::Remainder),
    ],
    allow_abbrev: true,
    add_help: true,
};
static CMD_ONLY: [ActionSpec; 1] = [positional("cmd", Nargs::Remainder)];
/// `hazmat.py ignore-exit-code`'s parser.
static IGNORE_EXIT_CODE: ParserSpec = ParserSpec {
    prog_suffix: " ignore-exit-code",
    actions: &CMD_ONLY,
    ..CD
};
/// `hazmat.py n1`'s parser.
static N1: ParserSpec = ParserSpec {
    prog_suffix: " n1",
    actions: &CMD_ONLY,
    ..CD
};
static TOOLS: [SubCommand; 3] = [
    SubCommand {
        name: "cd",
        help: Some("cd to a subdir and run the command"),
        listed: true,
        spec: &CD,
    },
    SubCommand {
        name: "ignore-exit-code",
        help: Some("run the command but ignore the exit code"),
        listed: true,
        spec: &IGNORE_EXIT_CODE,
    },
    SubCommand {
        name: "n1",
        help: Some("run the command once per filename"),
        listed: true,
        spec: &N1,
    },
];
static TOOL_ACTION: [ActionSpec; 1] = [ActionSpec {
    nargs: Nargs::Parser,
    kind: snp_argparse::Kind::SubParsers(&TOOLS),
    ..positional("tool", Nargs::Parser)
}];
/// `hazmat.main`'s parser: `ArgumentParser()` + `add_parsers` (`subparsers(dest='tool')`, not required).
static HAZMAT_PY: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &TOOL_ACTION,
    allow_abbrev: true,
    add_help: true,
};

/// `hazmat.py`'s parser (prog `hazmat.py`, CPython-3.13 texts).
pub static HAZMAT_PY_SPEC: &ParserSpec = &HAZMAT_PY;

/// The prog of path H.
const PROG: &str = "hazmat.py";

/// How a tool ended other than by returning a code.
#[derive(Debug)]
enum Stop {
    /// `raise SystemExit(msg)`: `msg` on stderr, exit 1.
    SystemExit(String),
    /// An uncaught exception.
    Exc(PyExc),
    /// Interrupted while a child ran.
    Cancelled(CancelReason),
}

/// `_cmd_filenames(cmd)`: the command before the **last** `--` and the file names after it.
fn cmd_filenames(cmd: &[OsString]) -> Result<(&[OsString], &[OsString]), Stop> {
    let Some(i) = cmd.iter().rposition(|t| t == "--") else {
        return Err(Stop::SystemExit(
            "hazmat entry must end with `--`".to_owned(),
        ));
    };
    let head = cmd.get(..i).unwrap_or_default();
    let tail = cmd.get(i.saturating_add(1)..).unwrap_or_default();
    Ok((head, tail))
}

/// `normalize_cmd(cmd)` with Python's exceptions (`IndexError` for an empty command).
fn normalize(cmd: &[OsString], env: &Env, cwd: &Path) -> Result<Vec<OsString>, Stop> {
    if cmd.is_empty() {
        return Err(Stop::Exc(PyExc::new(
            "IndexError",
            "tuple index out of range",
        )));
    }
    snp_proc::resolve::normalize_cmd(cmd, env, cwd).map_err(|e| {
        Stop::Exc(match &e {
            ExeError::Io(io) => PyExc::new(pyerr::os_error_class(io), e.message()),
            _ => PyExc::new("ExecutableNotFoundError", e.message()),
        })
    })
}

/// One `subprocess.call(argv, cwd=…)`: the Python return code (`-N` for a signal).
fn call(argv: &[OsString], env: &Env, cwd: &Path, cancel: &CancelToken) -> Result<i64, Stop> {
    let spec = Spec {
        argv,
        env,
        cwd,
        stdin: Stdin::Inherit,
        group: Group::Inherit,
        role: Role::Hook,
        deadline: None,
        cancel,
    };
    let status = snp_proc::spawn::inherit(&spec)
        .map_err(|e| Stop::Exc(PyExc::new(pyerr::os_error_class(&e.io), e.python_text())))?;
    if let Some(reason) = cancel.reason() {
        return Err(Stop::Cancelled(reason));
    }
    Ok(status.0)
}

/// `hazmat.impl(args)` for one tool, in `cwd` (the hook's work tree).
fn run_tool(tool: &HazmatTool, env: &Env, cwd: &Path, cancel: &CancelToken) -> Result<i64, Stop> {
    match tool {
        HazmatTool::Cd { subdir, cmd } => {
            let (head, files) = cmd_filenames(cmd)?;
            let mut prefix = subdir.clone();
            prefix.push("/");
            let mut stripped: Vec<OsString> = Vec::new();
            for f in files {
                let Some(rest) = f.as_encoded_bytes().strip_prefix(prefix.as_encoded_bytes())
                else {
                    let mut msg = format!(
                        "unexpected file without prefix={}: ",
                        snp_pycompat::repr::repr_os(&prefix)
                    );
                    msg.push_str(&f.to_string_lossy());
                    return Err(Stop::SystemExit(msg));
                };
                stripped.push(os_from_bytes(rest));
            }
            let mut argv = normalize(head, env, cwd)?;
            argv.extend(stripped);
            // `subprocess.call(…, cwd=subdir)`: relative to this process's cwd, named as given in errors.
            let dir = PathBuf::from(subdir);
            call(&argv, env, &dir, cancel)
        }
        HazmatTool::IgnoreExitCode { cmd } => {
            let argv = normalize(cmd, env, cwd)?;
            call(&argv, env, cwd, cancel).map(|_| 0)
        }
        HazmatTool::N1 { cmd } => {
            let (head, files) = cmd_filenames(cmd)?;
            let argv = normalize(head, env, cwd)?;
            let mut ret = 0_i64;
            for f in files {
                let mut one = argv.clone();
                one.push(f.clone());
                ret |= call(&one, env, cwd, cancel)?;
            }
            Ok(ret)
        }
        _ => Err(Stop::Exc(PyExc::new(
            "NotImplementedError",
            "unexpected tool",
        ))),
    }
}

/// An `OsString` of bytes that came from an `OsStr` (a suffix of one, cut after an ASCII `/`).
fn os_from_bytes(b: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        std::ffi::OsStr::from_bytes(b).to_os_string()
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(b).into_owned())
    }
}

/// The process status of a Python `raise SystemExit(ret)` (POSIX: `ret & 0xFF`; Windows: the 32-bit value).
fn exit_status(ret: i64) -> i32 {
    if cfg!(windows) {
        // `TerminateProcess`/`ExitProcess` codes are DWORDs; Python passes the low 32 bits.
        i32::from_ne_bytes(u32::try_from(ret & 0xFFFF_FFFF).unwrap_or(1).to_ne_bytes())
    } else {
        i32::try_from(ret & 0xFF).unwrap_or(1)
    }
}

/// Runs `body` while a watcher kills the children 250 ms after a cancellation (`Popen.wait`'s
/// `_sigint_wait_secs`, then `p.kill()`; SIGTERM/SIGHUP kill at once).
fn with_kill_watch<T>(cancel: &CancelToken, body: impl FnOnce() -> T) -> T {
    let done = AtomicBool::new(false);
    // Woken by the run's cancellation (inherited) or, once `body` returned, by its own cancellation.
    let watch = cancel.child();
    std::thread::scope(|s| {
        let watcher = std::thread::Builder::new()
            .name("hazmat-sigint".into())
            .stack_size(64 * 1024)
            .spawn_scoped(s, || {
                while !done.load(Ordering::Acquire) {
                    if !watch.wait_timeout(Duration::from_secs(60)) {
                        continue;
                    }
                    if done.load(Ordering::Acquire) {
                        break;
                    }
                    if cancel.reason() == Some(CancelReason::Interrupt) {
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    snp_proc::registry::escalate();
                    break;
                }
            });
        let out = body();
        done.store(true, Ordering::Release);
        watch.cancel(CancelReason::Interrupt);
        if let Ok(h) = watcher {
            let _joined = h.join();
        }
        out
    })
}

/// `snp __hazmat …` (path H, a subprocess per partition): prog `hazmat.py`, CPython-3.13 texts, exit 0/1/2/child.
///
/// `args` are the tokens after `__hazmat` (entry tokens, hook args, partition files); `env` is this process's
/// environment (the hook's). Never panics; output goes to this process's stdout (help) and stderr (everything
/// else), write errors ignored.
pub fn in_hook_main(args: &[OsString], env: &Env) -> i32 {
    let columns = snp_argparse::columns(env, None);
    let cfg = ParseCfg {
        prog: PROG,
        env,
        columns,
    };
    let ns = match snp_argparse::parse(&HAZMAT_PY, args, &cfg) {
        snp_argparse::Outcome::Parsed(ns) => ns,
        snp_argparse::Outcome::Help(text) | snp_argparse::Outcome::Version(text) => {
            write_out(text.as_bytes());
            return 0;
        }
        snp_argparse::Outcome::Usage { stderr } => {
            write_err(stderr.as_bytes());
            return 2;
        }
        _ => return 2,
    };
    let tool = match ns.command() {
        Some(("cd", sub)) => HazmatTool::Cd {
            subdir: sub.value("subdir").unwrap_or_default().to_os_string(),
            cmd: sub.list("cmd").to_vec(),
        },
        Some(("ignore-exit-code", sub)) => HazmatTool::IgnoreExitCode {
            cmd: sub.list("cmd").to_vec(),
        },
        Some(("n1", sub)) => HazmatTool::N1 {
            cmd: sub.list("cmd").to_vec(),
        },
        _ => {
            // `args.cmd = tuple(args.cmd)` on `Namespace(tool=None)` (03 §H.3, ORC D1).
            write_err(&framework::crash_line(&PyExc::new(
                "AttributeError",
                "'Namespace' object has no attribute 'cmd'",
            )));
            return 1;
        }
    };
    let Ok(cwd) = std::env::current_dir() else {
        write_err(b"FileNotFoundError: [Errno 2] No such file or directory\n");
        return 1;
    };
    let cancel = CancelToken::new();
    let guard = snp_proc::signals::install(cancel.clone(), false).ok();
    let result = with_kill_watch(&cancel, || run_tool(&tool, env, &cwd, &cancel));
    let code = match result {
        Ok(ret) => exit_status(ret),
        Err(Stop::SystemExit(msg)) => {
            let mut b = msg.into_bytes();
            b.push(b'\n');
            write_err(&b);
            1
        }
        Err(Stop::Exc(exc)) => {
            write_err(&framework::crash_line(&exc));
            1
        }
        Err(Stop::Cancelled(reason)) => die_by(reason),
    };
    drop(guard);
    code
}

/// Python's death after a `KeyboardInterrupt` (SIGINT re-raised by the interpreter) or by SIGTERM/SIGHUP: the
/// process ends by that signal; the returned status is used only if it survives (130/143/129;
/// Windows: `STATUS_CONTROL_C_EXIT`).
fn die_by(reason: CancelReason) -> i32 {
    if reason == CancelReason::Interrupt {
        write_err(b"KeyboardInterrupt\n");
    }
    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        let (sig, code) = match reason {
            CancelReason::Terminate => (SIGTERM, 143),
            CancelReason::Hangup => (SIGHUP, 129),
            _ => (SIGINT, 130),
        };
        let _ignored = signal_hook::low_level::emulate_default_handler(sig);
        code
    }
    #[cfg(not(unix))]
    {
        let _: CancelReason = reason;
        i32::from_ne_bytes(0xC000_013A_u32.to_ne_bytes())
    }
}

fn write_out(b: &[u8]) {
    let mut o = std::io::stdout().lock();
    let _ignored = o.write_all(b).and_then(|()| o.flush());
}

fn write_err(b: &[u8]) {
    let mut o = std::io::stderr().lock();
    let _ignored = o.write_all(b).and_then(|()| o.flush());
}

/// Public `{prog} hazmat …` (path C): `ExecutableNotFoundError` → `Failure` (exit 3).
///
/// `main.py:377-399` runs `hazmat.impl(args)` inside `error_handler`: an exception is
/// `Failure::Exc` (unexpected, exit 3, `An unexpected error has occurred: ExecutableNotFoundError: …`), a
/// `SystemExit` text is `Failure::SystemExit` (stderr, exit 1), an interrupt is `Failure::Interrupted` (130,
/// after killing the child 250 ms later). `Ok` is the process status (`ret & 0xFF` on POSIX). The tool comes
/// parsed from snp-cli (the no-tool case is its usage error, D-CRASH-22); the store is not touched.
pub fn cli(tool: &HazmatTool, env: &Env, cancel: &CancelToken) -> Result<i32, Failure> {
    let cwd = std::env::current_dir().map_err(|e| Failure::from_io(&e, None, None))?;
    match with_kill_watch(cancel, || run_tool(tool, env, &cwd, cancel)) {
        Ok(ret) => Ok(exit_status(ret)),
        Err(Stop::SystemExit(msg)) => {
            let mut stderr = msg.into_bytes();
            stderr.push(b'\n');
            Err(Failure::SystemExit { code: 1, stderr })
        }
        Err(Stop::Exc(exc)) => {
            let ty = exc.ty.rsplit('.').next().unwrap_or_default().to_owned();
            Err(Failure::unexpected(ty, exc.text))
        }
        Err(Stop::Cancelled(reason)) => Err(Failure::Interrupted(reason)),
    }
}

#[cfg(test)]
mod tests;
