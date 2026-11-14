//! Per-run cache of the git facts pre-commit-hooks compute, with their exact upstream argv (non `-z`, quotePath and
//! renames honoured: bug-compat) and the hook env (09 §3, §9.6). Used by snp-hooks natives.
//!
//! The cached facts depend only on `HEAD` and the index, so each entry is stamped with the `(size, mtime, inode)` of
//! the index file (`$GIT_INDEX_FILE` or `<git dir>/index`) and of `<git dir>/HEAD` and recomputed when either changed
//! (hooks of other languages may `git add`; 09 §9.6 "Invalidation").
//!
//! Owner: WP-18.

use std::any::{Any, TypeId};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use snp_core::failure::{Failure, pyerr};
use snp_proc::spawn::{Captured, Group, Stdin};
use snp_pycompat::codec::decode_utf8;
use snp_pycompat::repr::{repr_str, repr_str_tuple};
use snp_pycompat::text::{splitlines, strip};

use crate::Git;
use crate::types::parse_ls_files_stage;

/// The identity of one file for cache invalidation: `(len, mtime ns, inode)`; `None` when it does not exist.
type FileStamp = Option<(u64, u128, u64)>;

fn file_stamp(p: &Path) -> FileStamp {
    let md = std::fs::metadata(p).ok()?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(&md);
    #[cfg(not(unix))]
    let ino = 0;
    Some((md.len(), mtime, ino))
}

/// What a cached fact depends on: the index and `HEAD` stamps, plus the repository config stamps for the facts whose
/// output follows `core.quotePath` / `diff.renames` (empty for the others).
type Stamp = ([FileStamp; 2], Vec<FileStamp>);
/// One cached fact and the stamp it was computed under.
type Slot<T> = Mutex<Option<(Stamp, T)>>;
/// `(mode, path)` of every index entry.
type Modes = Arc<[(u32, Vec<u8>)]>;

/// Per-run cache of pre-commit-hooks git facts.
#[derive(Debug)]
pub struct PchFacts {
    git: Git,
    index_file: PathBuf,
    head_file: PathBuf,
    added: Slot<Arc<[Vec<u8>]>>,
    tracked: Slot<Arc<[Vec<u8>]>>,
    modes: Slot<Modes>,
    branch: Slot<Option<String>>,
    lfs: Mutex<LfsState>,
    /// The successful `rev-parse --git-dir` of [`PchFacts::raw`] (`check_merge_conflict` asks once per partition;
    /// the git dir cannot move during a run).
    git_dir: Mutex<Option<GitDirAnswer>>,
    /// The answer of `config core.fileMode` with the stamps of the repository's config files
    /// (`check_executables_have_shebangs` asks once per partition; B6 spawned it 20 times for one hook).
    file_mode: Mutex<Option<(Vec<FileStamp>, GitDirAnswer)>>,
    /// Values derived from the current [`PchFacts::tracked_files`] answer ([`PchFacts::tracked_derived`]).
    derived: Mutex<Vec<Derived>>,
}

/// A value built once per `tracked_files()` answer: keyed by that answer's identity (the strong `Arc` held here, so
/// its address cannot be reused while the entry lives), the value's type and the caller's tag. The cell is cloned out
/// of the list before it is filled, so a build never runs under any lock of [`PchFacts`] and concurrent partitions
/// wait for the one build ([`OnceLock::get_or_init`]).
struct Derived {
    tracked: Arc<[Vec<u8>]>,
    key: (TypeId, u32),
    cell: Arc<OnceLock<Arc<dyn Any + Send + Sync>>>,
}

impl std::fmt::Debug for Derived {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Derived")
            .field("tracked", &self.tracked.len())
            .field("tag", &self.key.1)
            .field("built", &self.cell.get().is_some())
            .finish()
    }
}

/// A set of the per-run facts a native hook asks for, for [`PchFacts::prefetch`] (ADR-102).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FactSet(u8);

impl FactSet {
    /// No fact.
    pub const NONE: Self = Self(0);
    /// [`PchFacts::tracked_files`].
    pub const TRACKED: Self = Self(1);
    /// [`PchFacts::added_files`].
    pub const ADDED: Self = Self(1 << 1);
    /// `rev-parse --git-dir` through [`PchFacts::raw`].
    pub const GIT_DIR: Self = Self(1 << 2);
    /// `config core.fileMode` through [`PchFacts::raw`].
    pub const FILE_MODE: Self = Self(1 << 3);
    /// [`PchFacts::current_branch`].
    pub const BRANCH: Self = Self(1 << 4);

    /// Both sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Every fact of `other` is in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// No fact.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// A captured answer: `(exit status, stdout, stderr)`.
type GitDirAnswer = (i64, Vec<u8>, Vec<u8>);

/// The `check-attr filter` memo of [`PchFacts::lfs_files`] (ADR-082).
#[derive(Debug, Default)]
enum LfsState {
    /// No call yet: the first one queries its own paths (a single-partition hook costs one spawn, as upstream).
    #[default]
    Fresh,
    /// One direct call happened: the next one takes the whole index in one `check-attr`.
    Asked,
    /// The whole index, answered.
    Index(Box<LfsIndex>),
    /// The whole-index query failed (a non-UTF-8 path, a git error): every call queries its own paths, so each
    /// partition sees exactly upstream's result or error.
    Direct,
}

/// `check-attr filter` of every index path, with what it depends on.
#[derive(Debug)]
struct LfsIndex {
    /// Index and `HEAD` stamps at query time.
    stamp: [FileStamp; 2],
    /// The index paths that were asked (sorted, as `ls-files` prints them).
    paths: Modes,
    /// Those whose `filter` is `lfs`.
    lfs: std::collections::HashSet<Vec<u8>>,
    /// `<dir>/.gitattributes` of every directory holding an index path (`""` = the root), and
    /// `<git dir>/info/attributes` (key `None`).
    attrs: std::collections::HashMap<Option<Vec<u8>>, FileStamp>,
}

/// Every proper ancestor directory of a `/`-separated repository path, the root (`""`) first.
fn ancestors(p: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::once(p.get(..0).unwrap_or_default()).chain(
        p.iter()
            .enumerate()
            .filter(|(_, b)| **b == b'/')
            .filter_map(|(i, _)| p.get(..i)),
    )
}

/// `repr(bytes)` (pre-commit-hooks' `CalledProcessError` carries stderr as bytes).
fn repr_bytes(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from("b");
    out.push(quote);
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            c if char::from(c) == quote => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(char::from(c)),
            c => {
                use std::fmt::Write as _;
                let _infallible: std::fmt::Result = write!(out, "\\x{c:02x}");
            }
        }
    }
    out.push(quote);
    out
}

impl PchFacts {
    /// A cache over `git` (its env is the hook env; 09 §9.6).
    pub fn new(git: Git) -> Self {
        let root = git.root().to_path_buf();
        let index_file = git
            .env()
            .get_nonempty("GIT_INDEX_FILE")
            .map_or_else(|| git.admin_dir.join("index"), |p| root.join(p));
        let head_file = git.admin_dir.join("HEAD");
        Self {
            git,
            index_file,
            head_file,
            added: Mutex::new(None),
            tracked: Mutex::new(None),
            modes: Mutex::new(None),
            branch: Mutex::new(None),
            lfs: Mutex::new(LfsState::Fresh),
            git_dir: Mutex::new(None),
            file_mode: Mutex::new(None),
            derived: Mutex::new(Vec::new()),
        }
    }

    /// The stamps of the repository config files a `git config` read sees (beside the global and system files,
    /// which a run does not change): `config` and `config.worktree` of the git dir and of the common dir.
    fn config_stamps(&self) -> Vec<FileStamp> {
        let mut dirs = vec![self.git.admin_dir.to_path_buf()];
        if let Ok(rel) = std::fs::read_to_string(self.git.admin_dir.join("commondir")) {
            dirs.push(self.git.admin_dir.join(rel.trim_end_matches(['\n', '\r'])));
        }
        dirs.iter()
            .flat_map(|d| [d.join("config"), d.join("config.worktree")])
            .map(|p| file_stamp(&p))
            .collect()
    }

    /// The stamp of `<dir>/.gitattributes` (`dir` relative to the root) or, for `None`, of
    /// `<git dir>/info/attributes`.
    fn attr_stamp(&self, dir: Option<&[u8]>) -> FileStamp {
        match dir {
            None => file_stamp(&self.git.admin_dir.join("info").join("attributes")),
            Some(d) => {
                let base = if d.is_empty() {
                    self.git.root.to_path_buf()
                } else {
                    self.git.root.join(crate::run::os_from_bytes(d))
                };
                file_stamp(&base.join(".gitattributes"))
            }
        }
    }

    /// `check-attr filter` of every index path in one call; `None` when that call cannot stand in for the
    /// per-partition ones (any error: the partitions then reproduce it exactly).
    fn lfs_index(&self) -> Option<LfsIndex> {
        let stamp = self.stamp();
        let paths = self.index_modes().ok()?;
        let mut names: Vec<&[u8]> = paths.iter().map(|(_, p)| p.as_slice()).collect();
        names.dedup();
        let lfs = if names.is_empty() {
            std::collections::HashSet::new()
        } else {
            self.check_attr_lfs(&names).ok()?.into_iter().collect()
        };
        let mut attrs = std::collections::HashMap::new();
        attrs.insert(None, self.attr_stamp(None));
        for p in &names {
            for d in ancestors(p) {
                attrs
                    .entry(Some(d.to_vec()))
                    .or_insert_with(|| self.attr_stamp(Some(d)));
            }
        }
        Some(LfsIndex {
            stamp,
            paths,
            lfs,
            attrs,
        })
    }

    /// The answer of `ix` for `paths` (input order, repeats kept, as git prints them), or `None` when a path is
    /// not an index path or an attribute source changed since `ix` was taken.
    fn lfs_from(&self, ix: &LfsIndex, paths: &[&[u8]]) -> Option<Vec<Vec<u8>>> {
        if ix.stamp != self.stamp() || ix.attrs.get(&None) != Some(&self.attr_stamp(None)) {
            return None;
        }
        let mut checked: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
        let mut out = Vec::new();
        for p in paths {
            ix.paths
                .binary_search_by(|(_, q)| q.as_slice().cmp(p))
                .ok()?;
            for d in ancestors(p) {
                if checked.insert(d)
                    && ix.attrs.get(&Some(d.to_vec()))? != &self.attr_stamp(Some(d))
                {
                    return None;
                }
            }
            if ix.lfs.contains(*p) {
                out.push(p.to_vec());
            }
        }
        Some(out)
    }

    fn stamp(&self) -> [FileStamp; 2] {
        [file_stamp(&self.index_file), file_stamp(&self.head_file)]
    }

    /// The value of `slot` when its stamp is current, else `compute()` (stored only on success). With `config`, the
    /// stamp includes the repository config files ([`PchFacts::config_stamps`]).
    fn cached<T: Clone>(
        &self,
        slot: &Slot<T>,
        config: bool,
        compute: impl FnOnce() -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        let mut guard = snp_core::sync::lock(slot);
        let stamp = (
            self.stamp(),
            if config {
                self.config_stamps()
            } else {
                Vec::new()
            },
        );
        if let Some((s, v)) = guard.as_ref()
            && *s == stamp
        {
            return Ok(v.clone());
        }
        let v = compute()?;
        *guard = Some((stamp, v.clone()));
        Ok(v)
    }

    /// `pre_commit_hooks.util.cmd_output(*cmd)` (`util.py:16-24`): `stdout.decode()` (strict UTF-8; a
    /// `UnicodeDecodeError` comes first), then `retcode` 0 or pre-commit-hooks' own `CalledProcessError`, whose
    /// `str()` is `repr((cmd, retcode, returncode, stdout, stderr))`.
    fn cmd_output(&self, args: &[&str]) -> Result<(i64, String), Failure> {
        let os_args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        let c = self.git.run_raw(&os_args)?;
        let stdout = decode_utf8(&c.stdout)
            .map_err(|e| Failure::unexpected(e.py_type(), e.to_string()))?
            .to_owned();
        Ok((c.status.0, stdout))
    }

    fn cmd_output_checked(&self, args: &[&str]) -> Result<String, Failure> {
        let os_args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        let c = self.git.run_raw(&os_args)?;
        let stdout = decode_utf8(&c.stdout)
            .map_err(|e| Failure::unexpected(e.py_type(), e.to_string()))?
            .to_owned();
        if c.status.0 == 0 {
            return Ok(stdout);
        }
        let mut cmd = vec!["git"];
        cmd.extend_from_slice(args);
        Err(Failure::unexpected(
            "CalledProcessError",
            format!(
                "({}, 0, {}, {}, {})",
                repr_str_tuple(&cmd),
                c.status.0,
                repr_str(&stdout),
                repr_bytes(&c.stderr)
            ),
        ))
    }

    fn lines(text: &str) -> Arc<[Vec<u8>]> {
        splitlines(text).map(|l| l.as_bytes().to_vec()).collect()
    }

    /// `diff --staged --name-only --diff-filter=A` (`util.added_files`): C-quoted paths, rename detection per
    /// `diff.renames`, split with `str.splitlines()` (09 §3). Cached while the index, `HEAD` and the repository
    /// config files (`core.quotePath`, `diff.renames`) keep their stamps.
    pub fn added_files(&self) -> Result<Arc<[Vec<u8>]>, Failure> {
        self.cached(&self.added, true, || {
            let out =
                self.cmd_output_checked(&["diff", "--staged", "--name-only", "--diff-filter=A"])?;
            Ok(Self::lines(&out))
        })
    }

    /// `ls-files` (quoted, non `-z`): `check_case_conflict`'s `git ls-files` (09 §4.E). Cached while the index,
    /// `HEAD` and the repository config files (`core.quotePath`) keep their stamps.
    pub fn tracked_files(&self) -> Result<Arc<[Vec<u8>]>, Failure> {
        self.cached(&self.tracked, true, || {
            let out = self.cmd_output_checked(&["ls-files"])?;
            Ok(Self::lines(&out))
        })
    }

    /// A value of type `T` derived from `tracked` (an answer of [`PchFacts::tracked_files`]) by `build`, built once
    /// per answer and `tag` and shared by every caller (check-case-conflict's repository index, ADR-102). Entries of
    /// an older answer are dropped when a new answer is first asked for. `None` only if an entry of another type
    /// shares the key (never: the type is part of it); the caller then computes without the memo.
    pub fn tracked_derived<T: Any + Send + Sync>(
        &self,
        tracked: &Arc<[Vec<u8>]>,
        tag: u32,
        build: impl FnOnce(&[Vec<u8>]) -> T,
    ) -> Option<Arc<T>> {
        let key = (TypeId::of::<T>(), tag);
        let cell = {
            let mut list = snp_core::sync::lock(&self.derived);
            list.retain(|d| Arc::ptr_eq(&d.tracked, tracked));
            if let Some(d) = list.iter().find(|d| d.key == key) {
                Arc::clone(&d.cell)
            } else {
                let cell = Arc::new(OnceLock::new());
                list.push(Derived {
                    tracked: Arc::clone(tracked),
                    key,
                    cell: Arc::clone(&cell),
                });
                cell
            }
        };
        let any = cell.get_or_init(|| {
            let built: Arc<dyn Any + Send + Sync> = Arc::new(build(tracked));
            built
        });
        Arc::clone(any).downcast::<T>().ok()
    }

    /// Computes the facts of `set` ahead of the hooks that will ask for them (ADR-102), one git child at a time, in
    /// the order tracked, added, branch, git dir, file mode; `stop` is checked before each. Each fact goes through
    /// its own accessor, so it is cached exactly as a hook's call would cache it (failures never are) and a later
    /// call answers from the cache or recomputes under the usual stamps. Results and errors are dropped here: a hook
    /// that asks again sees exactly what its own call would have produced.
    pub fn prefetch(&self, set: FactSet, stop: &dyn Fn() -> bool) {
        /// One fetch, its result dropped.
        type Fetch = fn(&PchFacts);
        let steps: [(FactSet, Fetch); 5] = [
            (FactSet::TRACKED, |f| drop(f.tracked_files())),
            (FactSet::ADDED, |f| drop(f.added_files())),
            (FactSet::BRANCH, |f| drop(f.current_branch())),
            (FactSet::GIT_DIR, |f| {
                drop(f.raw(&[OsStr::new("rev-parse"), OsStr::new("--git-dir")]));
            }),
            (FactSet::FILE_MODE, |f| {
                drop(f.raw(&[OsStr::new("config"), OsStr::new("core.fileMode")]));
            }),
        ];
        for (fact, fetch) in steps {
            if set.contains(fact) {
                if stop() {
                    return;
                }
                fetch(self);
            }
        }
    }

    /// `ls-files --stage` of the whole index as `(mode, path)` (`git ls-files -z --stage`, the record format of the
    /// per-partition `ls-files -z --stage -- <paths>` calls; unmerged paths appear once per stage).
    #[expect(clippy::type_complexity, reason = "frozen §1.4.13 signature")]
    pub fn index_modes(&self) -> Result<Arc<[(u32, Vec<u8>)]>, Failure> {
        self.cached(&self.modes, false, || {
            let out = self.cmd_output_checked(&["ls-files", "-z", "--stage"])?;
            let entries = parse_ls_files_stage(out.into_bytes()).map_err(|r| {
                Failure::unexpected(
                    "ValueError",
                    format!(
                        "unexpected `git ls-files -z --stage` output: {}",
                        repr_bytes(&r.0)
                    ),
                )
            })?;
            Ok(entries
                .modes
                .iter()
                .zip(entries.paths.iter())
                .map(|(&m, p)| (m, p.to_vec()))
                .collect())
        })
    }

    /// `check-attr filter` (09 §4.E): `check_added_large_files.filter_lfs_files` — `git check-attr filter -z
    /// --stdin` fed `'\0'.join(paths)`, stderr discarded, `check=True` (the stdlib `CalledProcessError`, format B);
    /// the paths whose `filter` is `lfs`, in git's order. No paths: no call.
    ///
    /// Memoised (ADR-082): from the second call of a run on, one `check-attr` over every index path answers every
    /// partition whose paths are all index paths, while the index, `HEAD`, `<git dir>/info/attributes` and the
    /// `.gitattributes` of each path's directories are unchanged; anything else is asked directly, so the result
    /// and every error are upstream's.
    pub fn lfs_files(&self, paths: &[&[u8]]) -> Result<Vec<Vec<u8>>, Failure> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        {
            let mut st = snp_core::sync::lock(&self.lfs);
            if let LfsState::Index(ix) = &*st {
                if let Some(hit) = self.lfs_from(ix, paths) {
                    return Ok(hit);
                }
                // an attribute source or the index changed: take the index again, once
                *st = LfsState::Asked;
            }
            match &*st {
                LfsState::Fresh => *st = LfsState::Asked,
                LfsState::Asked => {
                    *st = self
                        .lfs_index()
                        .map_or(LfsState::Direct, |ix| LfsState::Index(Box::new(ix)));
                    if let LfsState::Index(ix) = &*st
                        && let Some(hit) = self.lfs_from(ix, paths)
                    {
                        return Ok(hit);
                    }
                }
                LfsState::Index(_) | LfsState::Direct => {}
            }
        }
        self.check_attr_lfs(paths)
    }

    /// One `git check-attr filter -z --stdin` for `paths` (upstream's call, see [`PchFacts::lfs_files`]).
    fn check_attr_lfs(&self, paths: &[&[u8]]) -> Result<Vec<Vec<u8>>, Failure> {
        let stdin: Vec<u8> = paths.join(&b'\0');
        let args = ["check-attr", "filter", "-z", "--stdin"];
        let mut argv = self.git.exe.argv0.clone();
        argv.extend(args.iter().map(Into::into));
        let c = self
            .git
            .backend
            .capture(
                &argv,
                &self.git.env,
                &self.git.root,
                Stdin::Bytes(Arc::from(stdin)),
                Group::Inherit,
            )
            .map_err(|e| Failure::from_io(&e.io, Some(OsStr::new("git")), None))?;
        // `subprocess.run(encoding='utf-8', check=True)`: `communicate()` decodes stdout before `check` looks at
        // the returncode, so undecodable output is the `UnicodeDecodeError` even when git failed
        decode_utf8(&c.stdout).map_err(|e| Failure::unexpected(e.py_type(), e.to_string()))?;
        if c.status.0 != 0 {
            let mut bare = vec![OsStr::new("git").to_os_string()];
            bare.extend(args.iter().map(Into::into));
            return Err(Failure::unexpected(
                "CalledProcessError",
                pyerr::called_process_b(&bare, c.status.0),
            ));
        }
        let fields: Vec<&[u8]> = snp_pycompat::precommit::zsplit(&c.stdout).collect();
        let mut lfs = Vec::new();
        let mut i = 0_usize;
        while let (Some(name), Some(tag)) = (fields.get(i), fields.get(i.saturating_add(2))) {
            if *tag == b"lfs" {
                lfs.push(name.to_vec());
            }
            i = i.saturating_add(3);
        }
        Ok(lfs)
    }

    /// `symbolic-ref HEAD` (`no_commit_to_branch.is_on_branch`): `None` on a non-zero exit (detached HEAD); else the
    /// branch `'/'.join(ref.strip().split('/')[2:])` (`refs/heads/feature/x` → `feature/x`).
    pub fn current_branch(&self) -> Result<Option<String>, Failure> {
        self.cached(&self.branch, false, || {
            let (rc, out) = self.cmd_output(&["symbolic-ref", "HEAD"])?;
            if rc != 0 {
                return Ok(None);
            }
            Ok(Some(
                strip(&out).split('/').skip(2).collect::<Vec<_>>().join("/"),
            ))
        })
    }

    /// `config core.fileMode`, memoised while the repository's config files keep their stamps (any exit status:
    /// unset is `1` with empty output, and stays so until a config file changes).
    fn file_mode(&self, args: &[&OsStr]) -> Result<Captured, Failure> {
        let stamps = self.config_stamps();
        let mut memo = snp_core::sync::lock(&self.file_mode);
        if let Some((at, (status, stdout, stderr))) = memo.as_ref()
            && *at == stamps
        {
            return Ok(Captured {
                status: snp_proc::spawn::PyStatus(*status),
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                stopped_by_tty: false,
            });
        }
        let c = self.git.run_raw(args)?;
        *memo = Some((stamps, (c.status.0, c.stdout.clone(), c.stderr.clone())));
        Ok(c)
    }

    /// Any other catalogued call (09 §3: `config core.fileMode`, `ls-files -z --stage -- <paths>`, `status
    /// --porcelain=v2 -z -- <paths>`, `cat-file`, `diff --diff-filter=A --raw …`): `git <args>` with the hook env,
    /// unchecked, never cached — except a successful `rev-parse --git-dir`, which cannot change during a run
    /// (`check_merge_conflict` asks it once per partition; ADR-082).
    pub fn raw(&self, args: &[&OsStr]) -> Result<Captured, Failure> {
        let git_dir = args == [OsStr::new("rev-parse"), OsStr::new("--git-dir")];
        if args == [OsStr::new("config"), OsStr::new("core.fileMode")] {
            return self.file_mode(args);
        }
        if !git_dir {
            return self.git.run_raw(args);
        }
        let mut memo = snp_core::sync::lock(&self.git_dir);
        if let Some((status, stdout, stderr)) = memo.as_ref() {
            return Ok(Captured {
                status: snp_proc::spawn::PyStatus(*status),
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                stopped_by_tty: false,
            });
        }
        let c = self.git.run_raw(args)?;
        if c.status.0 == 0 {
            *memo = Some((c.status.0, c.stdout.clone(), c.stderr.clone()));
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_repr_like_python() {
        assert_eq!(repr_bytes(b""), "b''");
        assert_eq!(repr_bytes(b"fatal: x\n"), "b'fatal: x\\n'");
        assert_eq!(repr_bytes(b"it's"), "b\"it's\"");
        assert_eq!(repr_bytes(b"'\""), "b'\\'\"'");
        assert_eq!(repr_bytes(b"\x00\xff\\\t\r"), "b'\\x00\\xff\\\\\\t\\r'");
    }
}
