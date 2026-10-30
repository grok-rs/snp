//! The stash guard: `staged_files_only` as a journaled state machine (ARCHITECTURE §4.1–§4.5).
//!
//! pre-commit's algorithm (`staged_files_only.py`), byte for byte in the git commands, the user-visible git state and
//! the messages (02 §7, ADR-016), plus snp's hardening (ADR-016, ADR-026, ADR-065, ADR-066, ADR-067): the worktree lock
//! with a nested-run token, a lazily created journal and git-dir pointer written before every mutation, the patch
//! streamed into a `create_new` 0600 file while hashed and made durable before the checkout, a re-verification of the
//! diff right before the checkout, every git command on the caller's `Role::Critical` handle, the restore under a
//! `RestoreShield`, the K+ hint, and the panic-path restore in `Drop`.
//!
//! Fault points (`snp_core::fault`, ADR-066): `StashState(n)` after each transition — 1 locked, 2 journal started,
//! 3 intent-to-add cleared, 4 patch durable (state `stashed` journaled), 5 checkout done, 6 `staging-fixed` journaled,
//! 7 fixes staged, 8 rolling back, 9 restored, 10 done — and `RestoreCommand(n)` before each restore command: 1 apply,
//! 2 apply with `core.autocrlf=false`, 3 rollback checkout, 4 apply, 5 apply with `core.autocrlf=false`, 6 the
//! intent-to-add re-add.
//!
//! Owner: WP-23.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, BufWriter, Seek, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use snp_core::cancel::CancelToken;
use snp_core::clock::Clock;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::fault::{self, Point};
use snp_core::output::{Level, Printer};
use snp_core::path::RepoPath;
use snp_git::{DiffIndex, Git, ObjectId, RepoInfo, ZList};
use snp_proc::signals::SignalGuard;
use snp_proc::spawn::Group;

use crate::journal::{self, JournalData, PointerData, State};
use crate::lock::{self, Acquired, WorktreeLock};
use crate::{messages, recovery};

/// Owns cheap handles (`Git` and `Printer` are `Arc`-backed) so the guard has no lifetime and can live inside
/// `RunState`. `git` must be built with `Git::with_backend(ProcBackend { cancel: CancelToken::new(), role:
/// Role::Critical })`: a token that is never cancelled, so restore commands are never cut short (ADR-065). `cancel`
/// is the run token (observed only by the lock wait). `verify` = `SNP_STASH_VERIFY=1` (warn-only post-restore check,
/// §4.2 step 10e).
#[derive(Debug)]
pub struct StashCtx {
    /// The Critical-role git handle.
    pub git: Git,
    /// The repository layout.
    pub info: RepoInfo,
    /// `<store>/patches`.
    pub patches_dir: PathBuf,
    /// The output sink.
    pub printer: Arc<Printer>,
    /// The run token (lock wait only).
    pub cancel: CancelToken,
    /// Patch timestamps.
    pub clock: Arc<dyn Clock>,
    /// The signal handler (restore shield).
    pub signals: Option<Arc<SignalGuard>>,
    /// `SNP_STASH_LOCK_TIMEOUT`, default 60 s, ≤ 1 h.
    pub lock_timeout: Duration,
    /// `SNP_STASH_VERIFY=1`.
    pub verify: bool,
    /// `caps.version < 2.19`: refuse to stash (D-DATA-17).
    pub old_git: bool,
}

impl StashCtx {
    /// `SNP_STASH_LOCK_TIMEOUT` from the run environment: seconds (`float()` semantics), default 60 s, clamped to
    /// 1 h; an invalid value means the default. A convenience for the callers that build the context (snp-only).
    pub fn lock_timeout_from_env(env: &Env) -> Duration {
        lock::timeout_from(env.get("SNP_STASH_LOCK_TIMEOUT"))
    }
}

/// What the stash saved.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stashed {
    /// No unstaged changes.
    Nothing,
    /// The unstaged changes, saved as this patch.
    Patch(PathBuf),
}

/// The state machine; restores on drop (panic path only, via `RunState::drop`).
///
/// `staged_files_only` (`staged_files_only.py:107-113`) with the hardening of ARCHITECTURE §4: create it with
/// [`enter`], run the hooks, optionally [`StashGuard::stage_fixed`], then call [`StashGuard::restore`] exactly once.
/// Dropping a guard that was not restored restores it inside `catch_unwind` (the panic fallback, ADR-066).
#[derive(Debug)]
pub struct StashGuard {
    stashed: Stashed,
    clean: bool,
    inner: Option<Box<Inner>>,
}

/// The live state of a guard (dropped by `finish`).
#[derive(Debug)]
struct Inner {
    cx: StashCtx,
    /// `None` for a nested run (the outer run holds the lock).
    lock: Option<WorktreeLock>,
    /// The token exported to hooks (`_SNP_STASH_LOCK`).
    token: String,
    /// The intent-to-add paths cleared with `git rm --cached`.
    ita: ZList,
    tree: Option<ObjectId>,
    patch: Option<PathBuf>,
    journal: Option<Journal>,
    staged_fixed: Vec<RepoPath>,
    /// The SIGTTIN/SIGTTOU notice was printed.
    tty_noted: bool,
    /// Which restore steps are due.
    flags: Steps,
}

/// The restore steps still due (each is cleared when done, so a resumed `finish` skips it).
#[derive(Debug, Default, Clone, Copy)]
struct Steps {
    /// `git rm --cached` was attempted: the intent-to-add re-add is due.
    ita_cleared: bool,
    /// The checkout was attempted: the patch must be re-applied.
    stash_active: bool,
    /// The restore failed (K+): the journal is kept.
    restore_failed: bool,
}

/// The journal of this run: the store journal (optional when the store is read-only and only intent-to-add files are
/// cleared) and the git-dir pointer.
#[derive(Debug)]
struct Journal {
    data: JournalData,
    file: Option<PathBuf>,
    pointer: Option<PathBuf>,
}

impl Journal {
    fn pointer_data(&self) -> PointerData {
        PointerData {
            journal: self.file.clone(),
            patch: self.data.patch.clone(),
            tree: self.data.tree.clone(),
            state: self.data.state.clone(),
            worktree: Some(self.data.worktree.clone()),
            staged_fixed: self.data.staged_fixed.clone(),
        }
    }

    /// Rewrites both files with `state` (temp + rename; fsync for the data-protecting states).
    fn update(&mut self, state: State) -> io::Result<()> {
        state.as_str().clone_into(&mut self.data.state);
        let durable = state.durable();
        if let Some(f) = &self.file {
            journal::replace(f, &self.data.to_bytes(), durable)?;
        }
        if let Some(p) = &self.pointer {
            journal::replace(p, &self.pointer_data().to_bytes(), durable)?;
        }
        Ok(())
    }

    /// Done: both files deleted.
    fn remove(&self) {
        for p in [&self.file, &self.pointer].into_iter().flatten() {
            let _ignored = std::fs::remove_file(p);
        }
    }
}

/// How often the re-verification may rewrite the patch before giving up (ARCHITECTURE §4.2 step 6).
const MAX_REWRITES: u8 = 3;

/// `std::process::id()`.
fn pid() -> u32 {
    std::process::id()
}

/// The patch sink: the patch file (buffered) and a blake3 of everything written.
struct PatchSink<'a> {
    file: Option<&'a mut BufWriter<File>>,
    hasher: blake3::Hasher,
}

impl PatchSink<'_> {
    fn hash_only() -> PatchSink<'static> {
        PatchSink {
            file: None,
            hasher: blake3::Hasher::new(),
        }
    }
}

impl Write for PatchSink<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = match &mut self.file {
            Some(f) => f.write(buf)?,
            None => buf.len(),
        };
        self.hasher.update(buf.get(..n).unwrap_or_default());
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

/// The first `diff-index` sink: the patch file (`patches/patch<secs>-<pid>[-<n>]`, `create_new` 0600) is created at
/// the first byte, so a clean run never touches the store; everything written is hashed. A creation or write error is
/// kept (and stops the stream) so the caller can tell a read-only store (X13-ST-04) from a git failure.
struct LazyPatch<'a> {
    inner: &'a Inner,
    opened: Option<(PathBuf, BufWriter<File>)>,
    failed: Option<(io::Error, PathBuf)>,
    hasher: blake3::Hasher,
}

impl Write for LazyPatch<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.failed.is_some() {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        if self.opened.is_none() {
            match self.inner.create_patch_file() {
                Ok((p, f)) => self.opened = Some((p, BufWriter::new(f))),
                Err((e, shown)) => {
                    let kind = e.kind();
                    self.failed = Some((e, shown));
                    return Err(io::Error::from(kind));
                }
            }
        }
        let Some((p, w)) = self.opened.as_mut() else {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        };
        match w.write(buf) {
            Ok(n) => {
                self.hasher.update(buf.get(..n).unwrap_or_default());
                Ok(n)
            }
            Err(e) => {
                let kind = e.kind();
                self.failed = Some((e, p.clone()));
                Err(io::Error::from(kind))
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.opened.as_mut() {
            Some((_, w)) => w.flush(),
            None => Ok(()),
        }
    }
}

/// `staged_files_only` entry: lock, journal, `write-tree`, `diff-index`, checkout (§4.2 steps 1–9).
///
/// `staged_files_only.py:35-104` entered in pre-commit's order (`_intent_to_add_cleared` first,
/// `_unstaged_changes_cleared` second), preceded by the worktree lock and the next-run recovery notice (§4.6), and
/// with the patch durable and re-verified before `git -c submodule.recurse=0 checkout -- .`. An error after the first
/// mutation runs the restore first (pre-commit's `finally` blocks); a restore failure replaces the error, as an
/// exception raised in a Python `finally` does.
pub fn enter(cx: StashCtx) -> Result<StashGuard, Failure> {
    let env_token = cx.git.env().get(lock::TOKEN_ENV).map(OsStr::to_os_string);
    let acquired = lock::acquire(
        &cx.info.git_dir,
        env_token.as_deref(),
        &cx.printer,
        &cx.cancel,
        cx.lock_timeout,
    )?;
    let (lock, token, nested) = match acquired {
        Acquired::Held(l) => {
            let t = l.token().to_owned();
            (Some(l), t, false)
        }
        Acquired::Nested(t) => (None, t, true),
    };
    let mut guard = StashGuard {
        stashed: Stashed::Nothing,
        clean: false,
        inner: Some(Box::new(Inner {
            cx,
            lock,
            token,
            ita: ZList::default(),
            tree: None,
            patch: None,
            journal: None,
            staged_fixed: Vec::new(),
            tty_noted: false,
            flags: Steps::default(),
        })),
    };
    fault::hit(Point::StashState(1));
    if !nested && let Some(inner) = guard.inner.as_deref() {
        inner.recovery_notices();
    }
    let outcome = match guard.inner.as_deref_mut() {
        Some(inner) => inner.stash(),
        None => Ok(StashOutcome::Nothing { clean: false }),
    };
    match outcome {
        Ok(StashOutcome::Nothing { clean }) => {
            guard.clean = clean;
            Ok(guard)
        }
        Ok(StashOutcome::Patch(p)) => {
            guard.stashed = Stashed::Patch(p);
            Ok(guard)
        }
        Err(e) => {
            if let Some(inner) = guard.inner.as_deref()
                && inner.flags.stash_active
                && let Some(p) = &inner.patch
            {
                guard.stashed = Stashed::Patch(p.clone());
            }
            match guard.finish() {
                Ok(()) => Err(e),
                Err(restore) => Err(restore),
            }
        }
    }
}

/// The result of the stash steps.
enum StashOutcome {
    Nothing { clean: bool },
    Patch(PathBuf),
}

impl Inner {
    fn git(&self) -> &Git {
        &self.cx.git
    }

    fn printer(&self) -> &Printer {
        &self.cx.printer
    }

    /// ARCHITECTURE §4.6: orphaned journals of this worktree (lock held) → stderr notices; stale journals and pointers
    /// (restored, or never stashed) are removed.
    fn recovery_notices(&self) {
        let info = &self.cx.info;
        let scan = recovery::scan(&info.git_dir, &self.cx.patches_dir, &info.root);
        for stale in &scan.stale {
            let _ignored = std::fs::remove_file(stale);
        }
        for u in &scan.unrestored {
            self.printer().diag(&u.hint());
        }
    }

    fn new_journal_data(&self) -> JournalData {
        let info = &self.cx.info;
        let head = journal::read_small(&info.git_dir.join("HEAD"))
            .map(|b| String::from_utf8_lossy(&b).trim().to_owned())
            .unwrap_or_default();
        JournalData {
            state: State::Started.as_str().to_owned(),
            token: self.token.clone(),
            worktree: info.root.clone(),
            git_dir: info.git_dir.clone(),
            common_dir: info.common_dir.clone(),
            index_file: info.index_file.clone(),
            head,
            pid: pid(),
            start_time: self.cx.clock.unix_secs(),
            snp_version: snp_core::profile::SNP_VERSION.to_owned(),
            ita: self.ita.iter().map(<[u8]>::to_vec).collect(),
            tree: self.tree.as_ref().map(|t| t.as_str().to_owned()),
            patch: self.patch.clone(),
            staged_fixed: Vec::new(),
        }
    }

    /// `<store>/patches`, created (0700) when missing (`os.makedirs(patch_dir, exist_ok=True)`).
    fn ensure_patches_dir(&self) -> io::Result<()> {
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(&self.cx.patches_dir)
    }

    /// Creates the journal lazily (ARCHITECTURE §4.2 step 2) with `state`: the store journal and the git-dir pointer,
    /// both `create_new`. `allow_no_store`: the intent-to-add-only case, where an unwritable store skips the store
    /// journal with a stderr note.
    fn create_journal(&mut self, state: State, allow_no_store: bool) -> Result<(), Failure> {
        let mut data = self.new_journal_data();
        state.as_str().clone_into(&mut data.state);
        let durable = state.durable();
        let mut file = None;
        let store = self.ensure_patches_dir().and_then(|()| {
            for _ in 0..16 {
                let name = format!(
                    "run-{}-{}{}",
                    pid(),
                    lock::random_hex(6),
                    journal::JOURNAL_SUFFIX
                );
                let path = self.cx.patches_dir.join(name);
                match journal::write_new(&path, &data.to_bytes(), durable) {
                    Ok(()) => return Ok(path),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
            }
            Err(io::Error::from(io::ErrorKind::AlreadyExists))
        });
        match store {
            Ok(p) => file = Some(p),
            Err(e) if allow_no_store => {
                self.printer().diag(&messages::journal_skipped(
                    &self.cx.patches_dir,
                    &snp_core::failure::pyerr::os_error_text(&e, None, None),
                ));
            }
            Err(e) => {
                return Err(Failure::from_io(
                    &e,
                    Some(self.cx.patches_dir.as_os_str()),
                    None,
                ));
            }
        }
        let mut j = Journal {
            data,
            file,
            pointer: None,
        };
        let pointer_bytes = j.pointer_data().to_bytes();
        let git_dir = &self.cx.info.git_dir;
        let mut last = io::Error::from(io::ErrorKind::AlreadyExists);
        for n in 0_u32..100 {
            let name = if n == 0 {
                journal::POINTER_NAME.to_owned()
            } else {
                format!("snp-stash-{n}.journal")
            };
            let p = git_dir.join(name);
            match journal::write_new(&p, &pointer_bytes, durable) {
                Ok(()) => {
                    j.pointer = Some(p);
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
                Err(e) => {
                    last = e;
                    break;
                }
            }
        }
        if j.pointer.is_none() {
            j.remove();
            return Err(Failure::from_io(&last, Some(git_dir.as_os_str()), None));
        }
        self.journal = Some(j);
        Ok(())
    }

    /// Writes `state` (creating the journal when this is the first one).
    fn journal_state(&mut self, state: State) -> Result<(), Failure> {
        match &mut self.journal {
            Some(j) => {
                j.data.tree = self.tree.as_ref().map(|t| t.as_str().to_owned());
                j.data.patch.clone_from(&self.patch);
                j.data.staged_fixed = self
                    .staged_fixed
                    .iter()
                    .map(|r| r.as_bytes().to_vec())
                    .collect();
                j.update(state).map_err(|e| {
                    let shown = j
                        .file
                        .as_ref()
                        .or(j.pointer.as_ref())
                        .cloned()
                        .unwrap_or_default();
                    Failure::from_io(&e, Some(shown.as_os_str()), None)
                })
            }
            None => self.create_journal(state, false),
        }
    }

    /// A best-effort state write after the stash (rollback/failed/restored: the restore must go on).
    fn journal_best_effort(&mut self, state: State) {
        if let Some(j) = &mut self.journal {
            let _ignored = j.update(state);
        }
    }

    /// Steps 3–8.
    fn stash(&mut self) -> Result<StashOutcome, Failure> {
        // 3. intent-to-add (`_intent_to_add_cleared`, + `--no-renames`: hazard b).
        let ita = self.git().intent_to_add()?;
        if !ita.is_empty() {
            self.printer().log(Level::Warning, messages::ITA_DETECTED);
            self.ita = ita;
            self.create_journal(State::Started, true)?;
            fault::hit(Point::StashState(2));
            self.flags.ita_cleared = true;
            self.git().rm_cached(&self.ita, Group::Isolated)?;
            fault::hit(Point::StashState(3));
        }
        // 4. tree (checked → CalledProcessError, exit 3).
        let tree = self.git().write_tree()?;
        self.tree = Some(tree.clone());
        // 5. diff-index.
        if self.cx.old_git {
            let mut sink = PatchSink::hash_only();
            return match self.git().diff_index_patch(&tree, &mut sink)? {
                DiffIndex::Patch { .. } => Err(self.old_git_refusal()),
                DiffIndex::Clean => Ok(StashOutcome::Nothing { clean: true }),
                _ => Ok(StashOutcome::Nothing { clean: false }),
            };
        }
        let (first, opened, failed, digest) = {
            let mut sink = LazyPatch {
                inner: &*self,
                opened: None,
                failed: None,
                hasher: blake3::Hasher::new(),
            };
            let res = self.git().diff_index_patch(&tree, &mut sink);
            let flushed = sink.opened.as_mut().map_or(Ok(()), |(_, w)| w.flush());
            if let (Err(e), Some((p, _))) = (flushed, &sink.opened) {
                sink.failed = Some((e, p.clone()));
            }
            (res, sink.opened, sink.failed, sink.hasher.finalize())
        };
        let discard = |opened: Option<(PathBuf, BufWriter<File>)>| {
            if let Some((p, w)) = opened {
                drop(w);
                let _ignored = std::fs::remove_file(p);
            }
        };
        if let Some((e, shown)) = failed {
            let created = opened.as_ref().map(|(p, _)| p.clone());
            discard(opened);
            return match created {
                // Created but not writable (ENOSPC, EIO): before any worktree mutation.
                Some(p) => Err(Failure::from_io(&e, Some(p.as_os_str()), None)),
                None => self.unwritable_store(&tree, &e, &shown),
            };
        }
        let outcome = match first {
            Ok(o) => o,
            Err(e) => {
                discard(opened);
                return Err(e);
            }
        };
        let (path, mut w) = match (outcome, opened) {
            (DiffIndex::Patch { .. }, Some(v)) => v,
            (other, opened) => {
                discard(opened);
                return Ok(StashOutcome::Nothing {
                    clean: matches!(other, DiffIndex::Clean),
                });
            }
        };
        // 6. durable: the patch, then the journal + pointer (state `stashed`), one directory fsync.
        self.patch = Some(path.clone());
        let durable = w
            .get_ref()
            .sync_all()
            .map_err(|e| Failure::from_io(&e, Some(path.as_os_str()), None))
            .and_then(|()| self.journal_state(State::Stashed))
            .and_then(|()| {
                journal::sync_dir(&self.cx.patches_dir)
                    .map_err(|e| Failure::from_io(&e, Some(self.cx.patches_dir.as_os_str()), None))
            });
        if let Err(e) = durable {
            drop(w);
            self.abandon_patch(&path);
            return Err(e);
        }
        fault::hit(Point::StashState(4));
        // Re-verification: a second digest must equal the streamed one (an autosave between the two is caught and
        // the patch rewritten), at most 3 rewrites.
        let mut digest = digest;
        let mut verified = false;
        let mut rewrites = 0_u8;
        loop {
            let mut check = PatchSink::hash_only();
            match self.git().diff_index_patch(&tree, &mut check) {
                Ok(DiffIndex::Patch { .. }) if check.hasher.finalize() == digest => {
                    verified = true;
                    break;
                }
                Ok(DiffIndex::Patch { .. }) => {}
                Ok(other) => {
                    // The unstaged changes vanished meanwhile: nothing to stash.
                    drop(w);
                    self.abandon_patch(&path);
                    return Ok(StashOutcome::Nothing {
                        clean: matches!(other, DiffIndex::Clean),
                    });
                }
                Err(e) => {
                    drop(w);
                    self.abandon_patch(&path);
                    return Err(e);
                }
            }
            if rewrites >= MAX_REWRITES {
                break;
            }
            rewrites = rewrites.saturating_add(1);
            let rewritten = rewind(&mut w)
                .map_err(|e| Failure::from_io(&e, Some(path.as_os_str()), None))
                .and_then(|()| self.stream_patch(&tree, &mut w, &path))
                .and_then(|(o, d)| {
                    w.get_ref()
                        .sync_all()
                        .map_err(|e| Failure::from_io(&e, Some(path.as_os_str()), None))
                        .map(|()| (o, d))
                });
            match rewritten {
                Ok((DiffIndex::Patch { .. }, d)) => digest = d,
                Ok((other, _)) => {
                    drop(w);
                    self.abandon_patch(&path);
                    return Ok(StashOutcome::Nothing {
                        clean: matches!(other, DiffIndex::Clean),
                    });
                }
                Err(e) => {
                    drop(w);
                    self.abandon_patch(&path);
                    return Err(e);
                }
            }
        }
        drop(w);
        if !verified {
            self.abandon_patch(&path);
            return Err(Failure::fatal("FatalError", messages::KEPT_CHANGING));
        }
        self.printer()
            .log(Level::Warning, messages::UNSTAGED_DETECTED);
        self.printer().log(Level::Info, &messages::stashing(&path));
        // 8. checkout (pre-commit's `try` starts here: a failing checkout still restores).
        self.flags.stash_active = true;
        self.git().checkout_all(true, Group::Isolated)?;
        fault::hit(Point::StashState(5));
        Ok(StashOutcome::Patch(path))
    }

    /// The patch is not needed (or not safe): delete it and the journal state that names it.
    fn abandon_patch(&mut self, path: &Path) {
        let _ignored = std::fs::remove_file(path);
        self.patch = None;
        if let Some(j) = &mut self.journal {
            if self.flags.ita_cleared {
                j.data.patch = None;
                j.data.tree = None;
                let _ignored = j.update(State::Started);
            } else {
                j.remove();
                self.journal = None;
            }
        }
    }

    /// `patches/patch<secs>-<pid>` with `create_new` 0600; EEXIST → `-<n>`.
    fn create_patch_file(&self) -> Result<(PathBuf, File), (io::Error, PathBuf)> {
        self.ensure_patches_dir()
            .map_err(|e| (e, self.cx.patches_dir.clone()))?;
        let base = format!("patch{}-{}", self.cx.clock.unix_secs(), pid());
        let mut last = None;
        for n in 0_u32..1000 {
            let name = if n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            let path = self.cx.patches_dir.join(name);
            match journal::new_file(&path) {
                Ok(f) => return Ok((path, f)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some((e, path)),
                Err(e) => return Err((e, path)),
            }
        }
        Err(last.unwrap_or_else(|| {
            (
                io::Error::from(io::ErrorKind::AlreadyExists),
                self.cx.patches_dir.join(base),
            )
        }))
    }

    /// One `diff-index` streamed into the patch file while hashed; flushed.
    fn stream_patch(
        &self,
        tree: &ObjectId,
        w: &mut BufWriter<File>,
        path: &Path,
    ) -> Result<(DiffIndex, blake3::Hash), Failure> {
        let mut sink = PatchSink {
            file: Some(w),
            hasher: blake3::Hasher::new(),
        };
        let outcome = self.git().diff_index_patch(tree, &mut sink)?;
        sink.flush()
            .map_err(|e| Failure::from_io(&e, Some(path.as_os_str()), None))?;
        Ok((outcome, sink.hasher.finalize()))
    }

    /// X13-ST-04: the patch cannot be created (read-only store). A clean diff needs no patch (parity of success);
    /// otherwise pre-commit's two log lines and its `OSError` (exit 3), with the worktree untouched.
    fn unwritable_store(
        &self,
        tree: &ObjectId,
        e: &io::Error,
        shown: &Path,
    ) -> Result<StashOutcome, Failure> {
        let mut sink = PatchSink::hash_only();
        match self.git().diff_index_patch(tree, &mut sink)? {
            DiffIndex::Patch { .. } => {
                let patch = if shown == self.cx.patches_dir {
                    self.cx.patches_dir.join(format!(
                        "patch{}-{}",
                        self.cx.clock.unix_secs(),
                        pid()
                    ))
                } else {
                    shown.to_path_buf()
                };
                self.printer()
                    .log(Level::Warning, messages::UNSTAGED_DETECTED);
                self.printer().log(Level::Info, &messages::stashing(&patch));
                Err(Failure::from_io(e, Some(shown.as_os_str()), None))
            }
            DiffIndex::Clean => Ok(StashOutcome::Nothing { clean: true }),
            _ => Ok(StashOutcome::Nothing { clean: false }),
        }
    }

    /// D-DATA-17: `FatalError` with the version and executable of the user's git (looked up only on this path).
    fn old_git_refusal(&self) -> Failure {
        let version = self
            .git()
            .run_raw(&[OsStr::new("--version")])
            .ok()
            .and_then(|c| {
                let text = String::from_utf8_lossy(&c.stdout).into_owned();
                let rest = text.trim().strip_prefix("git version ")?.to_owned();
                let v: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                let v = v.trim_end_matches('.').to_owned();
                (!v.is_empty()).then_some(v)
            })
            .unwrap_or_else(|| "(unknown version)".to_owned());
        let exe = snp_proc::resolve::normalize_cmd(
            &[OsString::from("git")],
            self.git().env(),
            self.git().root(),
        )
        .ok()
        .and_then(|argv| argv.into_iter().next())
        .map_or_else(|| "git".to_owned(), |a| a.to_string_lossy().into_owned());
        Failure::fatal("FatalError", messages::old_git(&version, &exe))
    }

    /// `stage_fixed` (ARCHITECTURE §4.5, §8.3).
    fn stage_fixed(&mut self, paths: &[RepoPath]) -> Result<bool, Failure> {
        if paths.is_empty() {
            return Ok(true);
        }
        let Some(patch) = self.patch.clone().filter(|_| self.flags.stash_active) else {
            // Nothing stashed: the index can take the fixes directly (no user content to protect).
            self.git().add_literal(paths, Group::Isolated)?;
            self.staged_fixed.extend_from_slice(paths);
            return Ok(true);
        };
        if !self.git().apply_check(&patch)? {
            return Ok(false);
        }
        self.staged_fixed.extend_from_slice(paths);
        self.journal_state(State::StagingFixed)?;
        fault::hit(Point::StashState(6));
        self.git().add_literal(paths, Group::Isolated)?;
        fault::hit(Point::StashState(7));
        Ok(true)
    }

    /// One plain `git apply --whitespace=nowarn <patch>` attempt; a SIGTTIN/SIGTTOU stop is reported once.
    fn apply_plain(&mut self, patch: &Path) -> bool {
        let args = [
            OsStr::new("apply"),
            OsStr::new("--whitespace=nowarn"),
            patch.as_os_str(),
        ];
        match self.git().run_raw(&args) {
            Ok(c) => {
                if c.stopped_by_tty && !self.tty_noted {
                    self.tty_noted = true;
                    self.printer().diag(messages::TTY_STOPPED);
                }
                c.status.0 == 0
            }
            Err(_) => false,
        }
    }

    /// `_git_apply` (`staged_files_only.py:26-33`): plain, then `-c core.autocrlf=false`; the second failure is its
    /// `CalledProcessError`.
    fn git_apply(&mut self, patch: &Path, first: u8) -> Result<(), Failure> {
        fault::hit(Point::RestoreCommand(first));
        if self.apply_plain(patch) {
            return Ok(());
        }
        fault::hit(Point::RestoreCommand(first.saturating_add(1)));
        match self.git().apply(patch, true, Group::Isolated) {
            Ok(true) => Ok(()),
            Ok(false) => Err(Failure::unexpected(
                "CalledProcessError",
                "git apply failed",
            )),
            Err(e) => Err(e),
        }
    }

    /// Step 10 a–e: re-apply the patch (rollback on conflict); the K+ hint on final failure.
    fn restore_patch(&mut self, patch: &Path) -> Result<(), Failure> {
        let first = self.git_apply(patch, 1);
        let result = if first.is_ok() {
            Ok(())
        } else {
            self.journal_best_effort(State::RollingBack);
            fault::hit(Point::StashState(8));
            self.printer()
                .log(Level::Warning, messages::CONFLICT_ROLLBACK);
            fault::hit(Point::RestoreCommand(3));
            self.git()
                .checkout_all(true, Group::Isolated)
                .and_then(|()| self.git_apply(patch, 4))
        };
        match result {
            Ok(()) => {
                self.printer().log(Level::Info, &messages::restored(patch));
                self.journal_best_effort(State::Restored);
                fault::hit(Point::StashState(9));
                if self.cx.verify && !matches!(crate::verify_restored(self.git(), patch), Ok(true))
                {
                    self.printer().diag(&messages::verify_failed(patch));
                }
                Ok(())
            }
            Err(e) => {
                self.journal_best_effort(State::Failed);
                Err(with_hint(e, &messages::k_plus_hint(patch)))
            }
        }
    }

    /// Steps 10–11: restore, intent-to-add re-add, journal cleanup, unlock.
    ///
    /// Resumable: each step marks itself done, so the panic fallback (`Drop`) re-entered after a panic inside one of
    /// them continues where it stopped instead of re-applying a restored patch.
    fn finish(&mut self) -> Result<(), Failure> {
        let shield = self.cx.signals.as_ref().map(|s| s.shield());
        let mut result = Ok(());
        if self.flags.stash_active
            && let Some(patch) = self.patch.clone()
        {
            let restored = self.restore_patch(&patch);
            self.flags.stash_active = false;
            if let Err(e) = restored {
                self.flags.restore_failed = true;
                result = Err(e);
            }
        }
        if self.flags.ita_cleared {
            fault::hit(Point::RestoreCommand(6));
            let readd = self.git().add_intent_to_add(&self.ita, Group::Isolated);
            self.flags.ita_cleared = false;
            if let (Err(e), Ok(())) = (readd, &result) {
                result = Err(e);
            }
        }
        if !self.flags.restore_failed
            && let Some(j) = self.journal.take()
        {
            // Done. (K+: patch, journal and pointer are kept forever, never gc'd.)
            j.remove();
        }
        fault::hit(Point::StashState(10));
        if let Some(l) = self.lock.take() {
            l.release();
        }
        drop(shield);
        result
    }
}

/// `Failure` with the K+ hint attached (exceptions only).
fn with_hint(e: Failure, hint: &str) -> Failure {
    match e {
        Failure::Exc { kind, exc } => Failure::Exc {
            kind,
            exc: exc.with_hint(hint),
        },
        other => other,
    }
}

/// Truncate a buffered patch file for a rewrite.
fn rewind(w: &mut BufWriter<File>) -> io::Result<()> {
    w.flush()?;
    let f = w.get_mut();
    f.set_len(0)?;
    f.rewind()?;
    Ok(())
}

impl StashGuard {
    /// What was stashed.
    pub fn stashed(&self) -> &Stashed {
        &self.stashed
    }

    /// `diff-index` rc 0 ⇒ baseline digest known empty.
    pub fn diff_index_clean(&self) -> bool {
        self.clean
    }

    /// `("_SNP_STASH_LOCK", token)` for nested runs.
    pub fn child_env_token(&self) -> Option<(&'static str, OsString)> {
        let inner = self.inner.as_deref()?;
        (!inner.token.is_empty()).then(|| (lock::TOKEN_ENV, OsString::from(&inner.token)))
    }

    /// `stage_fixed` (§8.3): apply `--check`; journal `staging-fixed {paths}` durably; `git --literal-pathspecs add
    /// -- <paths>`; `Ok(false)` = would not apply (nothing staged).
    ///
    /// snp-only (05 §18.4 A4/A5): `git apply --check --whitespace=nowarn <patch>` against the hook-fixed worktree; when
    /// it applies, the `staging-fixed` state is journaled durably *before* the add, so a kill leaves a recoverable state
    /// (`git reset -q <tree> -- <paths>` + `git apply --3way <patch>`). Without a stashed patch the paths are staged
    /// directly.
    pub fn stage_fixed(&mut self, paths: &[RepoPath]) -> Result<bool, Failure> {
        match self.inner.as_deref_mut() {
            Some(inner) => inner.stage_fixed(paths),
            None => Ok(false),
        }
    }

    /// The explicit restore (§4.2 step 10).
    ///
    /// The `finally` blocks of `_unstaged_changes_cleared` and `_intent_to_add_cleared` (`staged_files_only.py:83-98,
    /// 44-45`): `git apply` → `git -c core.autocrlf=false apply` → `[WARNING] Stashed changes conflicted …` + rollback
    /// checkout + both applies again → `[INFO] Restored changes from <patch>.`; then `git add --intent-to-add`. A final
    /// failure is the last `CalledProcessError` with the K+ hint (D-DATA-01); patch, journal and pointer are kept.
    pub fn restore(mut self) -> Result<(), Failure> {
        self.finish()
    }

    /// Runs the restore with the state still in place: a panic inside it leaves `inner` for the `Drop` fallback,
    /// which resumes the remaining steps.
    fn finish(&mut self) -> Result<(), Failure> {
        let result = match self.inner.as_deref_mut() {
            Some(inner) => inner.finish(),
            None => Ok(()),
        };
        self.inner = None;
        result
    }
}

impl Drop for StashGuard {
    /// The panic fallback (ADR-015, ADR-066): restore inside `catch_unwind`, printing a failure on stderr.
    fn drop(&mut self) {
        let Some(mut inner) = self.inner.take() else {
            return;
        };
        let printer = Arc::clone(&inner.cx.printer);
        let outcome = catch_unwind(AssertUnwindSafe(|| inner.finish()));
        if let Ok(Err(Failure::Exc { exc, .. })) = outcome {
            printer.diag(&messages::drop_restore_failed(
                &exc.ty,
                &String::from_utf8_lossy(&exc.text),
            ));
            if let Some(hint) = &exc.hint {
                printer.diag(hint);
            }
        }
    }
}
