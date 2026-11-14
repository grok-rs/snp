//! The hook loop (`run._run_hooks` / `_run_single_hook`, `run.py:142-322`; ARCHITECTURE §3.4, §3.7–§3.9, §8.3).
//!
//! Without extension keys every hook is its own bucket, in config order — pre-commit's sequential loop, with the
//! `name....` start segment written and flushed before the hook runs. With `priority` the hooks of one bucket run
//! concurrently (hook threads hold no pool permit), a bucket is a barrier, and blocks print head-of-line in config
//! order with bytes identical to the sequential rendering.
//!
//! Owner: WP-32.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::Path;
use std::time::Instant;

use snp_config::ext::{FailOnChanges, OutputMode, Priority, RootExt, StageFixed};
use snp_core::failure::Failure;
use snp_core::lang::LanguageId;
use snp_core::output::Printer;
use snp_core::path::RepoPath;
use snp_files::FileId;
use snp_files::{Classifier, HookFilter};
use snp_git::Git;
use snp_proc::tty::Terminal;
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
    /// `SNP_NO_TTY`: `interactive` hooks run like any other hook (ADR-085).
    pub(crate) no_tty: bool,
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
    /// The files passed (`()` when `pass_filenames: false`), relative to the hook's cwd: borrowed from the run's
    /// file table (an owned name only where the table's view is not the argv bytes; ADR-103).
    argv_files: Vec<Cow<'h, OsStr>>,
    /// The hook's input files; their root-relative [`RepoPath`]s ([`input_paths`]) are built only where they are
    /// consumed (`stage_fixed` stamps, bucket attribution).
    ids: Vec<FileId>,
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

/// Buckets of positions: ascending priority, config order inside a bucket. An `interactive` hook is a bucket of its
/// own (ADR-085): the members of its priority before it in config order form one bucket, the ones after it another,
/// so nothing else runs while it owns the terminal.
fn buckets(hooks: &[&ResolvedHook], root: &RootExt) -> Vec<Vec<usize>> {
    let mut by: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (i, h) in hooks.iter().enumerate() {
        by.entry(priority(h, root)).or_default().push(i);
    }
    let mut out = Vec::new();
    for bucket in by.into_values() {
        let mut cur = Vec::new();
        for pos in bucket {
            if hooks.get(pos).is_some_and(|h| h.def.ext.interactive) {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(vec![pos]);
            } else {
                cur.push(pos);
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
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
    /// Blocks printed ahead of config order (before or of an `interactive` hook); `flush` steps over them.
    printed: BTreeSet<usize>,
    /// `interactive` hooks whose block prints as soon as it is done.
    eager: BTreeSet<usize>,
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

    /// Prints every finished block that is next in config order, then the finished `interactive` blocks.
    fn flush(&mut self, hooks: &[&ResolvedHook]) {
        loop {
            if let Some(d) = self.done.remove(&self.next) {
                if let Some(h) = hooks.get(self.next) {
                    self.print(h, &d, self.started == Some(self.next));
                }
                self.summary.push((self.next, d.status, d.duration));
            } else if !self.printed.remove(&self.next) {
                break;
            }
            self.next = self.next.saturating_add(1);
        }
        let eager: Vec<usize> = self
            .eager
            .iter()
            .copied()
            .filter(|p| self.done.contains_key(p))
            .collect();
        for pos in eager {
            self.eager.remove(&pos);
            self.print_ahead(hooks, pos);
        }
    }

    /// Prints the finished block at `pos` now, ahead of config order.
    fn print_ahead(&mut self, hooks: &[&ResolvedHook], pos: usize) {
        if let Some(d) = self.done.remove(&pos) {
            if let Some(h) = hooks.get(pos) {
                self.print(h, &d, self.started == Some(pos));
            }
            self.summary.push((pos, d.status, d.duration));
            self.printed.insert(pos);
        }
    }

    /// Before an `interactive` hook runs (ADR-085): every finished block prints now (with `priority`, ahead of hooks
    /// that run later), then `{name}....(interactive)`, flushed before the hook takes the terminal; its block prints
    /// as soon as it is done.
    fn begin_interactive(&mut self, hooks: &[&ResolvedHook], pos: usize, name: &str) {
        self.flush(hooks);
        let held: Vec<usize> = self.done.keys().copied().collect();
        for p in held {
            self.print_ahead(hooks, p);
        }
        self.flush(hooks);
        self.eager.insert(pos);
        self.printer
            .write(render::interactive_msg(name, self.cols).as_bytes());
        self.printer.flush();
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
                snp_core::failure::pyerr::os_error_text(&e, Some(OsStr::new(name)), None);
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
        printed: BTreeSet::new(),
        eager: BTreeSet::new(),
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
            let (argv_files, ids) = filenames(l, h)?;
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
                    ids,
                });
            }
        }
        // statuses that need no run print at once when they are next in line
        out.flush(l.hooks);
        for group in groups(l, jobs) {
            let finished = if group.len() == 1 {
                run_sequential(l, &mut out, oracle, &mut fixes, group)?
            } else {
                run_bucket(l, oracle, &mut fixes, group)?
            };
            for (pos, d) in finished {
                let h = l.hooks.get(pos);
                if d.failed {
                    res.retval = 1;
                    if l.fail_fast || h.is_some_and(|h| h.def.fail_fast) {
                        // the bucket is the unit (§3.1 step 18): its remaining members still run
                        stop = true;
                    }
                }
                out.done.insert(pos, d);
            }
            out.flush(l.hooks);
        }
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
    // belt and braces: a path changed after its acceptance is never staged, whatever ran after it
    fixes.check_after(root, &BTreeSet::new());
    let (to_stage, withdrawn) = fixes.into_paths();
    for p in &withdrawn {
        printer.diag(&messages::stage_fixed_withdrawn(&p.lossy()));
    }
    res.to_stage = to_stage;
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
/// unless the hook never consults files; `(argv files relative to the hook cwd, the files' ids)`. The argv names are
/// borrowed from the classifier's file table (ADR-103).
fn filenames<'a>(
    l: &Loop<'a>,
    h: &ResolvedHook,
) -> Result<(Vec<Cow<'a, OsStr>>, Vec<FileId>), Failure> {
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
    let table = &c.table;
    let argv = match &h.def.ext.root {
        Some(prefix) => ids
            .iter()
            .filter_map(|&id| table.relative_to(id, prefix).map(os_cow))
            .collect(),
        None => ids.iter().map(|&id| table.os(id)).collect(),
    };
    Ok((argv, ids))
}

/// Table bytes as an argv name: borrowed when the bytes are the name (Unix always; elsewhere valid UTF-8), else the
/// owned conversion of [`crate::files::os_from_bytes`].
fn os_cow(b: Cow<'_, [u8]>) -> Cow<'_, OsStr> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
        match b {
            Cow::Borrowed(b) => Cow::Borrowed(OsStr::from_bytes(b)),
            Cow::Owned(v) => Cow::Owned(std::ffi::OsString::from_vec(v)),
        }
    }
    #[cfg(not(unix))]
    {
        match b {
            Cow::Borrowed(b) => match std::str::from_utf8(b) {
                Ok(s) => Cow::Borrowed(OsStr::new(s)),
                Err(_) => Cow::Owned(crate::files::os_from_bytes(b)),
            },
            Cow::Owned(v) => Cow::Owned(crate::files::os_from_bytes(&v)),
        }
    }
}

/// The root-relative paths of a job's input files (`RepoPath::new` of each table name; the invalid ones dropped, so
/// an empty list still means "no valid input path").
fn input_paths(l: &Loop<'_>, ids: &[FileId]) -> Vec<RepoPath> {
    let Some(c) = l.classifier else {
        return Vec::new();
    };
    ids.iter()
        .filter_map(|&id| RepoPath::new(c.table.bytes(id)))
        .collect()
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

/// Whether an accepted `stage_fixed` fix still fails the hook (§8.3: "`fail_on_changes` combines", e.g. stage
/// locally, fail in CI). Only an explicit setting combines — the hook's own key, or a root value other than
/// pre-commit's default `always` (which would otherwise fail every accepted fix and make `stage_fixed` inert).
fn staged_fix_fails(l: &Loop<'_>, h: &ResolvedHook) -> bool {
    let explicit = h
        .def
        .ext
        .fail_on_changes
        .or_else(|| (l.fail_on_changes != FailOnChanges::Always).then_some(l.fail_on_changes));
    explicit.is_some() && changes_fail(l, h)
}

/// The active `stage_fixed` mode of a hook.
fn stage_fixed(l: &Loop<'_>, h: &ResolvedHook) -> Option<StageFixed> {
    h.def.ext.stage_fixed.filter(|_| l.stage_fixed)
}

/// The execution groups of one bucket: a hook with an active `stage_fixed` runs alone first (its M/O rule needs
/// exclusive access to the worktree), then the remaining hooks together. Output order is unaffected (head-of-line).
fn groups<'h>(l: &Loop<'_>, jobs: Vec<Job<'h>>) -> Vec<Vec<Job<'h>>> {
    if jobs.len() <= 1 {
        return if jobs.is_empty() {
            Vec::new()
        } else {
            vec![jobs]
        };
    }
    let (alone, rest): (Vec<Job<'h>>, Vec<Job<'h>>) = jobs
        .into_iter()
        .partition(|j| stage_fixed(l, j.hook).is_some());
    let mut out: Vec<Vec<Job<'h>>> = alone.into_iter().map(|j| vec![j]).collect();
    if !rest.is_empty() {
        out.push(rest);
    }
    out
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
        if self.accepted.is_empty() {
            return;
        }
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

    /// `(paths to stage, accepted paths withdrawn because a later hook changed them)`.
    fn into_paths(self) -> (Vec<RepoPath>, Vec<RepoPath>) {
        let poisoned = self.poisoned;
        self.accepted
            .into_keys()
            .partition(|p| !poisoned.contains(p))
    }
}

/// `git diff --name-only -z` (worktree vs index) as root-relative paths: the tracked files currently dirty.
fn dirty(git: &Git) -> Result<BTreeSet<RepoPath>, Failure> {
    let args = [
        OsStr::new("diff"),
        OsStr::new("--name-only"),
        OsStr::new("-z"),
        OsStr::new("--no-ext-diff"),
        OsStr::new("--no-textconv"),
        OsStr::new("--ignore-submodules"),
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
        let tty = terminal(l, job.hook);
        if tty.is_some() {
            out.begin_interactive(l.hooks, job.pos, &job.hook.def.name);
        } else if out.can_start(job.pos) {
            out.start(job.pos, &job.hook.def.name);
        }
        let d = run_one(l, oracle, fixes, &job, tty.as_ref())?;
        // a killed interactive hook (timeout, signal) was cut off mid-prompt: its status line starts a new line
        if tty.is_some() && d.code < 0 {
            out.printer.write(b"\n");
        }
        finished.push((job.pos, d));
    }
    Ok(finished)
}

/// The terminal of an `interactive` process hook (ADR-085). `None` for every other hook, under `SNP_NO_TTY`, and when
/// no terminal is usable — then after a one-line stderr note, and the hook runs like any other.
fn terminal(l: &Loop<'_>, h: &ResolvedHook) -> Option<Terminal> {
    let spawns = matches!(h.exec, ExecKind::Language)
        && !matches!(h.def.language, LanguageId::Fail | LanguageId::Pygrep);
    if !h.def.ext.interactive || !spawns || l.no_tty {
        return None;
    }
    match Terminal::open() {
        Ok(t) => Some(t),
        Err(why) => {
            l.e.cx
                .printer
                .diag(&messages::interactive_no_tty(&h.def.id, &why.to_string()));
            None
        }
    }
}

/// One executed hook: run, modification decision, `stage_fixed`.
fn run_one(
    l: &Loop<'_>,
    oracle: &mut DiffOracle,
    fixes: &mut Fixes,
    job: &Job<'_>,
    tty: Option<&Terminal>,
) -> Result<Done, Failure> {
    let e = l.e;
    let root = e.git.root();
    let h = job.hook;
    let sf = stage_fixed(l, h);
    // the input paths and their set exist only for an active `stage_fixed`
    let input_list = if sf.is_some() {
        input_paths(l, &job.ids)
    } else {
        Vec::new()
    };
    let inputs: BTreeSet<&RepoPath> = input_list.iter().collect();
    let dirty_before = if sf.is_some() {
        dirty(e.git)?
    } else {
        BTreeSet::new()
    };
    // M: the hook's inputs; O also counts files an earlier hook already dirtied (stamped: usually few)
    let (input_stamps, other_stamps) = if sf.is_some() {
        (
            stamp::stamps(root, &input_list),
            stamp::stamps(root, dirty_before.iter().filter(|p| !inputs.contains(p))),
        )
    } else {
        (BTreeMap::new(), BTreeMap::new())
    };
    let t0 = e.cx.clock.now();
    oracle.before_hook()?;
    let ran = exec::run_hook(e, h, &job.argv_files, tty)?;
    let duration = Some(secs(e, t0));
    let mut change = decide(oracle, h, &ran)?;
    let mut ran = ran;
    let mut ext_lines = std::mem::take(&mut ran.ext_lines);
    let mut accepted_now = BTreeSet::new();
    let mut staged = false;
    if let (Some(mode), Change::Changed) = (sf, change) {
        let dirty_after = dirty(e.git)?;
        let others = dirty_after
            .difference(&dirty_before)
            .any(|p| !inputs.contains(p))
            || !stamp::changed(root, &other_stamps).is_empty();
        let m = stamp::changed(root, &input_stamps);
        if !others && !m.is_empty() {
            let accept = if ran.code == 0 || mode == StageFixed::Trust {
                true
            } else {
                // `verify`: one re-run on the same inputs; it must pass without new changes
                oracle.before_hook()?;
                let again = exec::run_hook(e, h, &job.argv_files, tty)?;
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
                staged = true;
                ext_lines.push(messages::STAGED_FIXES.to_owned());
            }
        }
    }
    fixes.check_after(root, &accepted_now);
    let mut d = finish(l, h, ran, change, duration, ext_lines);
    if staged && !d.failed && staged_fix_fails(l, h) {
        d = Done {
            status: Status::Failed,
            failed: true,
            ..d
        };
        if let Some(t) = &h.def.ext.fail_text {
            d.ext_lines.push(messages::fail_text(t));
        }
    }
    Ok(d)
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

/// The most hook threads one bucket runs at once (hook threads hold no pool permit; their children do).
fn bucket_threads(l: &Loop<'_>, jobs: usize) -> usize {
    let cap = l.e.cx.pool.permits().get().saturating_mul(2).max(2);
    jobs.min(cap).max(1)
}

/// A bucket of several hooks: run concurrently, attribute the changes (§3.7 item 6).
fn run_bucket(
    l: &Loop<'_>,
    oracle: &mut DiffOracle,
    fixes: &mut Fixes,
    jobs: Vec<Job<'_>>,
) -> Result<Vec<(usize, Done)>, Failure> {
    let e = l.e;
    let root = e.git.root();
    let dirty_before = dirty(e.git)?;
    let inputs: Vec<Vec<RepoPath>> = jobs.iter().map(|j| input_paths(l, &j.ids)).collect();
    let watched: BTreeSet<RepoPath> = inputs
        .iter()
        .flatten()
        .cloned()
        .chain(dirty_before.iter().cloned())
        .collect();
    let before = stamp::stamps(root, &watched);
    oracle.before_hook()?;
    let rans = run_concurrently(l, &jobs)?;
    let is_writer =
        |r: &Ran, j: &Job<'_>| !(r.reads_only || matches!(j.hook.exec, ExecKind::Meta(_)));
    let any_writer = rans.iter().zip(&jobs).any(|((r, _), j)| is_writer(r, j));
    let bucket_change = if any_writer {
        oracle.after_hook()?
    } else {
        Change::Unchanged
    };
    let blamed = if bucket_change == Change::Changed {
        let dirty_after = dirty(e.git)?;
        let mut changed: BTreeSet<RepoPath> = stamp::changed(root, &before).into_iter().collect();
        changed.extend(dirty_after.difference(&dirty_before).cloned());
        attribute(&rans, &jobs, &inputs, &changed, is_writer)
    } else {
        BTreeSet::new()
    };
    // an accepted fix that a hook of this bucket changed again is never staged (§8.3)
    fixes.check_after(root, &BTreeSet::new());
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

/// One executed hook of a bucket and its duration.
type BucketRun = (Ran, Option<f64>);

/// What one bucket worker ran: `(job index, result)`.
type WorkerRuns = Vec<(usize, Result<BucketRun, Failure>)>;

/// Runs `jobs` on at most [`bucket_threads`] hook threads; each worker takes the next job when its hook ends.
/// Results come back in job order.
fn run_concurrently(lp: &Loop<'_>, jobs: &[Job<'_>]) -> Result<Vec<BucketRun>, Failure> {
    let env = lp.e;
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let workers = bucket_threads(lp, jobs.len());
    let joined: Vec<Result<WorkerRuns, Failure>> = std::thread::scope(|s| {
        let cursor = &cursor;
        // every worker is spawned before the first join (the hooks of a bucket run concurrently)
        #[expect(
            clippy::needless_collect,
            reason = "spawn all workers before joining any"
        )]
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                std::thread::Builder::new().spawn_scoped(s, move || {
                    let mut mine = Vec::new();
                    loop {
                        let idx = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(job) = jobs.get(idx) else {
                            break;
                        };
                        let t0 = env.cx.clock.now();
                        let r = exec::run_hook(env, job.hook, &job.argv_files, None)
                            .map(|ran| (ran, Some(secs(env, t0))));
                        mine.push((idx, r));
                    }
                    mine
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| match h {
                Ok(h) => h.join().map_err(|p| panic_failure(&p)),
                Err(err) => Err(Failure::from_io(&err, None, None)),
            })
            .collect()
    });
    let mut slots: Vec<Option<Result<BucketRun, Failure>>> =
        std::iter::repeat_with(|| None).take(jobs.len()).collect();
    let mut worker_err = None;
    for w in joined {
        match w {
            Ok(mine) => {
                for (idx, r) in mine {
                    if let Some(slot) = slots.get_mut(idx) {
                        *slot = Some(r);
                    }
                }
            }
            Err(f) => {
                worker_err.get_or_insert(f);
            }
        }
    }
    let mut rans = Vec::new();
    for slot in slots {
        match slot {
            Some(r) => rans.push(r?),
            // a job no worker finished: a worker failed to start or panicked
            None => {
                return Err(worker_err.unwrap_or_else(|| {
                    Failure::unexpected("RuntimeError", "a bucket hook did not run".to_owned())
                }));
            }
        }
    }
    Ok(rans)
}

/// §3.7 item 6: the bucket members a modification fails. A changed path is attributed to one writer only when no
/// other writer could have written it: the single writer whose inputs contain it, where every writer that got no
/// file list (`pass_filenames: false`, or no inputs) could have written any path. Otherwise every writer fails
/// (conservative, fail closed). A writer can still write outside its inputs undetected by this rule; the run's exit
/// status is never affected (some hook always fails), only which one is named.
/// `inputs` holds each job's [`input_paths`].
fn attribute(
    rans: &[BucketRun],
    jobs: &[Job<'_>],
    inputs: &[Vec<RepoPath>],
    changed: &BTreeSet<RepoPath>,
    is_writer: impl Fn(&Ran, &Job<'_>) -> bool,
) -> BTreeSet<usize> {
    let writers: Vec<(usize, Option<BTreeSet<&RepoPath>>)> = rans
        .iter()
        .zip(jobs)
        .enumerate()
        .filter(|(_, ((r, _), j))| is_writer(r, j))
        .map(|(i, (_, j))| {
            let mine: &[RepoPath] = inputs.get(i).map_or(&[], Vec::as_slice);
            let unconstrained = !j.hook.def.pass_filenames || mine.is_empty();
            (i, (!unconstrained).then(|| mine.iter().collect()))
        })
        .collect();
    let all: BTreeSet<usize> = writers.iter().map(|(i, _)| *i).collect();
    if changed.is_empty() {
        return all;
    }
    let mut blamed = BTreeSet::new();
    for p in changed {
        let mut owners = writers
            .iter()
            .filter(|(_, inputs)| inputs.as_ref().is_none_or(|s| s.contains(p)))
            .map(|(i, _)| *i);
        match (owners.next(), owners.next()) {
            (Some(only), None) => {
                blamed.insert(only);
            }
            _ => return all,
        }
    }
    blamed
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
