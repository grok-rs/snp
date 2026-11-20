//! The only way a native hook writes the work tree (ADR-132): a [`Writer`] token from [`HookCx::writer`].
//!
//! In a run whose selected, unskipped hooks all run in process (no multi-hook group, no active `stage_fixed`, no
//! baseline already known), the runner no longer takes the diff oracle's baseline `git diff` before each hook.
//! Instead it hands the hook a [`WriteGate`]: the first [`HookCx::writer`] call of the hook runs the real `git diff`
//! (pre-commit's `_get_diff()` before the hook), every other partition that wants to write blocks until that diff is
//! done, and partitions that only read keep running (both sides only read). No byte is written before the baseline is
//! complete, so the baseline is the pre-hook tree, as when the runner took it first. A hook that never writes never
//! fires the gate, and its run takes no `git diff` at all.
//!
//! Without a gate ([`HookCx::gate`] `None`: the baseline is already known, or the run is outside ADR-132's static
//! gate) [`HookCx::writer`] hands out the token at once.
//!
//! Outside this module the crate's `clippy.toml` (`disallowed-methods` / `disallowed-types`, kept complete by
//! `cargo xtask lint-guards`) bans the `std::fs` mutators (path functions, `OpenOptions`, `File::create*`, and the
//! `set_len` / `set_permissions` / `set_times` / `set_modified` methods, which work on a handle opened read-only),
//! `DirBuilder::create`, the Unix `chown` family, symlink creation, every write function of `snp_os::fs` and
//! `snp_os::lock::FileLock`'s acquisition (it creates the lock file); ADR-132 (5) lists them. The lint cannot see a
//! subprocess (`snp_proc`, the git children of `snp_git`) or another crate's write API: a native that would write
//! that way must call [`HookCx::writer`] first (doc rule).
//!
//! The gate's `git diff` takes no pool permit (the git backend streams it directly), so a partition that holds the
//! run's only permit (`SNP_JOBS=1`) cannot deadlock on it.
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::io;
use std::sync::OnceLock;

use snp_core::failure::{Failure, PyExc};
use snp_git::{DiffFingerprint, Git};

use crate::HookCx;
use crate::framework;

/// The deferred baseline of one hook (ADR-132): the real `git diff` the first writer runs.
#[derive(Debug)]
pub struct WriteGate {
    git: Git,
    diff: OnceLock<Result<DiffFingerprint, Failure>>,
}

impl WriteGate {
    /// A gate that has not fired, over the user repository `git` (the run's `Git`, whose diff is the oracle's).
    pub fn new(git: &Git) -> Self {
        Self {
            git: git.clone(),
            diff: OnceLock::new(),
        }
    }

    /// Fires the gate: the first caller runs `git diff`, concurrent callers block until it is done. `true` when
    /// the baseline was taken (a failing diff is kept for the runner and refuses every write).
    fn fire(&self) -> bool {
        self.diff
            .get_or_init(|| self.git.diff_fingerprint())
            .is_ok()
    }

    /// Whether a writer fired the gate.
    pub fn fired(&self) -> bool {
        self.diff.get().is_some()
    }

    /// After the hook: the baseline its first writer took, `None` when nothing asked to write.
    ///
    /// # Errors
    ///
    /// The `git diff` failure, which the runner reports like a failing baseline (exit 3).
    pub fn into_baseline(self) -> Option<Result<DiffFingerprint, Failure>> {
        self.diff.into_inner()
    }
}

/// The right to write the work tree during one hook: the gate (if any) has fired and its baseline is complete.
#[derive(Debug)]
pub struct Writer<'c, 'a> {
    cx: &'c HookCx<'a>,
}

impl<'a> HookCx<'a> {
    /// The write token (ADR-132). With a [`WriteGate`], the first call of the hook runs the baseline `git diff` and
    /// concurrent calls wait for it.
    ///
    /// # Errors
    ///
    /// The gate's `git diff` failed: nothing may be written. The runner discards the partition's outcome and
    /// reports the failure itself, so the exception's text is never printed.
    pub fn writer(&self) -> Result<Writer<'_, 'a>, PyExc> {
        match self.gate {
            Some(g) if !g.fire() => Err(PyExc::new(
                "RuntimeError",
                "snp: the baseline git diff failed; nothing was written",
            )),
            _ => Ok(Writer { cx: self }),
        }
    }
}

impl Writer<'_, '_> {
    /// Drops `name` from the window's content cache before it is written (ADR-131), whatever the write then does.
    fn evict(&self, name: &OsStr) {
        if let Some(c) = self.cx.cache.and_then(|c| c.view(self.cx.root)) {
            c.evict(name);
        }
    }

    /// `open(name, 'wb').write(bytes)` on the same inode (09 §2.8, [`snp_os::fs::rewrite_in_place`]).
    ///
    /// # Errors
    ///
    /// The open or write error (the caller maps it to the Python exception).
    #[expect(
        clippy::disallowed_methods,
        reason = "the writer module is the crate's only write site (ADR-132)"
    )]
    pub fn rewrite(&self, name: &OsStr, bytes: &[u8]) -> io::Result<()> {
        self.evict(name);
        snp_os::fs::rewrite_in_place(&framework::at(self.cx, name), bytes)
    }

    /// Appends one `\n` (end-of-file-fixer's `f.write(b'\n')` at the end).
    ///
    /// # Errors
    ///
    /// The open or write error.
    #[expect(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "the writer module is the crate's only write site (ADR-132)"
    )]
    pub fn append_newline(&self, name: &OsStr) -> io::Result<()> {
        use io::Write as _;
        self.evict(name);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(framework::at(self.cx, name))?;
        f.write_all(b"\n")
    }

    /// Truncates to `len` bytes (end-of-file-fixer's `f.truncate()`).
    ///
    /// # Errors
    ///
    /// The open or truncate error.
    #[expect(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "the writer module is the crate's only write site (ADR-132)"
    )]
    pub fn truncate(&self, name: &OsStr, len: u64) -> io::Result<()> {
        self.evict(name);
        std::fs::OpenOptions::new()
            .write(true)
            .open(framework::at(self.cx, name))?
            .set_len(len)
    }
}
