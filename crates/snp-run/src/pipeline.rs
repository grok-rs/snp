//! The `run` pipeline (ARCHITECTURE §3.1, §2.4, §4.4): `RunState` with its panic-path restore, and the body of
//! `commands/run.py:run` shared by `run`, `hook-impl` and `try-repo`.
//!
//! The body may return early anywhere: it only ever *takes* the stash guard into `RunState`, and the single exit in
//! [`run_pipeline`] restores it explicitly (on the cancel path after `registry::quiesce`), so the restore runs on
//! every path and its failure replaces the body's result (an exception raised in a Python `finally`). A panic
//! unwinds into `RunState::drop`, which restores before any field is dropped (ADR-066).
//!
//! Owner: WP-32.

use std::ffi::{OsStr, OsString};
use std::io::IsTerminal as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use snp_cli::RunArgs;
use snp_config::Config;
use snp_config::ext::OutputMode;
use snp_core::cancel::CancelReason;
use snp_core::env::{Env, EnvPatch};
use snp_core::failure::{Failure, PyExc};
use snp_core::output::Level;
use snp_core::stage::{HookType, Stage};
use snp_files::Classifier;
use snp_git::{DiffFingerprint, Git, GitVersion, IndexEntries, ProcBackend, RepoInfo};
use snp_proc::spawn::Role;
use snp_pyre::Options;
use snp_repo::ResolvedHook;
use snp_stash::diff_oracle::DiffOracle;
use snp_stash::{StashCtx, StashGuard, Stashed};
use snp_store::{Store, StoreGuard};

use crate::common::{self, Buffered, ResolveParts};
use crate::exec::ExecEnv;
use crate::files::{self, Selection};
use crate::select::{Conditions, GroupFilter, Skips};
use crate::{RunCtx, messages, meta, render, runenv, sched};

/// How a pipeline was entered.
#[derive(Debug)]
pub(crate) struct Invocation<'s> {
    /// The store hooks resolve against (try-repo: its throwaway store).
    pub(crate) store: &'s Store,
    /// `store.mark_config_used(config)` (not for try-repo's temporary config).
    pub(crate) track_config: bool,
    /// `run` started directly (the D-DIAG-03 hint).
    pub(crate) direct: bool,
    /// The config and file paths are final (hook-impl: git's root-relative arguments, already adjusted by the
    /// hook-impl command; try-repo: the absolute temporary config): `_adjust_args_and_chdir` is not applied again.
    pub(crate) fixed_paths: bool,
    /// The git hook's stdin (pre-push ref lines) for `use_stdin`.
    pub(crate) hook_stdin: Option<Arc<[u8]>>,
}

/// Lives on the stack for one pipeline (§2.4): the resources whose release order is part of the safety argument.
#[derive(Debug, Default)]
struct RunState {
    stash: Option<StashGuard>,
    store_guard: Option<StoreGuard>,
    /// A field whose own `Drop` panics (the ADR-066 drop-order test); only dropped, never read.
    #[cfg(test)]
    #[expect(dead_code, reason = "exists for its Drop")]
    probe: Option<tests::PanicOnDrop>,
}

impl Drop for RunState {
    /// Runs BEFORE any field is dropped: the panic-path restore (the normal and error paths restore explicitly).
    fn drop(&mut self) {
        if let Some(g) = self.stash.take() {
            // the guard reports a failed restore on stderr itself; nothing else can be done while unwinding
            drop(catch_unwind(AssertUnwindSafe(|| g.restore())));
        }
    }
}

/// The bound `registry::quiesce` waits for hook children on the cancel path before the restore.
const QUIESCE: Duration = Duration::from_secs(2);

/// Runs the pipeline with its single exit (§4.4).
pub(crate) fn run_pipeline(a: &RunArgs, cx: &RunCtx, inv: &Invocation<'_>) -> Result<i32, Failure> {
    let mut st = RunState::default();
    let body = body(&mut st, cx, a, inv);
    let cancelled = cx.cancel.reason();
    if cancelled.is_some() && st.stash.is_some() {
        snp_proc::registry::quiesce(QUIESCE);
    }
    let restored = match st.stash.take() {
        Some(g) => g.restore(),
        None => Ok(()),
    };
    drop(st.store_guard.take());
    // a restore failure replaces everything (an exception raised in a Python `finally`); after a cancellation the
    // body's own error is an artefact of it (a git call killed by the escalation), so the cancellation is reported
    match (body, restored, cancelled.or_else(|| cx.cancel.reason())) {
        (_, Err(r), _) => Err(r),
        (_, Ok(()), Some(CancelReason::StdoutClosed)) => Err(Failure::StdoutClosed),
        (_, Ok(()), Some(r)) => Err(Failure::Interrupted(r)),
        (Err(f), Ok(()), None) => Err(f),
        (Ok(code), Ok(()), None) => Ok(code),
    }
}

/// `run.py:run` from `_adjust_args_and_chdir` on.
#[expect(
    clippy::too_many_lines,
    reason = "pre-commit's run() in source order, one step after another"
)]
fn body(st: &mut RunState, cx: &RunCtx, a: &RunArgs, inv: &Invocation<'_>) -> Result<i32, Failure> {
    let printer = &*cx.printer;
    // §3.1 steps 3–4: discovery, then `_adjust_args_and_chdir` as a pure function
    let (git0, info) = common::discover(cx)?;
    let root = info.root.clone();
    let rebase = |f: &OsString| {
        if inv.fixed_paths {
            f.clone()
        } else {
            common::rebase_path(f, &cx.invocation_cwd, &root)
        }
    };
    let config = if inv.fixed_paths {
        a.config.clone()
    } else {
        common::adjust_config(&a.config, &cx.invocation_cwd, &root)
    };
    let cli_files = match a.ext.stdin_files {
        Some(fmt) if cx.brand == snp_core::brand::Brand::Snp => {
            files::read_stdin_files(fmt, &mut std::io::stdin().lock())?
        }
        _ => a.files.clone(),
    };
    let cli_files: Vec<OsString> = cli_files.iter().map(rebase).collect();
    let commit_msg_filename = a.commit_msg_filename.as_ref().map(rebase);
    let src = common::config_source(&config, &root);
    // step 5: config tracking (main store, best effort) and the next-run recovery notice
    if inv.track_config {
        common::mark_config_used(&cx.store, &src.path, &root);
    }
    let stash = !a.all_files && cli_files.is_empty();
    // A stashing run reports orphaned stashes inside `snp_stash::enter`, which holds the worktree lock anyway
    // (ARCHITECTURE §4.6); scanning here as well printed every notice twice.
    if !stash && cx.env.get("_SNP_STASH_LOCK").is_none() {
        for u in snp_stash::recovery_scan(&info.git_dir, &cx.store.patches_dir(), &root) {
            printer.diag(&u.hint());
        }
    }
    // step 6: the speculative config load (warnings buffered) beside the read-only preflight git calls
    let buffered = Buffered::default();
    let (loaded, unmerged, unstaged) = std::thread::scope(|s| {
        let load = std::thread::Builder::new().spawn_scoped(s, || {
            common::load_config_with(cx, &src, &buffered.warn, &buffered.diag)
        });
        let unstaged = std::thread::Builder::new()
            .spawn_scoped(s, || stash.then(|| git0.config_is_unstaged(&config)));
        let unmerged = stash.then(|| git0.has_unmerged());
        let unstaged = match unstaged {
            Ok(h) => h.join().unwrap_or_else(|p| Some(Err(panicked(&p)))),
            Err(_) => stash.then(|| git0.config_is_unstaged(&config)),
        };
        let loaded = match load {
            Ok(h) => h.join().unwrap_or_else(|p| Err(panicked(&p))),
            Err(_) => common::load_config_with(cx, &src, &buffered.warn, &buffered.diag),
        };
        (loaded, unmerged, unstaged)
    });
    if inv.direct
        && let Ok(cfg) = &loaded
    {
        run_hint(cx, &git0, &info, cfg);
    }
    // step 7: preflight in pre-commit's order
    if unmerged.transpose()?.unwrap_or(false) {
        printer.log(Level::Error, messages::UNMERGED);
        return Ok(1);
    }
    let from = a.from_ref.as_ref().filter(|r| !r.is_empty());
    let to = a.to_ref.as_ref().filter(|r| !r.is_empty());
    if from.is_some() != to.is_some() {
        printer.log(Level::Error, messages::BOTH_REFS);
        return Ok(1);
    }
    if unstaged.transpose()?.unwrap_or(false) {
        printer.log(
            Level::Error,
            &messages::config_unstaged(&config.to_string_lossy()),
        );
        return Ok(1);
    }
    if files::commit_msg_stage(a.hook_stage)
        && commit_msg_filename.as_ref().is_none_or(|f| f.is_empty())
    {
        printer.log(
            Level::Error,
            &messages::commit_msg_required(a.hook_stage.as_str()),
        );
        return Ok(1);
    }
    if a.hook_stage == Stage::Hook(HookType::PostCheckout)
        && cx
            .env
            .get_nonempty("_PRE_COMMIT_SKIP_POST_CHECKOUT")
            .is_some()
    {
        return Ok(0);
    }
    // step 8: run_env
    let mut run_env = runenv::run_env(&cx.env, a);
    // step 9: the stash guard
    if stash {
        let guard = snp_stash::enter(stash_ctx(cx, inv.store, &git0, &info, &run_env))?;
        if let Some((k, v)) = guard.child_env_token() {
            run_env = run_env.patched(&EnvPatch::default().set(k, v));
        }
        st.stash = Some(guard);
    }
    let git = git0.with_env(run_env.clone());
    // step 10: the config (warnings at pre-commit's position), the overlay
    buffered.replay(printer);
    let mut cfg = loaded?;
    let overlay = common::apply_overlay(cx, &mut cfg, &git, a.ext.no_local_config)?;
    if common::uses_store(&cfg) {
        st.store_guard = Some(common::store_lock(cx, inv.store)?);
    }
    // step 11: resolution
    let parts = ResolveParts::new(run_env.clone(), root.clone(), inv.store);
    let policy = common::fast_path_policy(&cfg, &cx.env, a.ext.no_fast_path, false);
    let rcx = parts.ctx(
        cx,
        &cfg,
        printer,
        printer,
        policy,
        overlay.as_ref(),
        &cx.cancel,
    );
    let t_resolve = cx.clock.now();
    let all = cx.resolver.all_hooks(&rcx)?;
    // step 12: selection
    let strict = common::ext_mode(&cx.env) == snp_config::ExtMode::Strict;
    let groups = GroupFilter::new(&a.ext.groups, &a.ext.require_groups, &a.ext.no_groups);
    let selectors: Vec<OsString> = a.hooks.iter().filter(|h| !h.is_empty()).cloned().collect();
    let hooks: Vec<&ResolvedHook> =
        match crate::select::select(&all, &selectors, a.hook_stage, &groups) {
            Ok(h) => h,
            Err(missing) => {
                printer.write_line(
                    messages::no_hook(&missing.to_string_lossy(), a.hook_stage.as_str()).as_bytes(),
                );
                return Ok(1);
            }
        };
    // step 13: SKIP, SNP_SKIP_GROUPS, skip/only conditions
    let skips = Skips::from_env(&run_env, strict);
    let conds = Conditions::new(&git, &info, &run_env, &cx.cancel, &cx.regexes);
    let root_skipped = !strict && conds.skipped(&cfg.root_ext.skip, &cfg.root_ext.only, &[]);
    let skipped: Vec<bool> = hooks
        .iter()
        .map(|h| {
            skips.skips(h)
                || (!strict
                    && (root_skipped
                        || conds.skipped(&h.def.ext.skip, &h.def.ext.only, &h.def.ext.env)))
        })
        .collect();
    let to_install: Vec<&ResolvedHook> = hooks
        .iter()
        .zip(&skipped)
        .filter(|(_, s)| !**s)
        .map(|(h, _)| *h)
        .collect();
    // step 14 (and the side fan-out: staged list and the DiffOracle inputs, read-only, beside the installs)
    let sel = Selection {
        stage: a.hook_stage,
        commit_msg_filename: commit_msg_filename.as_ref(),
        refs: from.zip(to),
        files: &cli_files,
        all_files: a.all_files,
    };
    let needs_files = hooks
        .iter()
        .any(|h| !h.def.always_run || h.def.pass_filenames);
    // F and G cost two spawns and save one `git diff` per executed process hook: worth it from three hooks on
    let process_hooks = to_install
        .iter()
        .filter(|h| matches!(h.exec, snp_repo::ExecKind::Language))
        .count();
    let stat_possible = process_hooks >= 3 && stat_possible(&git, &info, &run_env);
    let (installed, side) = std::thread::scope(|s| {
        let side = std::thread::Builder::new().spawn_scoped(s, || {
            side_queries(&git, &sel, &info, needs_files, stat_possible)
        });
        let installed = cx.resolver.install_envs(&to_install, &rcx);
        let side = match side {
            Ok(h) => h.join().unwrap_or_else(|p| Err(panicked(&p))),
            Err(_) => side_queries(&git, &sel, &info, needs_files, stat_possible),
        };
        (installed, side)
    });
    installed?;
    let side = side?;
    // step 15: cols, the file table, the classifier
    let names: Vec<&str> = hooks.iter().map(|h| &*h.def.name).collect();
    let cols = render::cols(&names);
    let classifier = if needs_files || sel.refs.is_some() {
        let table = files::table(&sel, &git, &info, printer, side.staged)?;
        let include = regex(cx, &cfg.files)?;
        let exclude = regex(cx, &cfg.exclude)?;
        Some(Classifier::new(
            Arc::new(table),
            &include,
            &exclude,
            &cx.pool,
        )?)
    } else {
        // P17: no hook reads the list, but the merge-conflict notice is printed exactly when pre-commit would
        if files::merge_branch(&sel, &git, &info) {
            printer.log(Level::Info, messages::MERGE_CONFLICT_ONLY);
        }
        None
    };
    // step 16: the baseline digest (ADR-078)
    let baseline = baseline(cx, st.stash.as_ref(), &git, &run_env, t_resolve);
    let mut oracle = DiffOracle::new(&git, &info, &run_env, baseline, side.tracked, side.origins)?;
    // step 17–18: execution
    let facts = snp_git::pch_facts::PchFacts::new(git.clone());
    let cpu = snp_proc::xargs::cpu_count();
    let max_len = snp_proc::xargs::platform_max_length(cx.env.python_environ_size());
    let color = printer.color();
    let meta_cx = meta::MetaCx {
        cx,
        git: &git,
        env: &run_env,
        store: &cx.store,
        rcx: &rcx,
    };
    let e = ExecEnv {
        cx,
        rcx: &rcx,
        git: &git,
        run_env: &run_env,
        color,
        cpu,
        max_len,
        facts: &facts,
        stdin: inv.hook_stdin.clone(),
        meta: &meta_cx,
    };
    let stage_fixed = !strict
        && a.hook_stage == Stage::Hook(HookType::PreCommit)
        && st.stash.is_some()
        && sel.refs.is_none();
    let lp = sched::Loop {
        e: &e,
        hooks: &hooks,
        skipped: &skipped,
        cols,
        verbose: a.verbose,
        fail_fast: cfg.fail_fast || a.fail_fast,
        classifier: classifier.as_ref(),
        mode: output_mode(&cfg, &run_env, a, strict),
        fail_on_changes: cfg.root_ext.fail_on_changes,
        ci: run_env.is_truthy_flag("CI"),
        stage_fixed,
        root_ext: &cfg.root_ext,
    };
    let res = sched::run_hooks(&lp, &mut oracle)?;
    let mut retval = res.retval;
    // step 19: --show-diff-on-failure (inside the stash)
    if retval != 0 && a.show_diff_on_failure && oracle.non_empty()? {
        if a.all_files {
            printer.write_line(messages::show_diff_all_files(cx.brand).as_bytes());
        }
        printer.write_line(messages::ALL_CHANGES.as_bytes());
        printer.flush();
        git.show_diff(color)?;
        snp_os::term::ensure_vt();
    }
    // step 20: stage_fixed staging (before the restore)
    if !res.to_stage.is_empty()
        && let Some(g) = st.stash.as_mut()
        && !g.stage_fixed(&res.to_stage)?
    {
        printer.diag(messages::STAGE_FIXED_REFUSED);
        retval = 1;
    }
    Ok(retval)
}

/// `_has_*` helpers report a panic of their thread as the envelope's `PanicException`.
fn panicked(p: &Box<dyn std::any::Any + Send>) -> Failure {
    let msg = p
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    Failure::Exc {
        kind: snp_core::failure::ExcKind::Unexpected,
        exc: PyExc::panic(&msg, None, None),
    }
}

fn regex(cx: &RunCtx, pattern: &str) -> Result<Arc<snp_pyre::PyRegex>, Failure> {
    cx.regexes
        .get(pattern, Options::CONFIG)
        .map_err(|e| Failure::unexpected("re.error", e.to_string()))
}

/// The stash's context: a Critical-role backend on a token that is never cancelled (ADR-065). The patch lives in
/// the run's store: try-repo's throwaway store keeps `patches/` in its temp dir, as pre-commit's `Store(tempdir)`
/// does (ADR-059), so `[INFO] Stashing unstaged files to <path>.` names the same kind of path.
fn stash_ctx(cx: &RunCtx, store: &Store, git: &Git, info: &RepoInfo, run_env: &Env) -> StashCtx {
    let backend = Arc::new(ProcBackend {
        cancel: snp_core::cancel::CancelToken::new(),
        role: Role::Critical,
    });
    StashCtx {
        git: git.with_env(run_env.clone()).with_backend(backend),
        info: info.clone(),
        patches_dir: store.patches_dir(),
        printer: Arc::clone(&cx.printer),
        cancel: cx.cancel.clone(),
        clock: Arc::clone(&cx.clock),
        signals: cx.signals.clone(),
        lock_timeout: StashCtx::lock_timeout_from_env(run_env),
        verify: run_env.is_truthy_flag("SNP_STASH_VERIFY"),
        old_git: cx.git_exe.caps.version.is_some_and(|v| {
            v < GitVersion {
                major: 2,
                minor: 19,
                patch: 0,
            }
        }),
    }
}

/// Whether the `DiffOracle`'s Stat mode can apply (Unix, ≤ 16 384 index entries, no `SNP_DIFF_MODE=git`, no
/// `GIT_ATTR_SOURCE`): only then are fan-out queries F and G taken.
fn stat_possible(git: &Git, info: &RepoInfo, env: &Env) -> bool {
    cfg!(unix)
        && env.get("SNP_DIFF_MODE").is_none_or(|v| v != "git")
        && env.get("GIT_ATTR_SOURCE").is_none()
        && git.index_entry_count(info).is_some_and(|n| n <= 16_384)
}

/// Fan-out queries E (staged list), F (`ls-files -s -z`) and G (`config --list --show-origin -z`).
#[derive(Debug, Default)]
struct Side {
    staged: Option<snp_git::ZList>,
    tracked: Option<Arc<IndexEntries>>,
    origins: Option<Arc<[PathBuf]>>,
}

fn side_queries(
    git: &Git,
    sel: &Selection<'_>,
    info: &RepoInfo,
    needs_files: bool,
    stat: bool,
) -> Result<Side, Failure> {
    // E, F and G are independent read-only queries: each on its own thread, so one spawn is on the critical path
    std::thread::scope(|s| {
        let spawn_f = || {
            std::thread::Builder::new()
                .spawn_scoped(s, || git.tracked_with_modes().ok().map(Arc::new))
        };
        let spawn_g = || {
            std::thread::Builder::new().spawn_scoped(s, || git.config_origins().ok().map(Arc::from))
        };
        let f = stat.then(spawn_f);
        let g = stat.then(spawn_g);
        let staged =
            (needs_files && files::wants_staged(sel, git, info)).then(|| git.staged_files());
        // a failure of F or G only costs the Stat mode (the oracle falls back to `git diff`); a thread that could
        // not start runs its query here
        let tracked = match f {
            Some(Ok(h)) => h.join().ok().flatten(),
            Some(Err(_)) => git.tracked_with_modes().ok().map(Arc::new),
            None => None,
        };
        let origins = match g {
            Some(Ok(h)) => h.join().ok().flatten(),
            Some(Err(_)) => git.config_origins().ok().map(Arc::from),
            None => None,
        };
        Ok(Side {
            staged: staged.transpose()?,
            tracked,
            origins,
        })
    })
}

/// The baseline of ADR-078: known empty after a clean `diff-index` on the warm path; taken lazily (pre-commit's
/// position, before the first hook) after a slow resolution/install (an IDE autosave during a long install counts
/// as pre-existing), and always with `SNP_VERIFY_BASELINE=1`.
fn baseline(
    cx: &RunCtx,
    stash: Option<&StashGuard>,
    git: &Git,
    env: &Env,
    t_resolve: std::time::Instant,
) -> Option<DiffFingerprint> {
    let warm = cx.clock.now().saturating_duration_since(t_resolve) < Duration::from_millis(250);
    let clean = stash.is_some_and(|g| g.diff_index_clean() && *g.stashed() == Stashed::Nothing);
    if !(warm && clean) {
        return None;
    }
    if env.is_truthy_flag("SNP_VERIFY_BASELINE") {
        let real = git.diff_fingerprint().ok()?;
        if real != DiffFingerprint::EMPTY {
            cx.printer.diag(messages::BASELINE_MISMATCH);
        }
        return Some(real);
    }
    Some(DiffFingerprint::EMPTY)
}

/// `output:` (root), `SNP_OUTPUT` overrides, `--summary` (snp brand) forces the footer; strict mode: default.
fn output_mode(cfg: &Config, env: &Env, a: &RunArgs, strict: bool) -> OutputMode {
    if strict {
        return OutputMode::Default;
    }
    if a.ext.summary {
        return OutputMode::Summary;
    }
    match env.get("SNP_OUTPUT").and_then(OsStr::to_str) {
        Some("summary") => OutputMode::Summary,
        Some("quiet") => OutputMode::Quiet,
        Some("default") => OutputMode::Default,
        _ => cfg.root_ext.output,
    }
}

/// D-DIAG-03 (11 §A.14): for each default hook type whose script is pre-commit's, prek's or snp 1.x's, one stderr
/// hint — only on a terminal, without `SNP_NO_HINTS`, without `core.hooksPath`.
fn run_hint(cx: &RunCtx, git: &Git, info: &RepoInfo, cfg: &Config) {
    if cx.env.get_nonempty("SNP_NO_HINTS").is_some() || !std::io::stderr().is_terminal() {
        return;
    }
    if git.hooks_path().ok().flatten().is_some() {
        return;
    }
    for t in &cfg.default_install_hook_types {
        let path = info.common_dir.join("hooks").join(t.as_str());
        let Ok(owner) = snp_git::hookscripts::classify(&path) else {
            continue;
        };
        if !matches!(
            owner,
            snp_git::hookscripts::Owner::PreCommit { .. }
                | snp_git::hookscripts::Owner::Prek
                | snp_git::hookscripts::Owner::SnpV1
        ) {
            continue;
        }
        // 11 §A.16.1: "In M4 the owner is rendered as `Python pre-commit` for `PreCommit`."
        let name = match owner {
            snp_git::hookscripts::Owner::PreCommit { .. } => "Python pre-commit",
            _ => match owner.owner_name() {
                Some(n) => n,
                None => continue,
            },
        };
        let shown = path.strip_prefix(&info.root).unwrap_or(&path);
        cx.printer.diag(&messages::run_hint(
            cx.brand,
            &shown.to_string_lossy(),
            name,
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use snp_core::clock::SystemClock;
    use snp_core::output::{Printer, PrinterOpts};
    use snp_git::GitExe;

    use super::*;

    /// A field whose `Drop` panics.
    #[derive(Debug)]
    pub(super) struct PanicOnDrop;

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("field drop panics");
        }
    }

    fn git(root: &Path, env: &Env, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(root)
            .env_clear()
            .envs(env.iter())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// ADR-066: `RunState::drop` runs before its fields are dropped, so a panic in a field's `Drop` (after the hooks)
    /// cannot prevent the restore.
    #[test]
    fn run_state_restores_before_a_panicking_field_drops() {
        let tmp = tempfile::tempdir().unwrap();
        let base = snp_pycompat::ospath::realpath(tmp.path().as_os_str(), Path::new("/")).unwrap();
        let root = base.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let env = Env::from_pairs([
            ("PATH", "/usr/local/bin:/usr/bin:/bin"),
            ("HOME", base.to_str().unwrap()),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "a"),
            ("GIT_AUTHOR_EMAIL", "a@b"),
            ("GIT_COMMITTER_NAME", "a"),
            ("GIT_COMMITTER_EMAIL", "a@b"),
        ]);
        git(&root, &env, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), b"a\n").unwrap();
        git(&root, &env, &["add", "a.txt"]);
        git(&root, &env, &["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("a.txt"), b"a\nstaged\n").unwrap();
        git(&root, &env, &["add", "a.txt"]);
        std::fs::write(root.join("a.txt"), b"a\nstaged\nunstaged\n").unwrap();
        let cancel = snp_core::cancel::CancelToken::new();
        let exe = Arc::new(GitExe::resolve(&env, &root, None).unwrap());
        let backend = Arc::new(ProcBackend {
            cancel: cancel.clone(),
            role: Role::Tool,
        });
        let (g, info) = Git::discover(exe, &env, &root, backend).unwrap();
        let printer = Arc::new(Printer::new(
            Box::new(std::io::sink()),
            Box::new(std::io::sink()),
            PrinterOpts {
                brand: snp_core::brand::Brand::PreCommit,
                color: false,
                stdout_is_tty: false,
                windows_text_mode: false,
                cancel: cancel.clone(),
            },
        ));
        let critical = Arc::new(ProcBackend {
            cancel: snp_core::cancel::CancelToken::new(),
            role: Role::Critical,
        });
        let guard = snp_stash::enter(StashCtx {
            git: g.with_backend(critical),
            info,
            patches_dir: base.join("patches"),
            printer,
            cancel,
            clock: Arc::new(SystemClock),
            signals: None,
            lock_timeout: Duration::from_secs(5),
            verify: false,
            old_git: false,
        })
        .unwrap();
        // the stash took the unstaged line away; a "hook" modified the file
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "a\nstaged\n"
        );
        let st = RunState {
            stash: Some(guard),
            store_guard: None,
            probe: Some(PanicOnDrop),
        };
        let r = catch_unwind(AssertUnwindSafe(move || drop(st)));
        assert!(r.is_err(), "the probe field panicked");
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "a\nstaged\nunstaged\n",
            "restored before the panicking field was dropped"
        );
    }
}
