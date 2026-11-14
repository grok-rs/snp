//! The store handle: lazy creation, locks, materialisation, install markers, tracking, sweep (§5).
//!
//! pre-commit's `Store` (`store.py`) keeps a SQLite database of clones and tracked configs and serialises every
//! clone and install behind one exclusive `<store>/.lock`. snp's store is content-addressed (no database to repair,
//! D-CRASH-17), created lazily (X13-ST-01), crash-consistent (scratch + durable marker + rename) and locks per
//! repository, per tool and globally shared/exclusive (ARCHITECTURE §5.5).
//!
//! Owner: WP-22.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use snp_core::brand::Brand;
use snp_core::cancel::{CancelReason, CancelToken};
use snp_core::failure::Failure;
use snp_core::fault::{self, Point};
use snp_core::lang::LanguageId;
use snp_core::output::DiagSink;
use snp_os::fs::Ownership;
use snp_os::lock::{FileLock, LockMode};

use crate::keys::{
    self, MarkerKey, RepoHandle, RepoKey, RepoKind, RepoMeta, ToolRef, TrackedConfig,
};
use crate::layout::{self, mkdir_private};
use crate::location::{StoreLocation, StoreSource};
use crate::{maintenance, marker, messages};

/// Whether the `Locking … directory` callback already ran in this process (ARCHITECTURE §5.5: once per process).
static LOCK_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);
/// Whether the foreign-owner warning was already printed in this process (D-DIAG-05).
static FOREIGN_WARNED: AtomicBool = AtomicBool::new(false);

/// The held global store lock (shared for runs, exclusive for gc/clean).
///
/// pre-commit's `store.exclusive_lock()` (`store.py:103-109`) for gc; snp runs hold the lock *shared* from the
/// first repository use until exit, so gc can never delete a repository in use. An ephemeral store's guard also
/// holds the shared lock of the main store whose tools it uses.
#[derive(Debug)]
pub struct StoreGuard(
    /// The held locks (released on drop).
    #[expect(dead_code, reason = "held only for its Drop, which releases the locks")]
    Vec<FileLock>,
);

/// What `ensure_created` established (once per `Store`).
#[derive(Clone, Debug)]
struct Created {
    /// `store-v1.json` recorded another path.
    relocated_from: Option<PathBuf>,
}

/// snp's store (`~/.cache/snp`).
///
/// The counterpart of pre-commit's `Store` (`store.py`): no SQLite, a versioned content-addressed layout (ARCHITECTURE §5.2).
#[derive(Debug)]
pub struct Store {
    /// Where the store lives.
    loc: StoreLocation,
    /// Where `tools-v1` and `cache-v1` live (the store itself, or the main store of an ephemeral one).
    tools_root: PathBuf,
    /// An ephemeral store's main store: created and policy-checked by our `ensure_created`, its shared lock held
    /// with ours.
    shared: Option<Box<Self>>,
    /// try-repo's throwaway store (ADR-059): no README, no ownership policy, no relocation tracking.
    ephemeral: bool,
    /// Set by a successful `ensure_created`.
    created: OnceLock<Created>,
}

impl Store {
    /// No I/O (lazy, 03 Q12).
    ///
    /// pre-commit's `Store.__init__` (`store.py:60-81`) creates the directory, the README and the database right
    /// away, for every command; snp creates nothing until a command needs the store (X13-ST-01).
    pub fn new(loc: StoreLocation) -> Self {
        let tools_root = loc.path.clone();
        Self {
            loc,
            tools_root,
            shared: None,
            ephemeral: false,
            created: OnceLock::new(),
        }
    }

    /// try-repo's throwaway store rooted in its `TempDir` (ADR-059): repos-v1, keys-v1, configs-v1, patches and the
    /// global lock live under `root`; tools-v1 and cache-v1 come from `shared` (invisible: tool downloads are snp's
    /// own).
    ///
    /// `try_repo.py` runs against `Store(tempdir)` (03 §12 step 3), so every try-repo clones and installs from
    /// scratch and prints the `Initializing`/`Installing` lines (X13-SM-01). No I/O.
    pub fn ephemeral(root: &Path, shared: Option<&Self>) -> Self {
        let loc = StoreLocation {
            path: root.to_path_buf(),
            source: StoreSource::SnpHome,
        };
        let (tools_root, shared) = match shared {
            Some(s) => (s.tools_root.clone(), Some(Box::new(s.handle_copy()))),
            None => (root.to_path_buf(), None),
        };
        Self {
            loc,
            tools_root,
            shared,
            ephemeral: true,
            created: OnceLock::new(),
        }
    }

    /// Where the store lives.
    ///
    /// pre-commit's `Store.directory` (`store.py:60-62`).
    pub fn location(&self) -> &StoreLocation {
        &self.loc
    }

    /// Whether the store directory exists.
    ///
    /// `os.path.exists(self.directory)` of `Store.__init__` (`store.py:66-70`); one `stat`, nothing is created.
    pub fn exists(&self) -> bool {
        self.root().is_dir()
    }

    /// 0700, README, store-v1.json.
    ///
    /// `store.py:62-81` (`makedirs` + README when pre-commit creates the directory), plus snp's ownership policy
    /// (ADR-044, ADR-077): a world-writable store without the sticky bit is refused; a store owned by another uid
    /// is used with one stderr warning (D-DIAG-05), except as root over a non-root owner's store, which is refused
    /// unless `SNP_ALLOW_FOREIGN_STORE=1` (D-SEC-13). Creates the layout's top-level directories (0700) and records
    /// the store path in `store-v1.json`; a different recorded path is reported by [`Store::relocated_from`]. A
    /// read-only store is accepted as it is (parity, 03 §7.9). Idempotent; cheap after the first success.
    ///
    /// When the recorded path differs (the store was moved, 11 §A.10.4), every environment's install state is
    /// removed under its repository lock *before* the new path is recorded, so the invalidation is durable: no later
    /// process uses an environment with the old absolute paths baked in, and a crash in between repeats it.
    ///
    /// An ephemeral store first runs its main store's `ensure_created` (layout, README, ownership policy), whose
    /// tools it uses.
    ///
    /// # Errors
    ///
    /// The `OSError` of the failing `mkdir` (`PermissionError: [Errno 13] Permission denied: '<dir>'`, exit 3,
    /// X13-ST-01), `NotADirectoryError` for a store path that is a file (X13-ST-02), or the `FatalError` refusals
    /// above (exit 1).
    pub fn ensure_created(&self, brand: Brand, diag: &dyn DiagSink) -> Result<(), Failure> {
        // The store's texts are brand-independent (messages.rs); the brand is only passed on to the main store.
        let _: Brand = brand;
        if self.created.get().is_some() {
            return Ok(());
        }
        if let Some(main) = &self.shared {
            main.ensure_created(brand, diag)?;
        }
        let root = self.root();
        let existed = match fs::metadata(root) {
            Ok(md) if md.is_dir() => true,
            Ok(_) => {
                return Err(Failure::from_io(
                    &not_a_directory(),
                    Some(root.join(layout::LOCK).as_os_str()),
                    None,
                ));
            }
            Err(_) => false,
        };
        if existed && !self.ephemeral && world_writable_without_sticky(root) {
            return Err(Failure::fatal(
                "FatalError",
                messages::refuse_world_writable_store(root),
            ));
        }
        let ownership =
            snp_os::fs::ensure_private_dir(root).map_err(|e| mkdir_failure(root, &e))?;
        if !self.ephemeral {
            self.check_ownership(ownership, diag)?;
        }
        let readonly = existed && is_readonly(root);
        let relocated_from = if !self.ephemeral && !readonly {
            self.write_readme_if_new(existed);
            self.track_location()
        } else {
            None
        };
        let mut dirs: Vec<PathBuf> = [
            layout::REPOS,
            layout::KEYS,
            layout::CONFIGS,
            layout::PATCHES,
            layout::SPILL,
            layout::SCRATCH,
            layout::TRASH,
        ]
        .iter()
        .map(|d| root.join(d))
        .collect();
        if self.shared.is_none() {
            dirs.push(self.tools_root.join(layout::TOOLS));
            dirs.push(self.tools_root.join(layout::CACHE));
        }
        for d in dirs {
            if let Err(e) = mkdir_private(&d)
                && !readonly
            {
                return Err(mkdir_failure(&d, &e));
            }
        }
        let _already: Result<(), Created> = self.created.set(Created { relocated_from });
        Ok(())
    }

    /// The shared global lock (runs using repos).
    ///
    /// `on_wait` runs at most once per process across every store lock (the `[INFO] Locking … directory` line,
    /// `store.py:105-106`, [`crate::locking_message`]); the wait observes `c` every 50 ms. An ephemeral store also
    /// takes its main store's shared lock (created by `ensure_created`), so gc there cannot remove tools in use.
    ///
    /// # Errors
    ///
    /// `Failure::Interrupted` when `c` is cancelled while waiting; the `OSError` of opening `<store>/.lock`
    /// (`IsADirectoryError`, `PermissionError`, …; X13-ST-11) otherwise.
    pub fn shared(
        &self,
        on_wait: &mut dyn FnMut(),
        c: &CancelToken,
    ) -> Result<StoreGuard, Failure> {
        let mut held = vec![lock_at(
            &self.root().join(layout::LOCK),
            LockMode::Shared,
            on_wait,
            c,
        )?];
        if let Some(main) = &self.shared {
            held.push(lock_at(
                &main.root().join(layout::LOCK),
                LockMode::Shared,
                on_wait,
                c,
            )?);
        }
        Ok(StoreGuard(held))
    }

    /// The exclusive global lock (gc/clean).
    ///
    /// # Errors
    ///
    /// As [`Store::shared`].
    pub fn exclusive(
        &self,
        on_wait: &mut dyn FnMut(),
        c: &CancelToken,
    ) -> Result<StoreGuard, Failure> {
        let lock = lock_at(
            &self.root().join(layout::LOCK),
            LockMode::Exclusive,
            on_wait,
            c,
        )?;
        Ok(StoreGuard(vec![lock]))
    }

    /// The key of a remote repository at `rev` with `deps`.
    ///
    /// `blake3("snp.repo.v1\0" ‖ lp(repo_id) ‖ lp(rev) ‖ json(deps))[..16]`: pre-commit's `(db_repo_name(repo,
    /// deps), rev)` row key (`store.py:125-171`), with a local directory identified by its canonical path
    /// (D-DATA-08). Dependency order matters (T10-STO-10).
    pub fn remote_key(id: &crate::RepoId, rev: &str, deps: &[Arc<str>]) -> RepoKey {
        keys::remote_key(id, rev, deps)
    }

    /// Includes `snp_core::profile::LOCAL_TEMPLATE_VERSION`.
    ///
    /// pre-commit's `make_local(deps)` row `('local[:deps]', C.LOCAL_REPO_VERSION)` (`store.py:212-215`).
    pub fn local_key(deps: &[Arc<str>]) -> RepoKey {
        keys::local_key(deps)
    }

    /// One stat of the marker; self-heals.
    ///
    /// pre-commit's `SELECT path FROM repos WHERE repo = ? AND ref = ?` (`store.py:142-153`) returns a row even when
    /// its directory is gone (X13-ST-10). snp returns an entry only when its `.snp-repo.json` commit marker parses,
    /// derives `key` and the checkout is present (`src/.git` for clones, `src/` for local templates); anything else is
    /// "not materialised" and the next [`Store::materialize`] replaces it under the repository lock.
    pub fn lookup(&self, key: &RepoKey) -> Option<RepoHandle> {
        self.load(key)
    }

    /// Under the per-repo lock: re-lookup; else `fill(scratch/<rand>/src)`, write `.snp-repo.json`, rename into place.
    ///
    /// pre-commit's `_new_repo` (`store.py:132-171`): query, lock, query again, clone into `mkdtemp`, insert the row;
    /// a failure removes the directory (`clean_path_on_failure`). snp fills an empty `scratch/<rand>/src`
    /// (same filesystem), makes the marker durable, then renames the whole entry into `repos-v1/<key>` (ADR-068
    /// retries on Windows). A process killed at any step leaves only its scratch directory (swept after an hour),
    /// so a partial entry is never used. Losing a rename race discards ours and returns the winner's entry.
    ///
    /// # Errors
    ///
    /// `fill`'s error (its scratch directory removed), `Failure::Interrupted` on cancellation, the `OSError` of a
    /// store operation (`PermissionError` in a read-only store, X13-ST-03), or an `AssertionError` when `meta` does
    /// not derive `key`.
    pub fn materialize(
        &self,
        key: &RepoKey,
        meta: RepoMeta,
        fill: &mut dyn FnMut(&Path) -> Result<(), Failure>,
        on_wait: &mut dyn FnMut(),
        c: &CancelToken,
    ) -> Result<RepoHandle, Failure> {
        if meta.key() != *key {
            return Err(Failure::unexpected(
                "AssertionError",
                format!("snp: store key {} does not match its metadata", key.hex()),
            ));
        }
        if let Load::Present(h) = self.load_entry(key) {
            return Ok(h);
        }
        let _lock = self.repo_lock(key, on_wait, c)?;
        match self.load_entry(key) {
            Load::Present(h) => return Ok(h),
            // Not definitively broken (EIO, EMFILE, EACCES, …): another process may be running hooks from it.
            Load::Error(e, p) => return Err(Failure::from_io(&e, Some(p.as_os_str()), None)),
            Load::Absent => {}
        }
        if c.is_cancelled() {
            return Err(interrupted(c));
        }
        let dest = self.repo_dir(key);
        // A definitively broken entry (no or bad marker, checkout gone): replace it, under the lock.
        if fs::symlink_metadata(&dest).is_ok() {
            snp_os::fs::retire_dir(&dest, &self.root().join(layout::TRASH))
                .map_err(|e| Failure::from_io(&e, Some(dest.as_os_str()), None))?;
        }
        let scratch_root = self.root().join(layout::SCRATCH);
        mkdir_private(&scratch_root).map_err(|e| mkdir_failure(&scratch_root, &e))?;
        let scratch = scratch_root.join(layout::random_name());
        let built = Self::build_entry(&scratch, &meta, fill);
        let result = built.and_then(|()| match snp_os::fs::rename_with_retry(&scratch, &dest) {
            Ok(()) => Ok(true),
            // EEXIST/ENOTEMPTY: lost a race against a process that does not honour the lock; theirs wins.
            Err(_) if fs::symlink_metadata(dest.join(layout::REPO_MARKER)).is_ok() => Ok(false),
            Err(e) => Err(Failure::from_io(
                &e,
                Some(scratch.as_os_str()),
                Some(dest.as_os_str()),
            )),
        });
        if fs::symlink_metadata(&scratch).is_ok() {
            let _removed: io::Result<()> = snp_os::fs::remove_tree_within(&scratch, &scratch_root);
        }
        let won = result?;
        layout::sync_dir(&self.root().join(layout::REPOS));
        if won {
            return Ok(RepoHandle {
                key: *key,
                src: dest.join("src"),
                envs: dest.join("envs"),
                meta,
            });
        }
        self.load(key).ok_or_else(|| {
            Failure::unexpected(
                "AssertionError",
                format!(
                    "snp: store entry {} vanished after materialisation",
                    key.hex()
                ),
            )
        })
    }

    /// The per-repository lock.
    ///
    /// `repos-v1/<key>.lock`, exclusive: held for the clone and every environment install of that prefix, so distinct
    /// repositories clone and install in parallel (pre-commit serialises everything behind `<store>/.lock`). Never
    /// creates the store root: a store that skipped `ensure_created` would escape its ownership policy and README.
    ///
    /// # Errors
    ///
    /// As [`Store::shared`]; `FileNotFoundError` naming the lock file when the store does not exist yet.
    pub fn repo_lock(
        &self,
        key: &RepoKey,
        on_wait: &mut dyn FnMut(),
        c: &CancelToken,
    ) -> Result<FileLock, Failure> {
        let mut name = key.hex();
        name.push_str(".lock");
        let path = bucket_in_existing(self.root(), layout::REPOS, &name)?;
        lock_at(&path, LockMode::Exclusive, on_wait, c)
    }

    /// `tools-v1/<tool>.lock`.
    ///
    /// Exclusive, for a toolchain bootstrap (one download per tool). An unsafe `tool` name is hashed (never a path).
    /// Like [`Store::repo_lock`] it never creates the store root (that is `ensure_created`'s job, with its policy).
    ///
    /// # Errors
    ///
    /// As [`Store::shared`]; `FileNotFoundError` naming the lock file when the store does not exist yet.
    pub fn tool_lock(
        &self,
        tool: &str,
        on_wait: &mut dyn FnMut(),
        c: &CancelToken,
    ) -> Result<FileLock, Failure> {
        let mut name = layout::safe_component(tool);
        name.push_str(".lock");
        let path = bucket_in_existing(&self.tools_root, layout::TOOLS, &name)?;
        lock_at(&path, LockMode::Exclusive, on_wait, c)
    }

    /// Sanitized-or-hashed environment directory (§5.4).
    ///
    /// pre-commit's `lang_base.environment_dir` is `normpath(join(prefix, f'{ENVIRONMENT_DIR}-{version}'))`
    /// (`lang_base.py:90-91`, 03 §8.3), so `language_version: ../../x` escapes the prefix and the install step deletes
    /// it (D-SEC-01). snp uses `<ENV_DIR>-<version>` only when `version` matches `^[A-Za-z0-9._+-]{1,64}$` and cannot
    /// alias another name (no trailing `.`; no upper-case ASCII on the case-folding filesystems of Windows and macOS),
    /// else `<ENV_DIR>-h<hex16(blake3(version))>` (ADR-009): the name is one component inside `repo.envs`, and two
    /// distinct versions never share a directory.
    /// Env-less languages (no `ENVIRONMENT_DIR`) use their language name as the base.
    pub fn env_dir(&self, repo: &RepoHandle, lang: LanguageId, version: &str) -> PathBuf {
        let base = lang.env_dir_base().unwrap_or_else(|| lang.as_str());
        let name = if layout::is_safe_component(version) {
            format!("{base}-{version}")
        } else {
            format!("{base}-h{}", keys::version_hash16(version))
        };
        repo.envs.join(name)
    }

    /// `.snp-install-state.json`.
    ///
    /// The bytes of the environment's install state (ADR-012) when it exists and is one JSON value (at most 1 MiB);
    /// `None` otherwise — a missing, truncated or garbled state means "not installed" (X13-ENV-01; pre-commit
    /// crashes on a malformed `.install_state_v1`).
    pub fn read_install_marker(env_dir: &Path) -> Option<Vec<u8>> {
        let bytes =
            snp_os::fs::read_capped(&env_dir.join(layout::INSTALL_STATE), marker::META_CAP).ok()?;
        marker::is_json_value(&bytes).then_some(bytes)
    }

    /// Writes the store-owned `.snp-tools.json` sidecar (`tools`), then `.snp-install-state.json` (temp + fsync +
    /// rename: the commit point). `json` stays opaque to the store.
    ///
    /// pre-commit writes `.install_state_v1staging` and `os.replace`s it (`repository.py:90-100`); snp's rename
    /// retries Windows sharing violations (ADR-068).
    ///
    /// # Errors
    ///
    /// The I/O error of either write (the state is then not committed).
    pub fn commit_install_marker(env_dir: &Path, json: &[u8], tools: &[ToolRef]) -> io::Result<()> {
        snp_os::fs::atomic_replace(
            &env_dir.join(layout::TOOLS_SIDECAR),
            &marker::encode_tools(tools),
            None,
        )?;
        fault::hit(Point::InstallCommit);
        snp_os::fs::atomic_replace(&env_dir.join(layout::INSTALL_STATE), json, None)
    }

    /// Whether a message-parity marker exists.
    ///
    /// `keys-v1/<hex32>` (ADR-032): pre-commit prints `[INFO] Initializing environment for {db_repo_name}.` exactly
    /// when it inserts a new `repos` row; the marker records that this store has "inserted" it. One `lstat`.
    pub fn marker_exists(&self, k: &MarkerKey) -> bool {
        fs::symlink_metadata(self.root().join(layout::KEYS).join(k.file_name())).is_ok()
    }

    /// Best effort, `create_new`.
    ///
    /// Never creates the store itself (the caller has called `ensure_created`); a failure only means the line is
    /// printed again next time.
    pub fn set_marker(&self, k: &MarkerKey) {
        if !self.exists() {
            return;
        }
        let dir = self.root().join(layout::KEYS);
        if mkdir_private(&dir).is_err() {
            return;
        }
        let path = dir.join(k.file_name());
        let _written: io::Result<()> = (|| {
            let mut o = fs::OpenOptions::new();
            o.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.mode(0o600);
            }
            let mut f = o.open(&path)?;
            io::Write::write_all(&mut f, &k.encode())
        })();
    }

    /// Keeps the `keys-v1` markers `keep` accepts and removes the others; returns how many were removed.
    ///
    /// gc's marking (`snp_cmd::gc`, ADR-075) decides which `(db_repo_name, rev)` and virtual-install markers are still
    /// backed by a present or Tier-A-loadable repository. Undecodable files are removed. Additive API (WP-22).
    pub fn retain_markers(&self, keep: &mut dyn FnMut(&MarkerKey) -> bool) -> usize {
        maintenance::retain_markers(&self.root().join(layout::KEYS), keep)
    }

    /// Best effort, lock-free, never creates the store.
    ///
    /// `Store.mark_config_used` (`store.py:225-235`, T10-STO-11/12): a no-op for a read-only or missing store or a
    /// config that does not exist; otherwise records `(cfg_realpath, project_root)` in `configs-v1` (one `stat` when
    /// already tracked; written by temp + rename, so a reader never sees a partial entry). Never blocks on a lock
    /// (X13-ST-05) and never fails (X13-ST-06…09).
    pub fn mark_config_used(&self, cfg_realpath: &Path, project_root: &Path) {
        let root = self.root();
        if !root.is_dir() || fs::symlink_metadata(cfg_realpath).is_err() {
            return;
        }
        let dir = root.join(layout::CONFIGS);
        let entry = dir.join(keys::config_entry_name(cfg_realpath, project_root));
        if fs::symlink_metadata(&entry).is_ok() || is_readonly(root) {
            return;
        }
        if mkdir_private(&dir).is_err() {
            return;
        }
        let _written: io::Result<()> = snp_os::fs::atomic_replace(
            &entry,
            &marker::encode_config_entry(cfg_realpath, project_root),
            None,
        );
    }

    /// Every tracked config (configs-v1).
    ///
    /// gc's `SELECT path FROM configs` (`gc.py:65-70`) with each config's recorded project root (ADR-075), sorted;
    /// malformed entries are skipped.
    pub fn tracked_configs(&self) -> Vec<TrackedConfig> {
        let mut out: Vec<TrackedConfig> =
            maintenance::config_entries(&self.root().join(layout::CONFIGS))
                .into_iter()
                .map(|(_, path, project_root)| TrackedConfig { path, project_root })
                .collect();
        out.sort_by(|a, b| (&a.path, &a.project_root).cmp(&(&b.path, &b.project_root)));
        out.dedup();
        out
    }

    /// Forgets configs that no longer exist.
    ///
    /// gc's `DELETE FROM configs WHERE path = ?` (`gc.py:82-84`): every entry recording one of `dead` is removed.
    pub fn forget_configs(&self, dead: &[PathBuf]) {
        for (file, path, _) in maintenance::config_entries(&self.root().join(layout::CONFIGS)) {
            if dead.contains(&path) {
                let _removed: io::Result<()> = fs::remove_file(file);
            }
        }
    }

    /// Every materialised repository.
    ///
    /// gc's `SELECT repo, ref, path FROM repos` (`gc.py:65-70`): the complete entries of `repos-v1`, ordered by key.
    pub fn all_repos(&self) -> Vec<RepoHandle> {
        let Ok(rd) = fs::read_dir(self.root().join(layout::REPOS)) else {
            return Vec::new();
        };
        let mut out: Vec<RepoHandle> = rd
            .filter_map(Result::ok)
            .filter_map(|e| RepoKey::from_hex(e.file_name().as_encoded_bytes()))
            .filter_map(|k| self.load(&k))
            .collect();
        out.sort_by_key(|h| h.key.0);
        out
    }

    /// Moves a repository to the trash.
    ///
    /// gc's `rmtree(path)` (`gc.py:89-90`), made crash-safe (rename into `trash/` first, then remove) and
    /// `ENOENT`-tolerant (X13-CMD-07). The caller holds the exclusive lock.
    ///
    /// # Errors
    ///
    /// The I/O error of the rename or the removal.
    pub fn retire_repo(&self, key: &RepoKey) -> io::Result<()> {
        snp_os::fs::retire_dir(&self.repo_dir(key), &self.root().join(layout::TRASH))
    }

    /// Scratch/trash orphans, old restored patches, tools no `.snp-tools.json` references.
    ///
    /// snp-only maintenance run by gc under the exclusive lock (ARCHITECTURE §5.6): scratch and trash entries older
    /// than an hour; tool versions older than an hour that no environment's `.snp-tools.json` references and whose
    /// tool lock is free (nothing is removed when a sidecar is unreadable); stash patches older than 30 days that no
    /// unrestored journal names (a journal in state `restored`/`done` protects nothing; unrestored patches, their
    /// `<patch>.*` siblings and every journal are kept, and a journal that cannot be read or understood keeps every
    /// patch and is reported in [`SweepReport::unreadable_journal`], D-DATA-04); orphan repository lock files;
    /// buckets of another format version; snp 1.x leftovers behind the 1.x README signature (D-DIAG-05). Never
    /// fails: what cannot be removed stays.
    pub fn sweep(&self, now: SystemTime) -> SweepReport {
        maintenance::sweep(self.root(), &self.tools_root, now)
    }

    /// Retires the whole store.
    ///
    /// pre-commit's `clean` `rmtree`s its store (`commands/clean.py:10-16`). snp removes every entry of its own
    /// layout (snp 1.x leftovers included behind their README signature) and nothing else (D-DATA-04): unknown files
    /// a user put into `SNP_HOME` survive, every journal, the patches of unrestored journals with their `<patch>.*`
    /// siblings and every patch younger than an hour survive (recovery data and runs stashing right now,
    /// ARCHITECTURE §4), and `.lock` stays so that processes waiting on it keep excluding each other. A journal that
    /// cannot be read or understood keeps every patch; it is returned (`Ok(Some(journal))`) so the caller can say
    /// so. A missing store is `Ok(None)` and nothing is created. Never touches pre-commit's directories (D-STORE-02).
    ///
    /// # Errors
    ///
    /// The `OSError` of a removal that failed.
    pub fn clean(&self) -> Result<Option<PathBuf>, Failure> {
        if !self.exists() {
            return Ok(None);
        }
        maintenance::clean(self.root())
    }

    /// `patches/`.
    ///
    /// Where the stash guard writes `patch<secs>-<pid>` and its journals; pre-commit writes the patch into the store
    /// root (`staged_files_only.py:66-69`).
    pub fn patches_dir(&self) -> PathBuf {
        self.root().join(layout::PATCHES)
    }

    /// Output-budget spill files.
    ///
    /// `spill/` (0700): created on demand when the store exists, so `unnamed_temp` can use it (ADR-077).
    pub fn spill_dir(&self) -> PathBuf {
        let dir = self.root().join(layout::SPILL);
        if self.exists() {
            let _made: io::Result<()> = mkdir_private(&dir);
        }
        dir
    }

    /// `scratch/`.
    ///
    /// In-progress clones and extractions (same filesystem as `repos-v1`, so the final rename is atomic); the
    /// counterpart of pre-commit's `tempfile.mkdtemp(prefix='repo', dir=store)` (`store.py:162`).
    pub fn scratch_dir(&self) -> PathBuf {
        self.root().join(layout::SCRATCH)
    }

    /// `<store>/pre-commit.log` | `snp.log`.
    ///
    /// The crash log, named by brand (ADR-013); pure path computation, so the error handler never double-faults
    /// (X13-EH-01).
    pub fn crash_log(&self, brand: Brand) -> PathBuf {
        self.root().join(brand.crash_log_name())
    }

    /// `tools-v1/<tool>/<version>`.
    ///
    /// Both components are sanitised like environment versions (an unsafe name is hashed). An ephemeral store
    /// returns the main store's directory.
    pub fn tool_dir(&self, tool: &str, version: &str) -> PathBuf {
        self.tools_root
            .join(layout::TOOLS)
            .join(layout::safe_component(tool))
            .join(layout::safe_component(version))
    }

    /// `cache-v1/<name>`.
    pub fn cache_dir(&self, name: &str) -> PathBuf {
        self.tools_root
            .join(layout::CACHE)
            .join(layout::safe_component(name))
    }

    /// `store-v1.json` path differs → the old path.
    ///
    /// 11 §A.10.4: a moved store keeps its clones but its environments (absolute paths inside) are invalid; the
    /// caller prints `[INFO] snp store moved from {old} to {new}; hook environments will be reinstalled.` once. After
    /// `ensure_created` (which records the new path) this reports what it saw.
    pub fn relocated_from(&self) -> Option<PathBuf> {
        if let Some(c) = self.created.get() {
            return c.relocated_from.clone();
        }
        if self.ephemeral {
            return None;
        }
        let bytes =
            snp_os::fs::read_capped(&self.root().join(layout::STORE_FILE), marker::META_CAP)
                .ok()?;
        let old = marker::decode_store_file(&bytes)?;
        (old != self.root()).then_some(old)
    }

    // ----- private -----

    /// A not-yet-created handle on the same store (an ephemeral store's copy of its main store).
    fn handle_copy(&self) -> Self {
        Self {
            loc: self.loc.clone(),
            tools_root: self.tools_root.clone(),
            shared: self.shared.as_ref().map(|s| Box::new(s.handle_copy())),
            ephemeral: self.ephemeral,
            created: self.created.clone(),
        }
    }

    /// The store root.
    fn root(&self) -> &Path {
        &self.loc.path
    }

    /// `repos-v1/<hex>`.
    fn repo_dir(&self, key: &RepoKey) -> PathBuf {
        self.root().join(layout::REPOS).join(key.hex())
    }

    /// A complete entry: marker parses, derives `key`, checkout present.
    fn load(&self, key: &RepoKey) -> Option<RepoHandle> {
        match self.load_entry(key) {
            Load::Present(h) => Some(h),
            Load::Absent | Load::Error(..) => None,
        }
    }

    /// [`Store::load`] telling a definitively missing or broken entry from a transient I/O failure.
    fn load_entry(&self, key: &RepoKey) -> Load {
        let dir = self.repo_dir(key);
        let marker_path = dir.join(layout::REPO_MARKER);
        let bytes = match snp_os::fs::read_capped(&marker_path, marker::META_CAP) {
            Ok(b) => b,
            Err(e) if definitive(&e) => return Load::Absent,
            Err(e) => return Load::Error(e, marker_path),
        };
        let Some(meta) = marker::decode_repo_meta(&bytes) else {
            return Load::Absent;
        };
        if meta.key() != *key {
            return Load::Absent;
        }
        let src = dir.join("src");
        let (probe, checked) = match meta.kind {
            RepoKind::Remote { .. } => {
                let git = src.join(".git");
                let r = fs::symlink_metadata(&git).map(|_| true);
                (git, r)
            }
            RepoKind::Local => (src.clone(), fs::metadata(&src).map(|m| m.is_dir())),
        };
        let present = match checked {
            Ok(p) => p,
            Err(e) if definitive(&e) => false,
            Err(e) => return Load::Error(e, probe),
        };
        if !present {
            return Load::Absent;
        }
        Load::Present(RepoHandle {
            key: *key,
            envs: dir.join("envs"),
            src,
            meta,
        })
    }

    /// Fills `scratch/<rand>/{src,envs}` and writes the durable marker.
    fn build_entry(
        scratch: &Path,
        meta: &RepoMeta,
        fill: &mut dyn FnMut(&Path) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        let src = scratch.join("src");
        let envs = scratch.join("envs");
        mkdir_private(&src).map_err(|e| mkdir_failure(&src, &e))?;
        mkdir_private(&envs).map_err(|e| mkdir_failure(&envs, &e))?;
        fill(&src)?;
        fault::hit(Point::StoreMaterialize);
        let marker_path = scratch.join(layout::REPO_MARKER);
        snp_os::fs::write_new_durable(&marker_path, &marker::encode_repo_meta(meta))
            .map_err(|e| Failure::from_io(&e, Some(marker_path.as_os_str()), None))
    }

    /// The ownership policy of ADR-044/ADR-077 (D-SEC-13, D-DIAG-05).
    fn check_ownership(&self, ownership: Ownership, diag: &dyn DiagSink) -> Result<(), Failure> {
        let Ownership::Foreign { root_over_user } = ownership else {
            return Ok(());
        };
        let root = self.root();
        let uid = owner_uid(root);
        if root_over_user && !foreign_store_allowed() {
            return Err(Failure::fatal(
                "FatalError",
                messages::refuse_foreign_store_as_root(uid.unwrap_or(0), root),
            ));
        }
        if !FOREIGN_WARNED.swap(true, Ordering::AcqRel) {
            diag.diag(&messages::foreign_store_warning(uid, root));
        }
        Ok(())
    }

    /// The README of a store snp creates (`store.py:70-76`): written when the directory is new or still empty (only
    /// `.lock`, e.g. after `clean`); an existing README is never touched (it may carry the 1.x signature).
    fn write_readme_if_new(&self, existed: bool) {
        let root = self.root();
        let readme = root.join(layout::README);
        if existed && (fs::symlink_metadata(&readme).is_ok() || !only_lock_inside(root)) {
            return;
        }
        let _written: io::Result<()> =
            snp_os::fs::atomic_replace(&readme, messages::README.as_bytes(), None);
    }

    /// Records the store path in `store-v1.json`; returns the previously recorded path when it differs.
    ///
    /// A moved store's environments are invalidated first (their install states removed under each repository
    /// lock); the new path is recorded only when every invalidation succeeded, so an interrupted or failed one is
    /// repeated by the next process.
    fn track_location(&self) -> Option<PathBuf> {
        let root = self.root();
        let file = root.join(layout::STORE_FILE);
        let old = snp_os::fs::read_capped(&file, marker::META_CAP)
            .ok()
            .and_then(|b| marker::decode_store_file(&b));
        if old.as_deref() == Some(root) {
            return None;
        }
        if old.is_some() && !self.invalidate_environments() {
            return old;
        }
        let _written: io::Result<()> =
            snp_os::fs::atomic_replace(&file, &marker::encode_store_file(root), None);
        old
    }

    /// Removes every `repos-v1/<key>/envs/*/.snp-install-state.json`, each repository under its lock (installs
    /// commit their state under that lock, so none is half-written). `false` when a removal failed.
    fn invalidate_environments(&self) -> bool {
        let repos = self.root().join(layout::REPOS);
        let Ok(rd) = fs::read_dir(&repos) else {
            return true;
        };
        let mut ok = true;
        for e in rd.filter_map(Result::ok) {
            let Some(key) = RepoKey::from_hex(e.file_name().as_encoded_bytes()) else {
                continue;
            };
            let mut lock_name = key.hex();
            lock_name.push_str(".lock");
            // Not `lock_at`: this internal wait must not consume the once-per-process `Locking …` notice.
            let Ok(lock) = FileLock::acquire(
                &repos.join(lock_name),
                LockMode::Exclusive,
                &mut || {},
                &CancelToken::new(),
                None,
            ) else {
                ok = false;
                continue;
            };
            let Ok(envs) = fs::read_dir(e.path().join("envs")) else {
                continue;
            };
            for env in envs.filter_map(Result::ok) {
                match fs::remove_file(env.path().join(layout::INSTALL_STATE)) {
                    Ok(()) => {}
                    Err(err) if definitive(&err) => {}
                    Err(_) => ok = false,
                }
            }
            drop(lock);
        }
        ok
    }
}

/// What `sweep` removed.
///
/// snp-only (pre-commit's gc removes database rows only, `gc.py:61-93`); WP-33 prints the 1.x notice from it.
#[derive(Debug, Default)]
pub struct SweepReport {
    /// Orphaned scratch/trash directories.
    pub removed_dirs: usize,
    /// Old restored patches.
    pub removed_patches: usize,
    /// Whether the legacy v1 layout was removed.
    pub legacy_v1_removed: bool,
    /// A stash journal that could not be read or understood: what it protects is unknown, so no patch was removed
    /// (fail closed, D-DATA-04).
    pub unreadable_journal: Option<PathBuf>,
}

/// The outcome of reading a repository entry.
enum Load {
    /// Complete and usable.
    Present(RepoHandle),
    /// Definitively missing or broken (no marker, a malformed or foreign marker, the checkout gone).
    Absent,
    /// A transient failure (the entry may be fine and in use): the error and the path it names.
    Error(io::Error, PathBuf),
}

/// An error that proves the entry is not there (`ENOENT`, `ENOTDIR`, truncated or invalid data).
fn definitive(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory | io::ErrorKind::InvalidData
    )
}

/// `<store_root>/<bucket>/<name>`, creating the bucket (0700) but never the store root itself.
fn bucket_in_existing(store_root: &Path, bucket: &str, name: &str) -> Result<PathBuf, Failure> {
    let dir = store_root.join(bucket);
    let path = dir.join(name);
    if !store_root.is_dir() {
        return Err(Failure::from_io(
            // ENOENT (Unix) / ERROR_FILE_NOT_FOUND (Windows): both 2.
            &io::Error::from_raw_os_error(2),
            Some(path.as_os_str()),
            None,
        ));
    }
    mkdir_private(&dir).map_err(|e| mkdir_failure(&dir, &e))?;
    Ok(path)
}

/// Acquires a store lock; `on_wait` runs at most once per process.
fn lock_at(
    path: &Path,
    mode: LockMode,
    on_wait: &mut dyn FnMut(),
    c: &CancelToken,
) -> Result<FileLock, Failure> {
    let mut once = || {
        if !LOCK_NOTICE_SHOWN.swap(true, Ordering::AcqRel) {
            on_wait();
        }
    };
    FileLock::acquire(path, mode, &mut once, c, None).map_err(|e| {
        if e.kind() == io::ErrorKind::Interrupted && c.is_cancelled() {
            interrupted(c)
        } else {
            Failure::from_io(&e, Some(path.as_os_str()), None)
        }
    })
}

/// The cancellation as a `Failure`.
fn interrupted(c: &CancelToken) -> Failure {
    Failure::Interrupted(c.reason().unwrap_or(CancelReason::Interrupt))
}

/// `os.makedirs`' error: the `OSError` names the component whose `mkdir` failed (the first missing one).
fn mkdir_failure(path: &Path, e: &io::Error) -> Failure {
    let mut failing = path;
    while let Some(parent) = failing.parent() {
        if parent.as_os_str().is_empty() || fs::symlink_metadata(parent).is_ok() {
            break;
        }
        failing = parent;
    }
    Failure::from_io(e, Some(failing.as_os_str()), None)
}

/// `ENOTDIR` (`NotADirectoryError: [Errno 20] Not a directory`), as pre-commit reports a store path that is a file
/// (`open('<file>/.lock')`, X13-ST-02).
fn not_a_directory() -> io::Error {
    #[cfg(unix)]
    {
        io::Error::from_raw_os_error(rustix::io::Errno::NOTDIR.raw_os_error())
    }
    #[cfg(not(unix))]
    {
        // ERROR_DIRECTORY ("The directory name is invalid.").
        io::Error::from_raw_os_error(267)
    }
}

/// `os.access(dir, os.W_OK)` is false (`store.py:65-68`: `readonly`).
///
/// Only ever asked about an existing directory. On Windows CPython's `os.access` reports every directory writable
/// (`os_access_impl` ignores `FILE_ATTRIBUTE_READONLY` when `FILE_ATTRIBUTE_DIRECTORY` is set; `store.py` marks the
/// branch `# pragma: win32 no cover`), and Explorer sets that attribute on customised folders, so snp does the same.
fn is_readonly(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        rustix::fs::access(dir, rustix::fs::Access::WRITE_OK).is_err()
    }
    #[cfg(not(unix))]
    {
        fs::metadata(dir).is_ok_and(|m| !m.is_dir() && m.permissions().readonly())
    }
}

/// A world-writable directory without the sticky bit (Unix; never elsewhere).
fn world_writable_without_sticky(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(dir).is_ok_and(|m| {
            let mode = m.permissions().mode();
            mode & 0o002 != 0 && mode & 0o1000 == 0
        })
    }
    #[cfg(not(unix))]
    {
        let _: &Path = dir;
        false
    }
}

/// The owner uid of `dir` (Unix).
fn owner_uid(dir: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(dir).ok().map(|m| m.uid())
    }
    #[cfg(not(unix))]
    {
        let _: &Path = dir;
        None
    }
}

/// `SNP_ALLOW_FOREIGN_STORE=1` in the process environment. `ensure_created` has no `Env` parameter (frozen §1.4.16),
/// so the rare root-over-user path reads the variable through the one sanctioned reader.
fn foreign_store_allowed() -> bool {
    snp_core::env::Env::capture_process()
        .get("SNP_ALLOW_FOREIGN_STORE")
        .is_some_and(|v| v == "1")
}

/// Whether `dir` holds nothing but (possibly) `.lock`.
fn only_lock_inside(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|rd| {
        rd.filter_map(Result::ok)
            .all(|e| e.file_name() == layout::LOCK)
    })
}

#[cfg(test)]
#[cfg(windows)]
mod tests {
    use super::*;

    /// CPython's `os.access(dir, os.W_OK)` ignores `FILE_ATTRIBUTE_READONLY` on directories (Explorer sets it on
    /// customised folders), so such a store is not read-only.
    #[test]
    fn readonly_attribute_on_a_directory_is_not_readonly() {
        let d = tempfile::tempdir().unwrap();
        let mut perms = fs::metadata(d.path()).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(d.path(), perms.clone()).unwrap();
        assert!(fs::metadata(d.path()).unwrap().permissions().readonly());
        assert!(!is_readonly(d.path()));
        #[expect(
            clippy::permissions_set_readonly_false,
            reason = "Windows: clears FILE_ATTRIBUTE_READONLY so the temp dir can be removed"
        )]
        perms.set_readonly(false);
        fs::set_permissions(d.path(), perms).unwrap();
    }
}
