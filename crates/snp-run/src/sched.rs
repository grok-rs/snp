//! The hook loop (`run._run_hooks` / `_run_single_hook`, `run.py:142-322`; ARCHITECTURE §3.4, §3.7–§3.9, §8.3).
//!
//! Without extension keys every hook is its own bucket, in config order — pre-commit's sequential loop, with the
//! `name....` start segment written and flushed before the hook runs. With `priority` the hooks of one bucket run
//! concurrently (hook threads hold no pool permit), a bucket is a barrier, and blocks print head-of-line in config
//! order with bytes identical to the sequential rendering.
//!
//! Owner: WP-32.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::Path;
use std::time::Instant;

use snp_config::ext::{FailOnChanges, OutputMode, Priority, RootExt, StageFixed};
use snp_core::failure::Failure;
use snp_core::output::Printer;
use snp_core::path::RepoPath;
use snp_files::{Classifier, HookFilter};
use snp_git::Git;
use snp_repo::{ExecKind, ResolvedHook};
use snp_stash::diff_oracle::{Change, DiffOracle};

use crate::exec::{self, ExecEnv, Ran};
use crate::hookout::{HookOutput, PrinterSink};
use crate::render::{self, Details, Status, SummaryRow};
use crate::{messages, stamp};

/// The loop's inputs.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one field per run option of `_run_hooks`"
)]
pub(crate) struct Loop<'a> {
    /// The executor.
    pub(crate) e: &'a ExecEnv<'a>,
    /// The selected hooks, config order.
    pub(crate) hooks: &'a [&'a ResolvedHook],
    /// Per hook: skipped by `SKIP`, `SNP_SKIP_GROUPS`, the overlay or a `skip`/`only` condition.
    pub(crate) skipped: &'a [bool],
    /// `_compute_cols(hooks)`.
    pub(crate) cols: usize,
    /// `--verbose`.
    pub(crate) verbose: bool,
    /// `config.fail_fast or args.fail_fast`.
    pub(crate) fail_fast: bool,
    /// The classifier (`None` when no selected hook consults files, P17).
    pub(crate) classifier: Option<&'a Classifier>,
    /// The output mode (`output:` / `SNP_OUTPUT` / `--summary`).
    pub(crate) mode: OutputMode,
    /// The root `fail_on_changes`.
    pub(crate) fail_on_changes: FailOnChanges,
    /// `CI` is truthy.
    pub(crate) ci: bool,
    /// `stage_fixed` can take effect (pre-commit stage, staged files, a stash).
    pub(crate) stage_fixed: bool,
    /// The root extension keys (`priorities`).
    pub(crate) root_ext: &'a RootExt,
}

/// What the loop produced.
#[derive(Debug, Default)]
pub(crate) struct LoopResult {
    /// `retval` (0/1).
    pub(crate) retval: i32,
    /// Accepted `stage_fixed` fixes still to stage.
    pub(crate) to_stage: Vec<RepoPath>,
}

/// The final state of one hook.
#[derive(Debug)]
struct Done {
    status: Status,
    code: i64,
    modified: bool,
    duration: Option<f64>,
    ext_lines: Vec<String>,
    out: HookOutput,
    /// Counts for `retval` / `fail_fast`.
    failed: bool,
}

impl Done {
    fn skipped(status: Status) -> Self {
        Self {
            status,
            code: 0,
            modified: false,
            duration: None,
            ext_lines: Vec::new(),
            out: HookOutput::default(),
            failed: false,
        }
    }
}

/// A hook ready to run.
#[derive(Debug)]
struct Job<'h> {
    pos: usize,
    hook: &'h ResolvedHook,
    /// The files passed (`()` when `pass_filenames: false`), relative to the hook's cwd.
    argv_files: Vec<OsString>,
    /// The hook's input files (root-relative): `stage_fixed` and bucket attribution.
    inputs: Vec<RepoPath>,
}

/// The effective `priority` (implicit = flattened index).
fn priority(h: &ResolvedHook, root: &RootExt) -> u64 {
    match &h.def.ext.priority {
        Some(Priority::Value(v)) => u64::from(*v),
        Some(Priority::Alias(a)) => root
            .priorities
            .get(a)
            .map_or_else(|| u64::from(h.index), |v| u64::from(*v)),
        _ => u64::from(h.index),
    }
}

/// Buckets of positions: ascending priority, config order inside a bucket.
fn buckets(hooks: &[&ResolvedHook], root: &RootExt) -> Vec<Vec<usize>> {
    let mut by: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (i, h) in hooks.iter().enumerate() {
        by.entry(priority(h, root)).or_default().push(i);
    }
    by.into_values().collect()
}

/// Head-of-line printer state.
#[derive(Debug)]
struct Out<'a> {
    printer: &'a Printer,
    cols: usize,
    color: bool,
    verbose: bool,
    mode: OutputMode,
    root: &'a Path,
    next: usize,
    /// A start segment was written for this position.
    started: Option<usize>,
    done: BTreeMap<usize, Done>,
    summary: Vec<(usize, Status, Option<f64>)>,
}

impl Out<'_> {
    /// Whether position `pos` may write its start segment now (sequential, default/summary output).
    fn can_start(&self, pos: usize) -> bool {
        self.next == pos && self.mode != OutputMode::Quiet
    }

    fn start(&mut self, pos: usize, name: &str) {
        self.printer
            .write(render::start_msg(name, self.cols).as_bytes());
        self.started = Some(pos);
    }

    /// Prints every finished block that is next in config order.
    fn flush(&mut self, hooks: &[&ResolvedHook]) {
        while let Some(d) = self.done.remove(&self.next) {
            if let Some(h) = hooks.get(self.next) {
                self.print(h, &d, self.started == Some(self.next));
            }
            self.summary.push((self.next, d.status, d.duration));
            self.next = self.next.saturating_add(1);
        }
    }

    /// At the end (or after `fail_fast`): the finished blocks in config order, skipping hooks that never ran.
    fn drain(&mut self, hooks: &[&ResolvedHook]) {
        let rest = std::mem::take(&mut self.done);
        for (pos, d) in rest {
            if let Some(h) = hooks.get(pos) {
                self.print(h, &d, self.started == Some(pos));
            }
            self.summary.push((pos, d.status, d.duration));
        }
    }

    fn print(&self, h: &ResolvedHook, d: &Done, started: bool) {
        if self.mode == OutputMode::Quiet && !d.failed {
            return;
        }
        let name = &*h.def.name;
        if started {
            self.printer
                .write(render::end_msg(d.status, self.color).as_bytes());
        } else {
            self.printer
                .write(render::full_msg(name, d.status, self.cols, self.color).as_bytes());
        }
        if !(self.verbose || h.def.verbose || d.code != 0 || d.modified || !d.ext_lines.is_empty())
        {
            return;
        }
        let header = render::details(&Details {
            id: &h.def.id,
            duration: d.duration.filter(|_| self.verbose || h.def.verbose),
            exit_code: Some(d.code),
            modified: d.modified,
            ext_lines: &d.ext_lines,
            output: b"",
            color: self.color,
        });
        self.printer.write(&header);
        if d.out.is_blank() {
            return;
        }
        // `output.write_line(); output.write_line_b(out.strip(), logfile_name=hook.log_file); output.write_line()`
        self.printer.write(b"\n");
        // the printer never fails (EPIPE latches it closed and cancels the run)
        drop(d.out.write_stripped(&mut PrinterSink(self.printer)));
        self.printer.write(b"\n");
        if !h.def.log_file.is_empty() {
            self.log_file(&h.def.log_file, &d.out);
        }
        self.printer.write(b"\n");
    }

    /// `open(log_file, 'ab')` + the stripped output + `\n`; a failure is a warning (D-CRASH-15, X13-EXE-05).
    fn log_file(&self, name: &str, out: &HookOutput) {
        use std::io::Write as _;
        let path = self.root.join(name);
        let r = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .and_then(|mut f| {
                out.write_stripped(&mut f)?;
                f.write_all(b"\n")
            });
        if let Err(e) = r {
            let text =
                snp_core::failure::pyerr::os_error_text(&e, Some(std::ffi::OsStr::new(name)), None);
            self.printer.log(
                snp_core::output::Level::Warning,
                &messages::log_file_failed(name, &text),
            );
        }
    }
}

/// `_run_hooks`: every bucket, `fail_fast`, the summary footer. The show-diff and staging steps are the caller's.
///
/// # Errors
///
/// An exception a hook run raised (exit 3 / 1), `Failure::Interrupted`, `Failure::StdoutClosed`.
pub(crate) fn run_hooks(l: &Loop<'_>, oracle: &mut DiffOracle) -> Result<LoopResult, Failure> {
    let e = l.e;
    let printer = &*e.cx.printer;
    let root = e.git.root();
    let started_at = e.cx.clock.now();
    let mut out = Out {
        printer,
        cols: l.cols,
        color: e.color,
        verbose: l.verbose,
        mode: l.mode,
        root,
        next: 0,
        started: None,
        done: BTreeMap::new(),
        summary: Vec::new(),
    };
    let mut res = LoopResult::default();
    let mut fixes = Fixes::default();
    let mut stop = false;
    for bucket in buckets(l.hooks, l.root_ext) {
        if stop {
            break;
        }
        check_cancel(e)?;
        let mut jobs = Vec::new();
        for &pos in &bucket {
            let Some(h) = l.hooks.get(pos).copied() else {
                continue;
            };
            let (argv_files, inputs) = filenames(l, h)?;
            if l.skipped.get(pos).copied().unwrap_or(false) {
                out.done.insert(pos, Done::skipped(Status::Skipped));
            } else if argv_files.is_empty() && !h.def.always_run {
                out.done.insert(pos, Done::skipped(Status::NoFiles));
            } else {
                jobs.push(Job {
                    pos,
                    hook: h,
                    argv_files: if h.def.pass_filenames {
                        argv_files
                    } else {
                        Vec::new()
                    },
                    inputs,
                });
            }
        }
        // statuses that need no run print at once when they are next in line
        out.flush(l.hooks);
        let finished = if jobs.len() == 1 {
            let job = jobs.pop().map(|j| vec![j]).unwrap_or_default();
            run_sequential(l, &mut out, oracle, &mut fixes, job)?
        } else if jobs.is_empty() {
            Vec::new()
        } else {
            run_bucket(l, oracle, &fixes, jobs)?
        };
        for (pos, d) in finished {
            let h = l.hooks.get(pos);
            if d.failed {
                res.retval = 1;
                if l.fail_fast || h.is_some_and(|h| h.def.fail_fast) {
                    stop = true;
                }
            }
            out.done.insert(pos, d);
        }
        out.flush(l.hooks);
    }
    out.drain(l.hooks);
    if l.mode == OutputMode::Summary {
        let wall = secs(e, started_at);
        let rows: Vec<SummaryRow<'_>> = out
            .summary
            .iter()
            .filter_map(|(pos, status, duration)| {
                Some(SummaryRow {
                    name: &l.hooks.get(*pos)?.def.name,
                    status: *status,
                    duration: *duration,
                })
            })
            .collect();
        printer.write(render::summary(&rows, wall, e.color).as_bytes());
    }
    res.to_stage = fixes.into_paths();
    check_cancel(e)?;
    Ok(res)
}

fn check_cancel(e: &ExecEnv<'_>) -> Result<(), Failure> {
    match e.cx.cancel.reason() {
        Some(snp_core::cancel::CancelReason::StdoutClosed) => Err(Failure::StdoutClosed),
        Some(r) => Err(Failure::Interrupted(r)),
        None => Ok(()),
    }
}

/// Seconds since `t` on the run clock.
fn secs(e: &ExecEnv<'_>, t: Instant) -> f64 {
    e.cx.clock.now().saturating_duration_since(t).as_secs_f64()
}

/// `classifier.filenames_for_hook(hook)` for every hook (skipped ones too: the tag cache fills in pre-commit's order)
/// unless the hook never consults files; `(argv files relative to the hook cwd, root-relative inputs)`.
fn filenames(l: &Loop<'_>, h: &ResolvedHook) -> Result<(Vec<OsString>, Vec<RepoPath>), Failure> {
    let Some(c) = l.classifier else {
        return Ok((Vec::new(), Vec::new()));
    };
    if h.def.always_run && !h.def.pass_filenames {
        return Ok((Vec::new(), Vec::new()));
    }
    let f = HookFilter::compile(
        &h.def.files,
        &h.def.exclude,
        h.def.types,
        h.def.types_or,
        h.def.exclude_types,
        h.def.ext.root.clone(),
        &l.e.cx.regexes,
    )
    .map_err(|err| Failure::unexpected("re.error", err.to_string()))?;
    let ids = c.for_hook(&f, &l.e.cx.pool)?;
    let mut argv = Vec::new();
    let mut inputs = Vec::new();
    for id in ids {
        match &h.def.ext.root {
            Some(prefix) => {
                if let Some(rel) = c.table.relative_to(id, prefix) {
                    argv.push(crate::files::os_from_bytes(&rel));
                }
            }
            None => argv.push(c.table.os(id).into_owned()),
        }
        if let Some(p) = RepoPath::new(c.table.bytes(id)) {
            inputs.push(p);
        }
    }
    Ok((argv, inputs))
}

/// Whether modifications fail this hook (`fail_on_changes`).
fn changes_fail(l: &Loop<'_>, h: &ResolvedHook) -> bool {
    match h.def.ext.fail_on_changes.unwrap_or(l.fail_on_changes) {
        FailOnChanges::Never => false,
        FailOnChanges::Ci => l.ci,
        FailOnChanges::NonCi => !l.ci,
        _ => true,
    }
}

/// The active `stage_fixed` mode of a hook.
fn stage_fixed(l: &Loop<'_>, h: &ResolvedHook) -> Option<StageFixed> {
    h.def.ext.stage_fixed.filter(|_| l.stage_fixed)
}

/// Accepted `stage_fixed` fixes: the stamp each path had when it was accepted.
#[derive(Debug, Default)]
struct Fixes {
    accepted: BTreeMap<RepoPath, stamp::Stamp>,
    poisoned: BTreeSet<RepoPath>,
}

impl Fixes {
    /// Paths a later hook changed again are never staged (§8.3).
    fn check_after(&mut self, root: &Path, accepting: &BTreeSet<RepoPath>) {
        for p in stamp::changed(root, &self.accepted) {
            if !accepting.contains(&p) {
                self.poisoned.insert(p);
            }
        }
    }

    fn accept(&mut self, root: &Path, paths: &[RepoPath]) {
        for p in paths {
            self.accepted.insert(p.clone(), stamp::stamp(root, p));
        }
    }

    fn into_paths(self) -> Vec<RepoPath> {
        self.accepted
            .into_keys()
            .filter(|p| !self.poisoned.contains(p))
            .collect()
    }
}

/// `git diff --name-only -z` (worktree vs index) as root-relative paths: the tracked files currently dirty.
fn dirty(git: &Git) -> Result<BTreeSet<RepoPath>, Failure> {
    let args = [
        std::ffi::OsStr::new("diff"),
        std::ffi::OsStr::new("--name-only"),
        std::ffi::OsStr::new("-z"),
        std::ffi::OsStr::new("--no-ext-diff"),
        std::ffi::OsStr::new("--no-textconv"),
        std::ffi::OsStr::new("--ignore-submodules"),
    ];
    let c = git.run_raw(&args)?;
    Ok(c.stdout
        .split(|b| *b == 0)
        .filter_map(RepoPath::new)
        .collect())
}

/// Runs one hook alone (the default model): start segment first when it is next in line.
fn run_sequential(
    l: &Loop<'_>,
    out: &mut Out<'_>,
    oracle: &mut DiffOracle,
    fixes: &mut Fixes,
    jobs: Vec<Job<'_>>,
) -> Result<Vec<(usize, Done)>, Failure> {
    let mut finished = Vec::new();
    for job in jobs {
        if out.can_start(job.pos) {
            out.start(job.pos, &job.hook.def.name);
        }
        finished.push((job.pos, run_one(l, oracle, fixes, &job)?));
    }
    Ok(finished)
}

/// One executed hook: run, modification decision, `stage_fixed`.
fn run_one(
    l: &Loop<'_>,
    oracle: &mut DiffOracle,
    fixes: &mut Fixes,
    job: &Job<'_>,
) -> Result<Done, Failure> {
    let e = l.e;
    let root = e.git.root();
    let h = job.hook;
    let sf = stage_fixed(l, h);
    let tracking = sf.is_some() || !fixes.accepted.is_empty();
    let dirty_before = if sf.is_some() {
        dirty(e.git)?
    } else {
        BTreeSet::new()
    };
    let input_stamps = if sf.is_some() {
        stamp::stamps(root, &job.inputs)
    } else {
        BTreeMap::new()
    };
    let t0 = e.cx.clock.now();
    oracle.before_hook()?;
    let ran = exec::run_hook(e, h, &job.argv_files)?;
    let duration = Some(secs(e, t0));
    let mut change = decide(oracle, h, &ran)?;
    let mut ran = ran;
    let mut ext_lines = std::mem::take(&mut ran.ext_lines);
    let mut accepted_now = BTreeSet::new();
    if let (Some(mode), Change::Changed) = (sf, change) {
        let dirty_after = dirty(e.git)?;
        let inputs: BTreeSet<&RepoPath> = job.inputs.iter().collect();
        let others = dirty_after
            .difference(&dirty_before)
            .any(|p| !inputs.contains(p));
        let m = stamp::changed(root, &input_stamps);
        if !others && !m.is_empty() {
            let accept = if ran.code == 0 || mode == StageFixed::Trust {
                true
            } else {
                // `verify`: one re-run on the same inputs; it must pass without new changes
                oracle.before_hook()?;
                let again = exec::run_hook(e, h, &job.argv_files)?;
                let c2 = decide(oracle, h, &again)?;
                let ok = again.code == 0 && c2 == Change::Unchanged;
                ran = again;
                ok
            };
            if accept {
                fixes.accept(root, &m);
                accepted_now.extend(m);
                change = Change::Unchanged;
                ran.code = 0;
                ext_lines.push(messages::STAGED_FIXES.to_owned());
            }
        }
    }
    if tracking {
        fixes.check_after(root, &accepted_now);
    }
    Ok(finish(l, h, ran, change, duration, ext_lines))
}

/// The modification decision of an executed hook (§3.7).
fn decide(oracle: &mut DiffOracle, h: &ResolvedHook, ran: &Ran) -> Result<Change, Failure> {
    if ran.reads_only || matches!(h.exec, ExecKind::Meta(_)) {
        return Ok(Change::Unchanged);
    }
    match &ran.native_modified {
        Some(paths) => oracle.after_native(!paths.is_empty(), !paths.is_empty()),
        None => oracle.after_hook(),
    }
}

/// Status, detail lines and `retval` contribution of an executed hook.
fn finish(
    l: &Loop<'_>,
    h: &ResolvedHook,
    ran: Ran,
    change: Change,
    duration: Option<f64>,
    mut ext_lines: Vec<String>,
) -> Done {
    let modified = change == Change::Changed;
    if let Some(t) = ran.timed_out {
        ext_lines.push(messages::timed_out(t));
    }
    let failed = ran.code != 0 || ran.timed_out.is_some() || (modified && changes_fail(l, h)) || {
        // a missing `root` directory fails the hook with its detail line
        ext_lines.iter().any(|x| x.starts_with("- root directory"))
    };
    if failed && let Some(t) = &h.def.ext.fail_text {
        ext_lines.push(messages::fail_text(t));
    }
    Done {
        status: if failed {
            Status::Failed
        } else {
            Status::Passed
        },
        code: ran.code,
        modified,
        duration,
        ext_lines,
        out: ran.out,
        failed,
    }
}

/// A bucket of several hooks: run concurrently, attribute the changes (§3.7 item 6).
fn run_bucket(
    l: &Loop<'_>,
    oracle: &mut DiffOracle,
    _fixes: &Fixes,
    jobs: Vec<Job<'_>>,
) -> Result<Vec<(usize, Done)>, Failure> {
    let e = l.e;
    let root = e.git.root();
    let dirty_before = dirty(e.git)?;
    let watched: BTreeSet<RepoPath> = jobs
        .iter()
        .flat_map(|j| j.inputs.iter().cloned())
        .chain(dirty_before.iter().cloned())
        .collect();
    let before = stamp::stamps(root, &watched);
    oracle.before_hook()?;
    let results: Vec<Result<(Ran, Option<f64>), Failure>> = std::thread::scope(|s| {
        // every thread is spawned before the first join (the hooks of a bucket run concurrently)
        #[expect(
            clippy::needless_collect,
            reason = "spawn all threads before joining any"
        )]
        let handles: Vec<_> = jobs
            .iter()
            .map(|j| {
                std::thread::Builder::new().spawn_scoped(s, move || {
                    let t0 = e.cx.clock.now();
                    let r = exec::run_hook(e, j.hook, &j.argv_files);
                    r.map(|ran| (ran, Some(secs(e, t0))))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| match h {
                Ok(h) => h.join().unwrap_or_else(|p| Err(panic_failure(&p))),
                Err(err) => Err(Failure::from_io(&err, None, None)),
            })
            .collect()
    });
    let mut rans = Vec::new();
    for r in results {
        rans.push(r?);
    }
    let any_writer = rans
        .iter()
        .zip(&jobs)
        .any(|((r, _), j)| !(r.reads_only || matches!(j.hook.exec, ExecKind::Meta(_))));
    let bucket_change = if any_writer {
        oracle.after_hook()?
    } else {
        Change::Unchanged
    };
    let mut blamed: BTreeSet<usize> = BTreeSet::new();
    if bucket_change == Change::Changed {
        let dirty_after = dirty(e.git)?;
        let mut changed: BTreeSet<RepoPath> = stamp::changed(root, &before).into_iter().collect();
        changed.extend(dirty_after.difference(&dirty_before).cloned());
        let writers: Vec<usize> = rans
            .iter()
            .zip(&jobs)
            .enumerate()
            .filter(|(_, ((r, _), j))| !(r.reads_only || matches!(j.hook.exec, ExecKind::Meta(_))))
            .map(|(i, _)| i)
            .collect();
        if changed.is_empty() {
            blamed.extend(writers.iter().copied());
        }
        for p in &changed {
            let owners: Vec<usize> = writers
                .iter()
                .copied()
                .filter(|i| jobs.get(*i).is_some_and(|j| j.inputs.contains(p)))
                .collect();
            if owners.len() == 1 {
                blamed.extend(owners);
            } else {
                // not attributable to exactly one hook: every hook that could have caused it fails (fail closed)
                blamed.extend(writers.iter().copied());
            }
        }
    }
    Ok(rans
        .into_iter()
        .zip(jobs)
        .enumerate()
        .map(|(i, ((mut ran, duration), job))| {
            let change = if blamed.contains(&i) {
                Change::Changed
            } else {
                Change::Unchanged
            };
            let ext = std::mem::take(&mut ran.ext_lines);
            (job.pos, finish(l, job.hook, ran, change, duration, ext))
        })
        .collect())
}

/// A panicking hook thread, as the envelope reports it.
fn panic_failure(p: &Box<dyn std::any::Any + Send>) -> Failure {
    let msg = p
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    Failure::Exc {
        kind: snp_core::failure::ExcKind::Unexpected,
        exc: snp_core::failure::PyExc::panic(&msg, None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_payloads() {
        let p: Box<dyn std::any::Any + Send> = Box::new("boom");
        match panic_failure(&p) {
            Failure::Exc { exc, .. } => assert_eq!(exc.ty, "PanicException"),
            other => panic!("{other:?}"),
        }
    }
}
