//! The executor: one hook from its plan to `(retcode, output)` (ARCHITECTURE §3.5–§3.6, §3.11; `lang_base.run_xargs`,
//! `xargs.xargs`, `util.cmd_output_b`/`cmd_output_p`).
//!
//! Process hooks are shuffled (Python-exact), partitioned (`xargs.partition`), and run on the global `JobPool`
//! (`threads = min(partitions, jobs)`); outputs are concatenated in partition order and the exit code is the one with
//! the largest absolute value. Native hooks, `fail` and `pygrep` never spawn: they get the same shuffle and
//! partitioning a subprocess would (ADR-021, ADR-072).
//!
//! Owner: WP-32.

use std::ffi::{OsStr, OsString};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use snp_core::env::{Env, EnvPatch};
use snp_core::failure::{Failure, pyerr};
use snp_core::fault::{self, Point};
use snp_core::path::RepoPath;
use snp_git::Git;
use snp_git::pch_facts::PchFacts;
use snp_hooks::{Epoch, HookCx, NativeSource};
use snp_lang::RunPlan;
use snp_proc::spawn::{self, Group, Output, PyStatus, Role, Spec, Stdin};
use snp_proc::tty::Terminal;
use snp_proc::xargs;
use snp_repo::{ExecKind, ResolveCtx, ResolvedHook};

use crate::hookout::HookOutput;
use crate::{RunCtx, messages, meta};

/// Everything the executor shares across the hooks of one run.
#[derive(Debug)]
pub(crate) struct ExecEnv<'a> {
    /// The run context.
    pub(crate) cx: &'a RunCtx,
    /// The resolution context (plans, nested meta resolution).
    pub(crate) rcx: &'a ResolveCtx<'a>,
    /// The user repository with `run_env`.
    pub(crate) git: &'a Git,
    /// `run_env` (§2.5).
    pub(crate) run_env: &'a Env,
    /// The colour decision (pty, `--color` of hooks).
    pub(crate) color: bool,
    /// `py_cpu_count()`.
    pub(crate) cpu: NonZeroUsize,
    /// `xargs._max_length` from the startup environment.
    pub(crate) max_len: usize,
    /// Memoised pre-commit-hooks git facts.
    pub(crate) facts: &'a PchFacts,
    /// The git hook's stdin (`use_stdin`).
    pub(crate) stdin: Option<Arc<[u8]>>,
    /// Meta hooks.
    pub(crate) meta: &'a meta::MetaCx<'a>,
}

/// What one execution of a hook produced.
#[derive(Debug, Default)]
pub(crate) struct Ran {
    /// The aggregated return code.
    pub(crate) code: i64,
    /// The concatenated output.
    pub(crate) out: HookOutput,
    /// A native hook's exact report (`Some` only for natives that report modifications exactly).
    pub(crate) native_modified: Option<Vec<RepoPath>>,
    /// The hook provably writes nothing (pygrep, fail, meta, read-only natives).
    pub(crate) reads_only: bool,
    /// The `timeout` expired.
    pub(crate) timed_out: Option<Duration>,
    /// Extension detail lines (`- root directory … does not exist`).
    pub(crate) ext_lines: Vec<String>,
}

impl Ran {
    fn error(code: i64, output: Vec<u8>) -> Self {
        Self {
            code,
            out: HookOutput::bytes(output),
            reads_only: true,
            ..Self::default()
        }
    }
}

/// The working directory of a hook: the root, or `<root>/<root ext>`.
pub(crate) fn hook_cwd(root: &Path, h: &ResolvedHook) -> PathBuf {
    match &h.def.ext.root {
        Some(r) => root.join(&*r.to_os()),
        None => root.to_path_buf(),
    }
}

/// Runs one hook on `files` (already `()` when `pass_filenames: false`; relative to the hook's cwd). With `tty`
/// (an `interactive` hook that found a terminal, ADR-085) a process hook's partitions run one after another on the
/// terminal, uncaptured; hooks that run inside snp ignore it.
///
/// # Errors
///
/// `Failure::Interrupted` when the run was cancelled meanwhile; the exceptions pre-commit raises out of a hook run
/// (a failing pre-step, a resolution error).
pub(crate) fn run_hook(
    e: &ExecEnv<'_>,
    h: &ResolvedHook,
    files: &[OsString],
    tty: Option<&Terminal>,
) -> Result<Ran, Failure> {
    let cwd = hook_cwd(e.git.root(), h);
    if h.def.ext.root.is_some() && !cwd.is_dir() {
        let root = h
            .def
            .ext
            .root
            .as_ref()
            .map(|r| r.lossy().into_owned())
            .unwrap_or_default();
        return Ok(Ran {
            code: 1,
            ext_lines: vec![messages::root_missing(&root)],
            reads_only: true,
            ..Ran::default()
        });
    }
    let ran = match &h.exec {
        ExecKind::Meta(m) => {
            // `language: unsupported` runs through `lang_base.run_xargs`, which shuffles the file arguments unless
            // the hook is `require_serial` (`identity` prints them in that order).
            let order: Vec<OsString> = shuffled(files, !h.def.require_serial)
                .into_iter()
                .map(OsStr::to_os_string)
                .collect();
            let (code, out) = meta::run(e.meta, *m, &h.def.args, &order)?;
            Ran {
                code: i64::from(code),
                out: HookOutput::bytes(out),
                reads_only: true,
                ..Ran::default()
            }
        }
        ExecKind::Native { source, id, epoch } => {
            run_native(e, h, *source, id, *epoch, files, &cwd)?
        }
        _ => {
            let plan = e.cx.resolver.plan(h, e.rcx, e.color)?;
            run_plan(e, h, plan, files, &cwd, tty)?
        }
    };
    interrupted(e)?;
    Ok(ran)
}

/// `Err(Interrupted)` once the run token is cancelled.
fn interrupted(e: &ExecEnv<'_>) -> Result<(), Failure> {
    match e.cx.cancel.reason() {
        Some(snp_core::cancel::CancelReason::StdoutClosed) => Err(Failure::StdoutClosed),
        Some(r) => Err(Failure::Interrupted(r)),
        None => Ok(()),
    }
}

/// The child environment: the plan's (language-activated) env, then the hook's `env:` extension (user wins).
pub(crate) fn child_env(base: &Env, h: &ResolvedHook) -> Env {
    if h.def.ext.env.is_empty() {
        return base.clone();
    }
    let mut p = EnvPatch::default();
    for (k, v) in &h.def.ext.env {
        p = p.set(k.to_string(), OsString::from(&**v));
    }
    base.patched(&p)
}

/// `files` in `lang_base._shuffled` order when `shuffle`.
fn shuffled(files: &[OsString], shuffle: bool) -> Vec<&OsStr> {
    if !shuffle || files.len() < 2 {
        return files.iter().map(OsString::as_os_str).collect();
    }
    snp_pycompat::random::precommit_shuffled_indices(files.len())
        .into_iter()
        .filter_map(|i| files.get(usize::try_from(i).ok()?))
        .map(OsString::as_os_str)
        .collect()
}

/// `RunPlan` → `Ran`.
fn run_plan(
    e: &ExecEnv<'_>,
    h: &ResolvedHook,
    plan: RunPlan,
    files: &[OsString],
    cwd: &Path,
    tty: Option<&Terminal>,
) -> Result<Ran, Failure> {
    match plan {
        RunPlan::Xargs {
            cmd,
            env,
            shuffle,
            serial,
            pre,
        } => {
            let env = child_env(&env, h);
            for step in &pre {
                pre_step(e, step)?;
            }
            let serial = serial || h.def.require_serial;
            let jobs = xargs::target_concurrency(&env, serial, e.cpu);
            let order = shuffled(files, shuffle && !serial);
            run_xargs(e, h, &cmd, &order, &env, cwd, jobs, tty)
        }
        RunPlan::Fail { entry } => {
            let names: Vec<&[u8]> = files.iter().map(|f| os_bytes(f)).collect();
            Ok(Ran::error(1, snp_hooks::fail::output(&entry, &names)))
        }
        RunPlan::Pygrep { args, entry } => run_pygrep(e, &args, &entry, files, cwd),
        RunPlan::Error { code, output } => Ok(Ran::error(i64::from(code), output)),
        _ => Err(Failure::unexpected("AssertionError", "unexpected run plan")),
    }
}

/// A plan's pre-step (docker's `docker build`, ADR-035): skipped when `skip_if_ok` succeeds; a failure is the
/// `CalledProcessError` pre-commit raises (exit 3).
fn pre_step(e: &ExecEnv<'_>, step: &snp_lang::PreStep) -> Result<(), Failure> {
    let run = |argv: &[OsString]| -> Result<spawn::Captured, Failure> {
        let argv = snp_proc::resolve::normalize_cmd(argv, &step.env, &step.cwd).map_err(|err| {
            Failure::unexpected(
                "CalledProcessError",
                pyerr::called_process_a(argv, 1, Some(err.message().as_bytes()), None),
            )
        })?;
        let spec = Spec {
            argv: &argv,
            env: &step.env,
            cwd: &step.cwd,
            stdin: Stdin::Null,
            group: Group::Inherit,
            role: Role::Tool,
            deadline: None,
            cancel: &e.cx.cancel,
        };
        spawn::capture(&spec).map_err(|err| {
            Failure::unexpected(
                "CalledProcessError",
                pyerr::called_process_a(
                    argv.as_slice(),
                    1,
                    Some(err.python_text().as_bytes()),
                    None,
                ),
            )
        })
    };
    if let Some(probe) = &step.skip_if_ok
        && run(probe).is_ok_and(|c| c.status.0 == 0)
    {
        return Ok(());
    }
    let c = run(&step.argv)?;
    if c.status.0 != 0 {
        return Err(Failure::unexpected(
            "CalledProcessError",
            pyerr::called_process_a(&step.argv, c.status.0, Some(&c.stdout), Some(&c.stderr)),
        ));
    }
    Ok(())
}

/// The result of one partition.
#[derive(Debug)]
struct Part {
    status: PyStatus,
    output: Output,
    timed_out: bool,
}

/// `xargs.xargs` for a process hook.
#[expect(
    clippy::too_many_arguments,
    reason = "xargs.xargs' inputs plus the interactive terminal"
)]
fn run_xargs(
    e: &ExecEnv<'_>,
    h: &ResolvedHook,
    cmd: &[OsString],
    files: &[&OsStr],
    env: &Env,
    cwd: &Path,
    jobs: NonZeroUsize,
    tty: Option<&Terminal>,
) -> Result<Ran, Failure> {
    // `normalize_cmd` once per hook, before partitioning (`xargs.py:149-152`).
    let cmd = match snp_proc::resolve::normalize_cmd(cmd, env, cwd) {
        Ok(c) => c,
        Err(err) => return Ok(Ran::error(1, err.message().into_bytes())),
    };
    let max_len = batch_budget(&cmd, env, cwd).unwrap_or(e.max_len);
    // C-hook (X13-EXE-03, D-CRASH-05): the text pre-commit prints when exec fails with E2BIG.
    let Ok(parts) = xargs::partition(&cmd, files, jobs, max_len) else {
        let exe = cmd
            .first()
            .map(|c| snp_pycompat::repr::repr_os(c))
            .unwrap_or_default();
        return Ok(Ran::error(
            1,
            messages::argument_list_too_long(&exe).into_bytes(),
        ));
    };
    let timeout = h.def.ext.timeout;
    let deadline = timeout.and_then(|t| Instant::now().checked_add(t));
    // an interactive hook stays in snp's (the terminal's foreground) process group: an isolated group could not read
    // the terminal (SIGTTIN); its timeout then signals the child process only (ADR-085)
    let group = if timeout.is_some() && tty.is_none() {
        Group::Isolated
    } else {
        Group::Inherit
    };
    let stdin = match (&e.stdin, h.def.ext.use_stdin) {
        (Some(b), true) => Some(Arc::clone(b)),
        (None, true) => Some(Arc::from(Vec::new())),
        _ => None,
    };
    let pty = e.color && cfg!(unix) && stdin.is_none();
    // the partitions of an interactive hook take turns on the terminal
    let threads = if tty.is_some() {
        NonZeroUsize::MIN
    } else {
        NonZeroUsize::new(parts.len().min(jobs.get())).unwrap_or(NonZeroUsize::MIN)
    };
    let results = e.cx.pool.map_ordered(threads, parts.len(), |i| {
        fault::hit(Point::WorkerPartition);
        let range = parts.get(i).cloned().unwrap_or(0..0);
        let mut argv = cmd.clone();
        argv.extend(
            files
                .get(range)
                .unwrap_or_default()
                .iter()
                .map(|f| f.to_os_string()),
        );
        let spec = Spec {
            argv: &argv,
            env,
            cwd,
            stdin: match (tty, &stdin) {
                (Some(t), _) => Stdin::Terminal(t.clone()),
                (None, Some(b)) => Stdin::Bytes(Arc::clone(b)),
                (None, None) => Stdin::EmptyPipe,
            },
            group,
            role: Role::Hook,
            deadline,
            cancel: &e.cx.cancel,
        };
        if tty.is_some() {
            return Ok(interactive_part(&spec));
        }
        Ok(match spawn::merged(&spec, pty, &e.cx.budget) {
            Ok(m) => Part {
                status: m.status,
                output: m.output,
                timed_out: m.timed_out,
            },
            // `util.py:86-87`: `(1, str(e).encode().rstrip(b'\n') + b'\n')`, once per partition.
            Err(err) => {
                let mut text = err.python_text().trim_end_matches('\n').to_owned();
                text.push('\n');
                Part {
                    status: PyStatus(1),
                    output: Output::Memory(text.into_bytes()),
                    timed_out: false,
                }
            }
        })
    });
    let mut out = HookOutput::default();
    let mut codes = Vec::new();
    let mut timed_out = false;
    for r in results {
        match r {
            Ok(p) => {
                codes.push(p.status);
                timed_out |= p.timed_out;
                out.push(p.output);
            }
            // a refused permit (cancellation): report what pre-commit would see from a dead partition; the caller
            // turns the cancellation into `Interrupted`
            Err(f) if cancelled(&f) => {
                codes.push(PyStatus(1));
                out.push_bytes(failure_text(&f).into_bytes());
            }
            // a panicking worker is joined as `PanicException` (ARCHITECTURE §3.4): the run fails (exit 3) and the
            // main thread restores the stash before the banner
            Err(f) => {
                snp_os::term::ensure_vt();
                return Err(f);
            }
        }
    }
    snp_os::term::ensure_vt();
    Ok(Ran {
        code: xargs::aggregate(codes).0,
        out,
        timed_out: timeout.filter(|_| timed_out),
        ..Ran::default()
    })
}

/// One partition of an interactive hook: on the terminal, nothing captured; an unstartable program reports its error
/// like any partition (`util.py:86-87`).
fn interactive_part(spec: &Spec<'_>) -> Part {
    match spawn::interactive(spec) {
        Ok(r) => Part {
            status: r.status,
            output: Output::Memory(Vec::new()),
            timed_out: r.timed_out,
        },
        Err(err) => {
            let mut text = err.python_text().trim_end_matches('\n').to_owned();
            text.push('\n');
            Part {
                status: PyStatus(1),
                output: Output::Memory(text.into_bytes()),
                timed_out: false,
            }
        }
    }
}

/// A partition refused by cancellation (as opposed to a panicking or unstartable worker).
fn cancelled(f: &Failure) -> bool {
    matches!(f, Failure::Interrupted(_) | Failure::StdoutClosed)
}

/// The one-line text of a worker failure.
fn failure_text(f: &Failure) -> String {
    match f {
        Failure::Exc { exc, .. } => {
            format!("{}: {}\n", exc.ty, String::from_utf8_lossy(&exc.text))
        }
        other => format!("{other:?}\n"),
    }
}

/// Windows `.bat`/`.cmd` budget (`xargs.py:155-160`).
fn batch_budget(cmd: &[OsString], env: &Env, cwd: &Path) -> Option<usize> {
    if !cfg!(windows) {
        return None;
    }
    let batch = Path::new(cmd.first()?)
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("bat") || x.eq_ignore_ascii_case("cmd"));
    if !batch {
        return None;
    }
    let exe = snp_proc::resolve::find_executable(OsStr::new("cmd.exe"), env, cwd)?;
    Some(xargs::batch_max_length(&exe))
}

/// Raw bytes of an `OsStr` (Unix exact; elsewhere UTF-8 lossy).
pub(crate) fn os_bytes(s: &OsStr) -> &[u8] {
    s.as_encoded_bytes()
}

/// The `sys.argv[0]` stand-in for the length budget of native partitions: a fixed-length interpreter path, so the
/// partition boundaries do not depend on where snp is installed (ARCHITECTURE §3.5 item 5).
fn synthetic_cmd(prog: &str, args: &[Arc<str>]) -> Vec<OsString> {
    let mut v = vec![OsString::from(format!(
        "/home/user/.cache/pre-commit/repo00000000/py_env-python3/bin/{prog}"
    ))];
    v.extend(args.iter().map(|a| OsString::from(&**a)));
    v
}

/// A native pre-commit-hooks port (or builtin variant): shuffled and partitioned like its console script.
fn run_native(
    e: &ExecEnv<'_>,
    h: &ResolvedHook,
    source: NativeSource,
    id: &str,
    epoch: Epoch,
    files: &[OsString],
    cwd: &Path,
) -> Result<Ran, Failure> {
    let Some(native) = snp_hooks::native(source, id) else {
        return Err(Failure::unexpected(
            "AssertionError",
            format!("no native implementation of {id}"),
        ));
    };
    let env = child_env(e.run_env, h);
    let serial = h.def.require_serial;
    let jobs = xargs::target_concurrency(&env, serial, e.cpu);
    let order = shuffled(files, !serial);
    let args = &h.def.args;
    let parts = if e.rcx.fast_path.coalesce {
        std::iter::once(0..order.len()).collect()
    } else {
        let Ok(p) = xargs::partition(&synthetic_cmd(native.prog(), args), &order, jobs, e.max_len)
        else {
            return Ok(Ran::error(
                1,
                messages::argument_list_too_long("'python'").into_bytes(),
            ));
        };
        p
    };
    let columns = snp_hooks::columns(&env, None);
    let hcx = HookCx {
        root: cwd,
        env: &env,
        git: e.git,
        color: e.color,
        epoch,
        windows: cfg!(windows),
        columns,
        facts: e.facts,
        cancel: &e.cx.cancel,
    };
    let threads = NonZeroUsize::new(parts.len().min(jobs.get())).unwrap_or(NonZeroUsize::MIN);
    let results = e.cx.pool.map_ordered(threads, parts.len(), |i| {
        fault::hit(Point::WorkerPartition);
        let range = parts.get(i).cloned().unwrap_or(0..0);
        let mut argv: Vec<&OsStr> = args.iter().map(|a| OsStr::new(&**a)).collect();
        argv.extend(order.get(range).unwrap_or_default().iter().copied());
        Ok(native.run(&hcx, &argv))
    });
    let mut out = HookOutput::default();
    let mut codes = Vec::new();
    let mut modified = Vec::new();
    let mut reads_only = true;
    for r in results {
        let o = r?;
        codes.push(PyStatus(i64::from(o.code)));
        out.push_bytes(o.output);
        modified.extend(o.modified);
        reads_only &= o.reads_only;
    }
    Ok(Ran {
        code: xargs::aggregate(codes).0,
        out,
        native_modified: native.reports_modifications_exactly().then_some(modified),
        reads_only,
        ..Ran::default()
    })
}

/// `language: pygrep` in process: compiled once per hook per run, unshuffled, partitioned with
/// `target_concurrency=1` exactly like `xargs((python, -m, pre_commit.languages.pygrep, *args, entry), files)`.
fn run_pygrep(
    e: &ExecEnv<'_>,
    args: &[Arc<str>],
    entry: &str,
    files: &[OsString],
    cwd: &Path,
) -> Result<Ran, Failure> {
    let columns = snp_hooks::columns(e.run_env, None);
    let compiled = match snp_hooks::pygrep::prepare(args, entry, columns) {
        Ok(c) => c,
        Err((code, out)) => return Ok(Ran::error(i64::from(code), out)),
    };
    let order: Vec<&OsStr> = files.iter().map(OsString::as_os_str).collect();
    let mut cmd = vec![
        OsString::from("/usr/bin/python3"),
        OsString::from("-m"),
        OsString::from("pre_commit.languages.pygrep"),
    ];
    cmd.extend(args.iter().map(|a| OsString::from(&**a)));
    cmd.push(OsString::from(entry));
    let Ok(parts) = xargs::partition(&cmd, &order, NonZeroUsize::MIN, e.max_len) else {
        return Ok(Ran::error(
            1,
            messages::argument_list_too_long("'/usr/bin/python3'").into_bytes(),
        ));
    };
    let jobs = xargs::target_concurrency(e.run_env, false, e.cpu);
    let threads = NonZeroUsize::new(parts.len().min(jobs.get())).unwrap_or(NonZeroUsize::MIN);
    let results = e.cx.pool.map_ordered(threads, parts.len(), |i| {
        fault::hit(Point::WorkerPartition);
        let range = parts.get(i).cloned().unwrap_or(0..0);
        let mut argv: Vec<&OsStr> = args.iter().map(|a| OsStr::new(&**a)).collect();
        argv.push(OsStr::new(entry));
        argv.extend(order.get(range).unwrap_or_default().iter().copied());
        Ok(snp_hooks::pygrep::run_partition(
            &compiled,
            &argv,
            cwd,
            &e.cx.regexes,
            &e.cx.cancel,
        ))
    });
    let mut out = HookOutput::default();
    let mut codes = Vec::new();
    for r in results {
        match r {
            Ok(o) => {
                codes.push(PyStatus(i64::from(o.code)));
                out.push_bytes(o.output);
            }
            Err(f) if cancelled(&f) => {
                codes.push(PyStatus(1));
                out.push_bytes(failure_text(&f).into_bytes());
            }
            Err(f) => return Err(f),
        }
    }
    Ok(Ran {
        code: xargs::aggregate(codes).0,
        out,
        reads_only: true,
        ..Ran::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffle_matches_python() {
        let files: Vec<OsString> = (0..10).map(|i| OsString::from(i.to_string())).collect();
        let got: Vec<String> = shuffled(&files, true)
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        // tests/lang_base_test.py:138-141
        assert_eq!(got, ["4", "0", "5", "1", "8", "6", "2", "3", "7", "9"]);
        let got: Vec<&OsStr> = shuffled(&files, false);
        assert_eq!(got.first().copied(), Some(OsStr::new("0")));
    }

    #[test]
    fn synthetic_prefix_is_fixed_length() {
        let a = synthetic_cmd("check-yaml", &[]);
        let b = synthetic_cmd("check-yaml", &[Arc::from("--x")]);
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 2);
        assert!(a[0].to_string_lossy().ends_with("/bin/check-yaml"));
    }
}
