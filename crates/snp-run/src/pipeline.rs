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
use std::sync::Arc;
use std::time::Duration;

use snp_cli::RunArgs;
use snp_config::ext::OutputMode;
use snp_config::{Config, RepoConfig};
use snp_core::cancel::CancelReason;
use snp_core::env::{Env, EnvPatch};
use snp_core::failure::{Failure, PyExc};
use snp_core::output::Level;
use snp_core::stage::{HookType, Stage};
use snp_files::{Classifier, FileTable};
use snp_git::{
    ConfigListing, DiffFingerprint, Git, GitVersion, IndexEntries, ProcBackend, RepoInfo,
};
use snp_proc::spawn::Role;
use snp_pyre::Options;
use snp_repo::ResolvedHook;
use snp_stash::diff_oracle::{DiffOracle, Elision};
use snp_stash::{StashCtx, StashGuard, Stashed};
use snp_store::{Store, StoreGuard};

use crate::common::{self, Buffered, ResolveParts};
use crate::exec::ExecEnv;
use crate::files::{self, Selection};
use crate::select::{Conditions, GroupFilter, Skips};
use crate::{RunCtx, messages, meta, render, runenv, sched, trust};

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
    /// The repository the caller already discovered with this `RunCtx` (hook-impl): step 3 reuses it instead of
    /// spawning the same `rev-parse` again (B9, ADR-084).
    pub(crate) discovered: Option<(Git, RepoInfo)>,
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

/// `run.py:run` from `_adjust_args_and_chdir` on, step by step (ARCHITECTURE §3.1). The step functions borrow the
/// stash (never own it), so every exit still goes through [`run_pipeline`]'s explicit restore.
fn body(st: &mut RunState, cx: &RunCtx, a: &RunArgs, inv: &Invocation<'_>) -> Result<i32, Failure> {
    let printer = &*cx.printer;
    // §3.1 steps 3–4: discovery, with the config read speculatively at the invocation cwd meanwhile (ADR-104), then
    // `_adjust_args_and_chdir` as a pure function
    let early_buffered = Buffered::default();
    let (found, preloaded) = discover(cx, a, inv, &early_buffered);
    let (git0, info) = found?;
    let root = info.root.clone();
    let rebase = |f: &OsString| {
        if inv.fixed_paths {
            f.clone()
        } else {
            common::rebase_path(f, &cx.invocation_cwd, &root)
        }
    };
    let config = if inv.fixed_paths {
        a.config.path.clone()
    } else {
        common::resolve_config(cx, &a.config, &root, true)?
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
    // the speculative load stands only when it read the very file the real resolution chose, from the real root;
    // step 6's queries asked beside discovery stand only when, moreover, they were asked for the real layout
    // (ADR-154): the same commands, cwd and environment step 6 would use now
    let at_root = info.root == cx.invocation_cwd;
    let (preloaded, asked) = match preloaded.filter(|sp| at_root && sp.config == config) {
        Some(sp) if sp.layout.as_ref().is_some_and(|l| same_layout(l, &info)) => {
            (None, Some(sp.six))
        }
        Some(sp) => (Some(sp.six.loaded), None),
        None => (None, None),
    };
    // step 5: config tracking (main store, best effort) and the next-run recovery notice
    if inv.track_config {
        common::mark_config_used(&cx.store, &src.path, &root);
    }
    let stash = !a.all_files && cli_files.is_empty();
    // Every non-nested run reports orphaned stashes here, before any preflight can end it (ARCHITECTURE §4.6,
    // ADR-083). When the lock probe fails (another run holds it), a stash reports once it holds the lock instead:
    // each notice is printed exactly once per run on every path.
    let recovery = cx
        .env
        .get("_SNP_STASH_LOCK")
        .is_none()
        .then(|| snp_stash::recovery_report(&git0, &info.git_dir, &cx.store.patches_dir(), &root))
        .flatten();
    let recovery_reported = recovery.is_some();
    for line in recovery.unwrap_or_default() {
        printer.diag(&line);
    }
    // step 6: the read-only preflight queries beside the config load and the DiffOracle inputs
    let buffered = Buffered::default();
    let have_config = preloaded.is_some() || asked.is_some();
    let step6 = Step6 {
        cx,
        a,
        git: &git0,
        info: &info,
        src: &src,
        config: &config,
        stash,
        buffered: &buffered,
    };
    let six = match asked {
        Some(six) => six,
        None => step6.run(preloaded),
    };
    if inv.direct
        && let Ok(cfg) = &six.loaded
    {
        run_hint(cx, &git0, &info, cfg);
    }
    // step 7: preflight in pre-commit's order
    let from = a.from_ref.as_ref().filter(|r| !r.is_empty());
    let to = a.to_ref.as_ref().filter(|r| !r.is_empty());
    let pre = Preflight {
        unmerged: six.unmerged,
        refs_paired: from.is_some() == to.is_some(),
        unstaged: six.unstaged,
        config: &config,
        commit_msg_filename: commit_msg_filename.as_ref(),
    };
    if let Some(code) = pre.check(cx, a)? {
        return Ok(code);
    }
    // step 8: run_env
    let mut run_env = runenv::run_env(&cx.env, a);
    // step 9: the stash guard; ADR-133: on the stat-capable path with git >= 2.42, `git var GIT_ATTR_SYSTEM` (for
    // ADR-119's proof after a checkout) runs overlapped with the stash's checkout, and only when one starts
    let mut system_attributes = None;
    if stash {
        let ask = cx.git_exe.caps.attr_system && six.stamp.is_some();
        let (entered, system) =
            enter_overlapping_system_query(ask, &git0, &info, &run_env, |notice| {
                let stash_cx = StashCtx {
                    // step 6's empty intent-to-add answer (ADR-154)
                    intent_to_add_hint: six.ita_empty,
                    // step 6's G, for the in-process write-tree and diff-index (ADR-153)
                    config_listing: six.early.origins.clone(),
                    ..stash_ctx(
                        cx,
                        inv.store,
                        &git0,
                        &info,
                        &run_env,
                        !recovery_reported,
                        notice,
                    )
                };
                snp_stash::enter(stash_cx)
            });
        let guard = entered?;
        system_attributes = system;
        if let Some((k, v)) = guard.child_env_token() {
            run_env = run_env.patched(&EnvPatch::default().set(k, v));
        }
        st.stash = Some(guard);
    }
    // F from step 6 describes the index only if nobody rewrote it meanwhile, such as another run's `git add` while
    // we waited for the worktree lock (C2). The stamp also changes when the stash itself rewrote the index (its
    // checkout of unstaged files, a `write-tree` that refreshes the cache tree); then F is asked again at step 14
    // (fail closed). G does not read the index: it is kept unless we waited for the lock, when another run's hooks
    // may have changed the config files (ADR-117).
    let fresh = six.stamp.is_some() && git0.index_stamp(&info) == six.stamp;
    let (ita_cleared, contended) = st.stash.as_ref().map_or((false, false), |g| {
        (g.cleared_intent_to_add(), g.lock_contended())
    });
    // F's size decides the one-hook Stat mode also when F itself is asked again (a cost hint, never an input)
    let small = six.early.tracked.as_deref().is_some_and(small_tree);
    let early = if fresh {
        Early {
            tracked: six.early.tracked.filter(|_| !ita_cleared),
            origins: six.early.origins,
            small,
        }
    } else {
        Early {
            tracked: None,
            origins: six.early.origins.filter(|_| !contended),
            small,
        }
    };
    let git = git0.with_env(run_env.clone());
    // step 10: the config (warnings at pre-commit's position), the overlay
    if have_config {
        early_buffered.replay(printer);
    } else {
        buffered.replay(printer);
    }
    let mut cfg = six.loaded?;
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
    let work_before = (cx.resolver.store_work(), parts.downloads());
    let all = cx.resolver.all_hooks(&rcx)?;
    // steps 12–13: selection; SKIP, SNP_SKIP_GROUPS, skip/only conditions
    let strict = common::ext_mode(&cx.env) == snp_config::ExtMode::Strict;
    let conds = Conditions::new(&git, &info, &run_env, &cx.cancel, &cx.regexes);
    let picked = match select(cx, a, &all, &cfg, &run_env, &conds, strict) {
        Ok(p) => p,
        Err(code) => return Ok(code),
    };
    // step 14 (and the side fan-out: staged list and the DiffOracle inputs, read-only, beside the installs)
    let sel = Selection {
        stage: a.hook_stage,
        commit_msg_filename: commit_msg_filename.as_ref(),
        refs: from.zip(to),
        files: &cli_files,
        all_files: a.all_files,
    };
    let needs_files = picked
        .hooks
        .iter()
        .any(|h| !h.def.always_run || h.def.pass_filenames);
    let side = install_beside_side_queries(
        cx,
        &rcx,
        &git,
        &info,
        &run_env,
        &sel,
        &picked,
        &early,
        needs_files,
    )?;
    // step 15: cols, the file table, the classifier (on a clean `--all-files` run from trusted index modes, with the
    // before-diff taken here: ADR-106)
    let names: Vec<&str> = picked.hooks.iter().map(|h| &*h.def.name).collect();
    let cols = render::cols(&names);
    let fifteen = classifier(
        cx,
        &cfg,
        &sel,
        &git,
        &info,
        side.staged,
        needs_files,
        &picked,
    )?;
    let classifier = fifteen.classifier;
    // step 16: the baseline digest (ADR-078, or step 15's before-diff); a resolver that does not count its work never
    // looks quiet
    let quiet =
        work_before.0.is_some() && (cx.resolver.store_work(), parts.downloads()) == work_before;
    let warm =
        quiet && cx.clock.now().saturating_duration_since(t_resolve) < Duration::from_millis(250);
    let baseline = match fifteen.baseline {
        Some(fp) => Some(fp),
        None => baseline(cx, st.stash.as_ref(), &git, &run_env, warm),
    };
    let files = side.origins.as_ref().map(|l| Arc::clone(&l.files));
    let mut oracle = DiffOracle::new(&git, &info, &run_env, baseline, side.tracked, files)?;
    if let Some(listing) = side.origins.as_deref() {
        oracle.enable_index_proof(
            &info,
            &run_env,
            Arc::clone(&cx.pool),
            listing,
            cx.env.is_truthy_flag("SNP_VERIFY_INDEX_PROOF"),
        );
    }
    if baseline.is_none() && warm {
        checkout_baseline(
            cx,
            st.stash.as_ref(),
            side.origins.as_deref(),
            system_attributes.as_ref(),
            &mut oracle,
        );
    }
    if let Some(flag) = fifteen.trust {
        oracle.guard_index_trust(flag);
    }
    // steps 17–20
    let lp = ExecLoop {
        cx,
        a,
        inv,
        git: &git,
        run_env: &run_env,
        rcx: &rcx,
        cfg: &cfg,
        picked: &picked,
        cols,
        classifier: classifier.as_ref(),
        strict,
        refs: sel.refs.is_some(),
    };
    let code = lp.run(st.stash.as_mut(), &mut oracle);
    // ADR-152: the oracle's evidence for the restore by rename, after the last hook, step 19's output and step 20's
    // staging (the guard refuses it after a `stage_fixed` staging)
    if code.is_ok()
        && let Some(g) = st.stash.as_mut()
        && let Some(proof) = g.checked_out().and_then(|p| oracle.restore_proof(p))
    {
        g.prove_restore(proof);
    }
    if oracle.proof_stats().is_some_and(|p| p.mismatches > 0) {
        cx.printer.diag(messages::INDEX_PROOF_MISMATCH);
    }
    code
}

/// §3.1 step 14: `install_envs`, with the side queries (the staged list E, and F/G unless step 6 has them) on a
/// thread beside it.
#[expect(
    clippy::too_many_arguments,
    reason = "the run's borrowed state, one argument per piece"
)]
fn install_beside_side_queries(
    cx: &RunCtx,
    rcx: &snp_repo::ResolveCtx<'_>,
    git: &Git,
    info: &RepoInfo,
    run_env: &Env,
    sel: &Selection<'_>,
    picked: &Picked<'_>,
    early: &Early,
    needs_files: bool,
) -> Result<Side, Failure> {
    let process_hooks = picked
        .to_install
        .iter()
        .filter(|h| matches!(h.exec, snp_repo::ExecKind::Language))
        .count();
    // F and G cost two spawns and save one `git diff` per executed process hook: from three hooks on, or from one
    // when step 6 already has them and the tree is small enough that the stat pass beats the diff (ADR-104), or when
    // step 6 had F on a small tree and the stash's own index rewrite dropped it (ADR-117: F, and G when it was
    // dropped too, are asked again beside the installs)
    let stat = match (&early.tracked, &early.origins) {
        (Some(tracked), Some(_)) => {
            process_hooks >= 3 || (process_hooks >= 1 && small_tree(tracked))
        }
        _ => {
            (process_hooks >= 3 || (process_hooks >= 1 && early.small))
                && stat_possible(git, info, run_env)
        }
    };
    // ADR-134: without the Stat mode, G alone arms the index-stat proof for the process hooks' `git diff`s
    let proof = !stat && process_hooks >= 1 && index_proof_possible(info, run_env);
    let wants_staged = needs_files && files::wants_staged(sel, git, info);
    let have_early = early.tracked.is_some() && early.origins.is_some();
    let side_work = wants_staged || (stat && !have_early) || (proof && early.origins.is_none());
    let (installed, side) = std::thread::scope(|s| {
        let queries = || side_queries(git, wants_staged, stat, proof, early);
        // no thread when the side queries have nothing to ask; inline when it cannot start
        let side = side_work.then(|| common::fork(s, queries));
        let installed = cx.resolver.install_envs(&picked.to_install, rcx);
        let side = match side {
            Some(f) => f.join(|p| Err(panicked(&p))),
            None => queries(),
        };
        (installed, side)
    });
    installed?;
    side
}

/// Step 9 with ADR-133's overlap: when `ask`, a thread waits for the stash's checkout notice and only then, when the
/// worktree and the git directory are on filesystems the Stat mode accepts, asks `git var GIT_ATTR_SYSTEM` with the
/// executable and environment of the parity `git diff` (`git0` with `run_env`; the worktree-lock token added after the
/// stash does not reach attribute lookup). The query overlaps the checkout, and a run that checks nothing out spawns
/// nothing (the stash drops the notice unsent). Per run, never kept. A thread that cannot start, a panic or a failed
/// query is no answer: ADR-119's proof then falls back as without `git var`.
fn enter_overlapping_system_query<T>(
    ask: bool,
    git0: &Git,
    info: &RepoInfo,
    run_env: &Env,
    enter: impl FnOnce(Option<std::sync::mpsc::SyncSender<()>>) -> T,
) -> (T, Option<snp_git::SystemAttributes>) {
    if !ask {
        return (enter(None), None);
    }
    let git = git0.with_env(run_env.clone());
    let (tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
    std::thread::scope(|s| {
        let git = &git;
        let safe = |p: &std::path::Path| snp_os::fsinfo::stat_oracle_safe(p).unwrap_or(false);
        let waiter = std::thread::Builder::new().spawn_scoped(s, move || {
            rx.recv()
                .ok()
                .filter(|()| safe(&info.root) && safe(&info.git_dir))
                .and_then(|()| git.system_attributes())
        });
        // no waiter: no notice either, so a checkout is not reported to a receiver that is gone
        let notice = waiter.is_ok().then_some(tx);
        let entered = enter(notice);
        let system = waiter.ok().and_then(|h| h.join().ok().flatten());
        (entered, system)
    })
}

/// §3.1 step 15's results.
#[derive(Debug, Default)]
struct Fifteen {
    classifier: Option<Classifier>,
    /// The before-diff a trust-capable `--all-files` run took here (ADR-106).
    baseline: Option<DiffFingerprint>,
    /// The flag of the table's trusted index modes, for the diff oracle to guard (ADR-106).
    trust: Option<Arc<std::sync::atomic::AtomicBool>>,
}

/// §3.1 step 15: the file table and the classifier when some hook reads files or refs are given; otherwise only the
/// merge-conflict notice (P17), printed exactly when pre-commit would. A `--all-files` run that can trust index
/// modes builds its table from `ls-files -s -v -f -z` and takes its before-diff here (ADR-106).
#[expect(
    clippy::too_many_arguments,
    reason = "the run's borrowed state, one argument per piece"
)]
fn classifier(
    cx: &RunCtx,
    cfg: &Config,
    sel: &Selection<'_>,
    git: &Git,
    info: &RepoInfo,
    staged: Option<snp_git::ZList>,
    needs_files: bool,
    picked: &Picked<'_>,
) -> Result<Fifteen, Failure> {
    if needs_files || sel.refs.is_some() {
        let early = index_trust(sel, picked, git);
        let (table, baseline, trust) = match early {
            Some(trust::AllFiles {
                table: Some(table),
                baseline,
                trust,
            }) => (table, baseline, trust),
            // the tagged listing failed: `ls-files -z --deduplicate` at the same position, its error pre-commit's
            other => (
                files::table(sel, git, info, &cx.printer, staged)?,
                other.and_then(|a| a.baseline),
                None,
            ),
        };
        let include = regex(cx, &cfg.files)?;
        let exclude = regex(cx, &cfg.exclude)?;
        return Ok(Fifteen {
            classifier: Some(Classifier::new(
                Arc::new(table),
                &include,
                &exclude,
                &cx.pool,
            )?),
            baseline,
            trust,
        });
    }
    if files::merge_branch(sel, git, info) {
        cx.printer.log(Level::Info, messages::MERGE_CONFLICT_ONLY);
    }
    Ok(Fifteen::default())
}

/// ADR-106: the trust-capable step 15 (`trust::all_files`) for a `--all-files` run in which some selected hook can
/// execute (otherwise no before-diff is taken at all, as today) and no selected hook's filter references
/// `executable` or `non-executable` (skipped hooks count: they are classified too, in pre-commit's order). `None`
/// keeps today's path: `ls-files -z --deduplicate`, the lstat pass, the lazy before-diff.
#[expect(
    clippy::redundant_closure_for_method_calls,
    reason = "the tag-set type cannot be named here: snp-run does not depend on file-identify"
)]
fn index_trust(sel: &Selection<'_>, picked: &Picked<'_>, git: &Git) -> Option<trust::AllFiles> {
    if !trust::all_files_branch(sel) || picked.to_install.is_empty() {
        return None;
    }
    let referenced = picked
        .hooks
        .iter()
        .map(|h| h.def.types.union(h.def.types_or).union(h.def.exclude_types))
        .reduce(|a, b| a.union(b))?;
    if !FileTable::index_trust_possible(referenced) {
        return None;
    }
    Some(trust::all_files(git, |table, modes, flag| {
        table.trust_index_modes(modes, flag, referenced)
    }))
}

/// §3.1 step 3: discovery (or the caller's), with the config discovered and loaded at the invocation cwd on a thread
/// meanwhile (`run` only; ADR-104), and, under [`speculative_layout`]'s gates, step 6 asked by that thread for the
/// cwd's layout (ADR-154). The thread's result is `None` when it could not start or panicked: step 6 then loads and
/// asks as before.
fn discover(
    cx: &RunCtx,
    a: &RunArgs,
    inv: &Invocation<'_>,
    buffered: &Buffered,
) -> (Result<(Git, RepoInfo), Failure>, Option<Speculated>) {
    std::thread::scope(|s| {
        let spec = (inv.discovered.is_none() && !inv.fixed_paths)
            .then(|| common::fork(s, || speculative_config(cx, a, buffered)));
        let found = match &inv.discovered {
            Some((git, info)) => Ok((git.clone(), info.clone())),
            None => common::discover(cx),
        };
        let spec = spec.and_then(|f| f.join_or_run(|| None, |_| None));
        (found, spec)
    })
}

/// What ran beside step 3's `rev-parse`: the config path resolved at the invocation cwd and its load (in `six`), and,
/// when the gates of [`speculative_layout`] held, step 6 itself for the layout a repository rooted at the cwd has.
struct Speculated {
    config: OsString,
    /// The layout step 6 was asked for (`None`: only the config was loaded; `six` holds just the load).
    layout: Option<RepoInfo>,
    six: Six,
}

/// §3.1 step 6's inputs.
struct Step6<'s> {
    cx: &'s RunCtx,
    a: &'s RunArgs,
    git: &'s Git,
    info: &'s RepoInfo,
    src: &'s snp_config::ConfigSource,
    config: &'s OsStr,
    stash: bool,
    /// The step-6 load's warnings (unused when step 3's load is kept).
    buffered: &'s Buffered,
}

/// §3.1 step 6's results.
struct Six {
    loaded: Result<Config, Failure>,
    /// Query A (`None`: not a stashing run).
    unmerged: Option<Result<bool, Failure>>,
    /// Query B (`None`: not a stashing run).
    unstaged: Option<Result<bool, Failure>>,
    early: Early,
    /// The index stamp taken before F (ADR-104).
    stamp: Option<snp_git::IndexStamp>,
    /// ADR-154: the index stamp under which query C, asked in this fan-out, succeeded with nothing printed on an
    /// index proved free of intent-to-add entries (the stash's `intent_to_add_hint`).
    ita_empty: Option<snp_git::IndexStamp>,
}

impl Six {
    /// A config load without step 6's queries.
    fn loaded(loaded: Result<Config, Failure>) -> Self {
        Self {
            loaded,
            unmerged: None,
            unstaged: None,
            early: Early::default(),
            stamp: None,
            ita_empty: None,
        }
    }
}

impl Step6<'_> {
    /// The speculative config load (warnings buffered) beside the read-only preflight git calls, and — for a stashing
    /// run on the stat-capable path — the `DiffOracle` inputs F and G, which the stash does not change (only its
    /// intent-to-add clearing changes the index: then F is asked again after it). Started here they are off the
    /// critical path instead of a serial git stage after the stash (B2/B3, ADR-084). F and G start first when they
    /// will be used (the config is already loaded, or the tree is small), and F then also answers query A (ADR-104).
    fn run(&self, preloaded: Option<Result<Config, Failure>>) -> Six {
        let (cx, a, git, stash) = (self.cx, self.a, self.git, self.stash);
        let entries = stat_entries(git, self.info, &cx.env).filter(|_| stash);
        let speculate = entries.is_some();
        let candidates = preloaded.as_ref().map(|loaded| {
            loaded
                .as_ref()
                .map_or(0, |c| process_candidates(c, &a.hooks))
        });
        let hoist = entries.is_some_and(|n| hoist_early(self.info, n, candidates));
        // taken before F and C start: compared after the stash, it shows a rewrite of the index while we waited
        let stamp0 = if stash {
            git.index_stamp(self.info)
        } else {
            None
        };
        let stamp = stamp0.filter(|_| speculate);
        // ADR-154: query C with the stash's command and environment (the run environment is a pure function of the
        // startup environment and the arguments), only on an index proved free of intent-to-add entries
        let ita_git = stamp0.map(|_| git.with_env(runenv::run_env(&cx.env, a)));
        let info = self.info;
        let load_now =
            || common::load_config_with(cx, self.src, &self.buffered.warn, &self.buffered.diag);
        std::thread::scope(|s| {
            // first: its in-process proof comes before its spawn
            let ita = ita_git.as_ref().map(|g| {
                common::fork(s, move || {
                    g.index_without_intent_to_add(info)
                        && g.intent_to_add().is_ok_and(|l| l.is_empty())
                })
            });
            let f = hoist.then(|| common::fork(s, || git.tracked_with_stages().ok()));
            let g = hoist.then(|| common::fork(s, || git.config_listing().ok().map(Arc::new)));
            let load = preloaded.is_none().then(|| {
                common::fork(s, move || {
                    let loaded = load_now();
                    // Without the hoist, only for a config with three or more process-hook candidates: below
                    // that the stat path does not pay for its two spawns (B2's spawn budget). A thread that could
                    // not start, panicked or failed leaves its input to step 14.
                    let early = if speculate
                        && !hoist
                        && loaded
                            .as_ref()
                            .is_ok_and(|c| process_candidates(c, &a.hooks) >= 3)
                    {
                        let f = common::fork(s, || git.tracked_with_modes().ok().map(Arc::new));
                        let origins = git.config_listing().ok().map(Arc::new);
                        Early {
                            tracked: f.join_or_run(|| None, |_| None),
                            origins,
                            small: false,
                        }
                    } else {
                        Early::default()
                    };
                    (loaded, early)
                })
            });
            let unstaged = common::fork(s, || stash.then(|| git.config_is_unstaged(self.config)));
            // query A: from F when F ran and parsed; on any failure of F (spawn, exit status, parse, panic), `git
            // ls-files --unmerged` at the same position, so a broken index reports pre-commit's own error
            let (unmerged, tracked) = match f.map(|f| f.join(|_| None)) {
                Some(Some((entries, unmerged))) => (Some(Ok(unmerged)), Some(Arc::new(entries))),
                Some(None) => (Some(git.has_unmerged()), None),
                None => (stash.then(|| git.has_unmerged()), None),
            };
            let origins = g.and_then(|g| g.join(|_| None));
            let unstaged = unstaged.join(|p| Some(Err(panicked(&p))));
            // a panic, or a failed or non-empty answer, leaves C to the stash at pre-commit's position
            let ita_empty = stamp0.filter(|_| ita.is_some_and(|c| c.join(|_| false)));
            let (loaded, early) = match (preloaded, load) {
                (Some(loaded), _) => (loaded, Early::default()),
                // a thread that could not start loads here, without speculating F and G
                (None, Some(l)) => l.join_or_run(
                    || (load_now(), Early::default()),
                    |p| (Err(panicked(&p)), Early::default()),
                ),
                (None, None) => (load_now(), Early::default()),
            };
            let early = if hoist {
                Early {
                    tracked,
                    origins,
                    small: false,
                }
            } else {
                early
            };
            Six {
                loaded,
                unmerged,
                unstaged,
                early,
                stamp,
                ita_empty,
            }
        })
    }
}

/// §3.1 step 7's inputs.
struct Preflight<'p> {
    unmerged: Option<Result<bool, Failure>>,
    /// `--from-ref` and `--to-ref` are both given or both absent.
    refs_paired: bool,
    unstaged: Option<Result<bool, Failure>>,
    config: &'p OsStr,
    commit_msg_filename: Option<&'p OsString>,
}

impl Preflight<'_> {
    /// The preflight checks in pre-commit's order; `Some(code)` ends the run.
    fn check(self, cx: &RunCtx, a: &RunArgs) -> Result<Option<i32>, Failure> {
        let printer = &*cx.printer;
        if self.unmerged.transpose()?.unwrap_or(false) {
            printer.log(Level::Error, messages::UNMERGED);
            return Ok(Some(1));
        }
        if !self.refs_paired {
            printer.log(Level::Error, messages::BOTH_REFS);
            return Ok(Some(1));
        }
        if self.unstaged.transpose()?.unwrap_or(false) {
            printer.log(
                Level::Error,
                &messages::config_unstaged(&self.config.to_string_lossy()),
            );
            return Ok(Some(1));
        }
        if files::commit_msg_stage(a.hook_stage)
            && self.commit_msg_filename.is_none_or(|f| f.is_empty())
        {
            printer.log(
                Level::Error,
                &messages::commit_msg_required(a.hook_stage.as_str()),
            );
            return Ok(Some(1));
        }
        if a.hook_stage == Stage::Hook(HookType::PostCheckout)
            && cx
                .env
                .get_nonempty("_PRE_COMMIT_SKIP_POST_CHECKOUT")
                .is_some()
        {
            return Ok(Some(0));
        }
        Ok(None)
    }
}

/// The hooks of steps 12–13.
struct Picked<'h> {
    /// Selected by id/alias, stage and group, config order.
    hooks: Vec<&'h ResolvedHook>,
    /// Per selected hook: skipped.
    skipped: Vec<bool>,
    /// The selected hooks not skipped.
    to_install: Vec<&'h ResolvedHook>,
}

/// §3.1 steps 12–13: selection by group, id/alias and stage (`Err(1)` after the message), then `SKIP`,
/// `SNP_SKIP_GROUPS` and the `skip`/`only` conditions.
fn select<'h>(
    cx: &RunCtx,
    a: &RunArgs,
    all: &'h [ResolvedHook],
    cfg: &Config,
    run_env: &Env,
    conds: &Conditions<'_>,
    strict: bool,
) -> Result<Picked<'h>, i32> {
    let printer = &*cx.printer;
    let groups = GroupFilter::new(&a.ext.groups, &a.ext.require_groups, &a.ext.no_groups);
    if let Some((missing, known)) = groups.unknown(all) {
        printer.write_line(messages::no_group(missing, &known).as_bytes());
        return Err(1);
    }
    let selectors: Vec<OsString> = a.hooks.iter().filter(|h| !h.is_empty()).cloned().collect();
    let hooks =
        crate::select::select(all, &selectors, a.hook_stage, &groups).map_err(|missing| {
            printer.write_line(
                messages::no_hook(&missing.to_string_lossy(), a.hook_stage.as_str()).as_bytes(),
            );
            1
        })?;
    let skips = Skips::from_env(run_env, strict);
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
    let to_install = hooks
        .iter()
        .zip(&skipped)
        .filter(|(_, s)| !**s)
        .map(|(h, _)| *h)
        .collect();
    Ok(Picked {
        hooks,
        skipped,
        to_install,
    })
}

/// §3.1 steps 17–20's inputs.
struct ExecLoop<'x> {
    cx: &'x RunCtx,
    a: &'x RunArgs,
    inv: &'x Invocation<'x>,
    git: &'x Git,
    run_env: &'x Env,
    rcx: &'x snp_repo::ResolveCtx<'x>,
    cfg: &'x Config,
    picked: &'x Picked<'x>,
    cols: usize,
    classifier: Option<&'x Classifier>,
    strict: bool,
    /// `--from-ref`/`--to-ref` are set.
    refs: bool,
}

impl ExecLoop<'_> {
    /// Steps 17–20: the hooks, `--show-diff-on-failure` (inside the stash) and the `stage_fixed` staging before the
    /// restore. The stash is only borrowed: the caller's single exit restores it.
    fn run(&self, stash: Option<&mut StashGuard>, oracle: &mut DiffOracle) -> Result<i32, Failure> {
        let (cx, a, git, run_env, cfg) = (self.cx, self.a, self.git, self.run_env, self.cfg);
        let printer = &*cx.printer;
        let facts = snp_git::pch_facts::PchFacts::new(git.clone());
        let color = printer.color();
        let meta_cx = meta::MetaCx {
            cx,
            git,
            env: run_env,
            store: &cx.store,
            rcx: self.rcx,
        };
        let e = ExecEnv {
            cx,
            rcx: self.rcx,
            git,
            run_env,
            color,
            cpu: snp_proc::xargs::cpu_count(),
            max_len: snp_proc::xargs::platform_max_length(cx.env.python_environ_size()),
            facts: &facts,
            stdin: self.inv.hook_stdin.clone(),
            meta: &meta_cx,
        };
        let stage_fixed = !self.strict
            && a.hook_stage == Stage::Hook(HookType::PreCommit)
            && stash.is_some()
            && !self.refs;
        let lp = sched::Loop {
            e: &e,
            hooks: &self.picked.hooks,
            skipped: &self.picked.skipped,
            cols: self.cols,
            verbose: a.verbose,
            fail_fast: cfg.fail_fast || a.fail_fast,
            classifier: self.classifier,
            mode: output_mode(cfg, run_env, a, self.strict),
            fail_on_changes: cfg.root_ext.fail_on_changes,
            ci: run_env.is_truthy_flag("CI"),
            stage_fixed,
            root_ext: &cfg.root_ext,
            no_tty: run_env.is_truthy_flag("SNP_NO_TTY"),
        };
        let res = sched::run_hooks(&lp, oracle)?;
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
            && let Some(g) = stash
            && !g.stage_fixed(&res.to_stage)?
        {
            printer.diag(messages::STAGE_FIXED_REFUSED);
            retval = 1;
        }
        Ok(retval)
    }
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
fn stash_ctx(
    cx: &RunCtx,
    store: &Store,
    git: &Git,
    info: &RepoInfo,
    run_env: &Env,
    report_recovery: bool,
    checkout_notice: Option<std::sync::mpsc::SyncSender<()>>,
) -> StashCtx {
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
        report_recovery,
        checkout_notice,
        intent_to_add_hint: None,
        config_listing: None,
    }
}

/// Whether the `DiffOracle`'s Stat mode can apply (Unix, ≤ 16 384 index entries, no `SNP_DIFF_MODE=git`, no
/// `GIT_ATTR_SOURCE`): only then are fan-out queries F and G taken.
fn stat_possible(git: &Git, info: &RepoInfo, env: &Env) -> bool {
    stat_entries(git, info, env).is_some()
}

/// The index-header entry count when [`stat_possible`] holds.
fn stat_entries(git: &Git, info: &RepoInfo, env: &Env) -> Option<u32> {
    if !cfg!(unix)
        || env.get("SNP_DIFF_MODE").is_some_and(|v| v == "git")
        || env.get("GIT_ATTR_SOURCE").is_some()
    {
        return None;
    }
    git.index_entry_count(info).filter(|n| *n <= 16_384)
}

/// Whether F and G start at the top of step 6, before the config load (ADR-104), given the index entry count `n`
/// and the config's process-hook `candidates` when the config is already loaded:
///
/// * three or more candidates: always (ADR-084(6) takes F and G then anyway, only later);
/// * none: never (only process hooks use the Stat mode);
/// * one or two, or not known yet: on an index of at most [`SMALL_TREE`] entries (F and G are cheap there, and one
///   process hook uses them), with three or more CPUs (the load, B, F and G overlap), and when the worktree and the
///   git directory are on filesystems the Stat mode accepts (one statfs each), so no G is spent on a run whose
///   oracle would refuse it. A larger index waits for the config (measured: a 2 000-file, one-hook run spent 32%
///   more instructions with both queries started early and Git mode kept).
fn hoist_early(info: &RepoInfo, n: u32, candidates: Option<usize>) -> bool {
    let safe = |p: &std::path::Path| snp_os::fsinfo::stat_oracle_safe(p).unwrap_or(false);
    match candidates {
        Some(c) if c >= 3 => true,
        Some(0) => false,
        _ => {
            usize::try_from(n).is_ok_and(|n| n <= SMALL_TREE)
                && snp_proc::xargs::cpu_count().get() >= 3
                && safe(&info.root)
                && safe(&info.git_dir)
        }
    }
}

/// The config as discovery and `_adjust_args_and_chdir` would find it if the repository root were the invocation
/// cwd, loaded with its warnings buffered (ADR-104). It runs beside step 3's `rev-parse`; the caller accepts it only
/// when the real root is the cwd and the real config path is byte-equal, and drops it (with its buffered warnings)
/// otherwise. Discovery is note-free here: the stderr note is printed by the real resolution only.
fn speculative_config(cx: &RunCtx, a: &RunArgs, buffered: &Buffered) -> Option<Speculated> {
    let cwd = &cx.invocation_cwd;
    let d =
        snp_config::discover::discover(a.config.explicit(), cwd, cwd, cx.brand, &cx.env).ok()?;
    let config = common::adjust_config(&d.config, cwd, cwd);
    let src = common::config_source(&config, cwd);
    let loaded = common::load_config_with(cx, &src, &buffered.warn, &buffered.diag);
    let Some((git, layout)) = speculative_layout(cx, a) else {
        return Some(Speculated {
            config,
            layout: None,
            six: Six::loaded(loaded),
        });
    };
    // ADR-154: step 6 as it will run with this config kept, for the cwd's layout (`buffered` is unused: the config
    // is loaded already)
    let step6 = Step6 {
        cx,
        a,
        git: &git,
        info: &layout,
        src: &src,
        config: &config,
        stash: true,
        buffered,
    };
    let six = step6.run(Some(loaded));
    Some(Speculated {
        config,
        layout: Some(layout),
        six,
    })
}

/// ADR-154: the layout of a repository rooted at the invocation cwd, with the handle discovery would build for it,
/// when step 6 may be asked beside `rev-parse`: on Unix (elsewhere the index stamp, and with it everything step 6
/// speculates, is unavailable), for a stashing run (decided by the arguments alone: no `--all-files`, no file
/// arguments, no file list on stdin), with none of `GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR` and
/// `GIT_INDEX_FILE` set, and `<cwd>/.git` a directory (a linked worktree's `.git` file, a symbolic link and a
/// subdirectory invocation do not speculate). The caller keeps the results only when discovery reports this very
/// layout ([`same_layout`]).
fn speculative_layout(cx: &RunCtx, a: &RunArgs) -> Option<(Git, RepoInfo)> {
    let stdin_list = a.ext.stdin_files.is_some() && cx.brand == snp_core::brand::Brand::Snp;
    let located = [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
    ]
    .iter()
    .any(|k| cx.env.get(k).is_some());
    if !cfg!(unix) || a.all_files || !a.files.is_empty() || stdin_list || located {
        return None;
    }
    let root = cx.invocation_cwd.clone();
    let git_dir = root.join(".git");
    if !std::fs::symlink_metadata(&git_dir).is_ok_and(|m| m.is_dir()) {
        return None;
    }
    let info = RepoInfo {
        common_dir: git_dir.clone(),
        index_file: git_dir.join("index"),
        git_dir,
        root,
        is_worktree: false,
    };
    let git = Git::at(
        Arc::clone(&cx.git_exe),
        &cx.env,
        &info,
        Arc::clone(&cx.git_backend),
    )
    .with_brand(cx.brand);
    Some((git, info))
}

/// Two layouts name the same root, git directory, common directory and index file (ADR-154).
fn same_layout(a: &RepoInfo, b: &RepoInfo) -> bool {
    a.root == b.root
        && a.git_dir == b.git_dir
        && a.common_dir == b.common_dir
        && a.index_file == b.index_file
        && a.is_worktree == b.is_worktree
}

/// Tracked entries plus their ancestor directories up to which one or two process hooks use the Stat mode: below it
/// the stat pass (and the two `.gitattributes` stats per directory) costs less than the after-hook `git diff` it
/// replaces; above it Git mode was measured faster for one to three hooks at 500–4 000 files (ADR-104).
const SMALL_TREE: usize = 256;

/// Whether `tracked` has at most [`SMALL_TREE`] entries and distinct ancestor directories together.
fn small_tree(tracked: &IndexEntries) -> bool {
    let entries = tracked.paths.len();
    if entries > SMALL_TREE {
        return false;
    }
    let mut dirs: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
    for path in tracked.paths.iter() {
        let mut rest = path;
        while let Some(slash) = rest.iter().rposition(|&b| b == b'/') {
            rest = rest.get(..slash).unwrap_or_default();
            if !dirs.insert(rest) {
                break;
            }
            if entries.saturating_add(dirs.len()) > SMALL_TREE {
                return false;
            }
        }
    }
    true
}

/// Fan-out queries E (staged list), F (`ls-files -s -v -z`) and G (`config --list --show-origin -z`).
#[derive(Debug, Default)]
struct Side {
    staged: Option<snp_git::ZList>,
    tracked: Option<Arc<IndexEntries>>,
    origins: Option<Arc<ConfigListing>>,
}

/// The hooks of `cfg` that may run as processes (local and remote; meta and builtin hooks are in-process), among
/// the selected ones when the command line names hooks (`run noop-01` of a ten-hook config is one candidate).
fn process_candidates(cfg: &Config, selectors: &[OsString]) -> usize {
    let picked = |id: &str, alias: &str| {
        selectors.is_empty()
            || selectors.iter().any(|s| {
                s.as_os_str() == OsStr::new(id)
                    || (!alias.is_empty() && s.as_os_str() == OsStr::new(alias))
            })
    };
    cfg.repos
        .iter()
        .map(|r| match r {
            RepoConfig::Local { hooks } => hooks.iter().filter(|h| picked(&h.id, &h.alias)).count(),
            RepoConfig::Remote { hooks, .. } => hooks.iter().filter(|h| picked(&h.id, "")).count(),
            _ => 0,
        })
        .sum()
}

/// F and G taken at step 6 (`None`: not speculated, failed, or stale after an index rewrite or an intent-to-add
/// clearing).
#[derive(Debug, Default)]
struct Early {
    tracked: Option<Arc<IndexEntries>>,
    origins: Option<Arc<ConfigListing>>,
    /// Step 6's F described a [`small_tree`], even if it was dropped since (ADR-117).
    small: bool,
}

/// Whether the index-stat proof can arm in this run (ADR-134), checked before G is asked for it: Unix, no
/// `SNP_DIFF_MODE=git`, no fsmonitor test variable, and the worktree and the git directory on filesystems the
/// `DiffOracle` accepts. The oracle checks all of it again with G's configuration.
fn index_proof_possible(info: &RepoInfo, env: &Env) -> bool {
    let safe = |p: &std::path::Path| snp_os::fsinfo::stat_oracle_safe(p).unwrap_or(false);
    cfg!(unix)
        && env.get("SNP_DIFF_MODE").is_none_or(|v| v != "git")
        && ["GIT_TEST_FSMONITOR", "GIT_FSMONITOR_TEST"]
            .into_iter()
            .all(|k| env.get_nonempty(k).is_none())
        && safe(&info.root)
        && safe(&info.git_dir)
}

/// E, and F/G unless step 6 has them; with `proof` (and no Stat mode) G alone, for the index-stat proof (ADR-134).
/// `wants_staged` is decided once by the caller (it stats the merge files).
fn side_queries(
    git: &Git,
    wants_staged: bool,
    stat: bool,
    proof: bool,
    early: &Early,
) -> Result<Side, Failure> {
    let early_pair = match (&early.tracked, &early.origins) {
        (Some(t), Some(o)) => Some((Arc::clone(t), Arc::clone(o))),
        _ => None,
    };
    if !stat && proof {
        // G kept from step 6, or asked beside E (a failure only costs the proof)
        let kept = early.origins.as_ref().map(Arc::clone);
        return std::thread::scope(|s| {
            let g = (kept.is_none() && wants_staged)
                .then(|| common::fork(s, || git.config_listing().ok().map(Arc::new)));
            let staged = wants_staged.then(|| git.staged_files());
            let origins = match g {
                Some(g) => g.join(|_| None),
                None => kept.or_else(|| git.config_listing().ok().map(Arc::new)),
            };
            Ok(Side {
                staged: staged.transpose()?,
                tracked: None,
                origins,
            })
        });
    }
    if !stat || early_pair.is_some() {
        let (tracked, origins) = early_pair.filter(|_| stat).unzip();
        return Ok(Side {
            staged: wants_staged.then(|| git.staged_files()).transpose()?,
            tracked,
            origins,
        });
    }
    // E, F and G are independent read-only queries: each on its own thread, so one spawn is on the critical path; a
    // G kept from step 6 is not asked again (ADR-117)
    let kept = early.origins.as_ref().map(Arc::clone);
    std::thread::scope(|s| {
        let f = common::fork(s, || git.tracked_with_modes().ok().map(Arc::new));
        let g = kept
            .is_none()
            .then(|| common::fork(s, || git.config_listing().ok().map(Arc::new)));
        let staged = wants_staged.then(|| git.staged_files());
        // a failure of F or G only costs the Stat mode (the oracle falls back to `git diff`); a thread that could
        // not start runs its query here
        let tracked = f.join(|_| None);
        let origins = match g {
            Some(g) => g.join(|_| None),
            None => kept,
        };
        Ok(Side {
            staged: staged.transpose()?,
            tracked,
            origins,
        })
    })
}

/// The baseline of ADR-078(3): known empty after a clean `diff-index` on the warm path, taken lazily (pre-commit's
/// position, before the first hook) otherwise, and always with `SNP_VERIFY_BASELINE=1`.
///
/// `warm` is ADR-078(3)'s "no clone, install or download since `diff-index`", decided by the caller: the resolver
/// counted no clone or install and no network request went out (ADR-104), and resolution and installation took
/// under 250 ms, a second, fail-closed guard: a long wait (a contended store lock) is also a window in which an IDE
/// autosave must count as pre-existing. After a stash that checked out files, see [`checkout_baseline`] (ADR-119).
fn baseline(
    cx: &RunCtx,
    stash: Option<&StashGuard>,
    git: &Git,
    env: &Env,
    warm: bool,
) -> Option<DiffFingerprint> {
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

/// ADR-119: on the warm path, after a stash that checked out the patch's files, the baseline is known empty when the
/// oracle's Stat-mode snapshot proves the checkout left `git diff` empty (`DiffOracle::elide_checkout_baseline`);
/// otherwise it stays lazy, as before. Not after an intent-to-add clearing, and only with query G's configuration
/// facts; `system` is `git var GIT_ATTR_SYSTEM`'s answer of this run (ADR-133). `SNP_VERIFY_BASELINE=1` takes a
/// proven baseline with the real `git diff` and reports a non-empty one.
fn checkout_baseline(
    cx: &RunCtx,
    stash: Option<&StashGuard>,
    config: Option<&ConfigListing>,
    system: Option<&snp_git::SystemAttributes>,
    oracle: &mut DiffOracle,
) {
    let Some(g) = stash.filter(|g| !g.cleared_intent_to_add()) else {
        return;
    };
    let (Some(paths), Some(config)) = (g.checked_out(), config) else {
        return;
    };
    let verify = cx.env.is_truthy_flag("SNP_VERIFY_BASELINE");
    if oracle.elide_checkout_baseline(paths, &config.checkout, system, verify) == Elision::Mismatch
    {
        cx.printer.diag(messages::BASELINE_MISMATCH);
    }
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
        let shown = path.trim_prefix(&info.root);
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
        // The process's PATH (git lives elsewhere on Windows) and the variables a Windows git needs to start.
        let process = Env::capture_process();
        let mut pairs: Vec<(String, String)> = [
            "PATH",
            "SYSTEMROOT",
            "WINDIR",
            "PATHEXT",
            "COMSPEC",
            "TEMP",
            "TMP",
        ]
        .into_iter()
        .filter_map(|k| {
            process
                .get(k)
                .map(|v| (k.to_owned(), v.to_string_lossy().into_owned()))
        })
        .collect();
        if pairs.is_empty() {
            pairs.push(("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()));
        }
        for (k, v) in [
            ("HOME", base.to_str().unwrap()),
            ("USERPROFILE", base.to_str().unwrap()),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "a"),
            ("GIT_AUTHOR_EMAIL", "a@b"),
            ("GIT_COMMITTER_NAME", "a"),
            ("GIT_COMMITTER_EMAIL", "a@b"),
        ] {
            pairs.push((k.to_owned(), v.to_owned()));
        }
        let env = Env::from_pairs(pairs);
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
            report_recovery: true,
            checkout_notice: None,
            intent_to_add_hint: None,
            config_listing: None,
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
