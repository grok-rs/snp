//! Journal recovery of runs that never restored (§4.6, ADR-067).
//!
//! snp-only (pre-commit keeps no journal: a killed run leaves `patch<ts>-<pid>` in the store and says nothing,
//! D-DATA-13). Liveness is the worktree lock, never a pid. Nothing here applies a patch: the user decides.
//!
//! Owner: WP-23.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use snp_core::failure::Failure;
use snp_core::path::RepoPath;
use snp_git::Git;

use crate::journal::{self, JournalData, PointerData};
use crate::{lock, messages};

/// A journaled run that never restored (§4.6). Recovery hints use a plain, worktree-only `git apply
/// --whitespace=nowarn` (ADR-084: all-or-nothing, so it never overwrites what changed since the interrupted run).
#[derive(Debug, Clone)]
pub struct Unrestored {
    /// The saved patch.
    pub patch: PathBuf,
    /// The `write-tree` of the index at stash time.
    pub tree: Option<String>,
    /// The worktree it belongs to.
    pub worktree: PathBuf,
    /// The pid that wrote the journal (informational; liveness is the worktree lock).
    pub pid: u32,
    /// The last journaled state.
    pub state: Box<str>,
    /// Paths staged by `stage_fixed` before the run died.
    pub staged_fixed: Vec<RepoPath>,
}

impl Unrestored {
    /// The exact stderr notice (§4.6, ADR-084).
    ///
    /// `snp: an earlier run was interrupted before it restored your unstaged changes; they are saved in <patch> (index
    /// tree <tree>). Restore with: git apply --whitespace=nowarn <patch>`: the apply changes the worktree only and is
    /// all-or-nothing, so it never overwrites what changed since the interrupted run. With the run's own index, `&& git
    /// --literal-pathspecs reset -q <tree> -- <staged paths…>` unstages what `stage_fixed` had staged; the
    /// intent-to-add paths the run cleared are re-added (`&& git --literal-pathspecs add --intent-to-add -- …`, or
    /// `--pathspec-from-file=<patch>.ita` for a long list, kept next to the patch). Then what to do when the apply
    /// refuses: keep a copy of the current files (`git diff --binary <tree> > <patch>.now`), put the worktree copies of
    /// the patch's files back to the stash-time tree (`git restore --source=<tree> --worktree`), run it again.
    ///
    /// The patch's paths are read from the patch (`diff --git` headers) and the intent-to-add list and the index file
    /// from the store journal next to it, so the text is complete for any `Unrestored`.
    pub fn hint(&self) -> String {
        let paths = patch_paths(&self.patch);
        let journal = journal_for(&self.patch);
        let staged: Vec<Vec<u8>> = self
            .staged_fixed
            .iter()
            .map(|r| r.as_bytes().to_vec())
            .collect();
        let ita = journal.as_ref().map_or(&[][..], |(_, d)| d.ita.as_slice());
        let ita_file = kept_ita_file(&self.patch, ita);
        let own_index = journal.as_ref().is_none_or(|(_, d)| uses_own_index(d));
        messages::recovery_notice(&messages::Recovery {
            patch: &self.patch,
            tree: self.tree.as_deref(),
            patch_paths: paths.as_deref(),
            staged_fixed: &staged,
            own_index,
            ita,
            ita_file: ita_file.as_deref(),
        })
    }
}

/// Whether the journaled run used the worktree's own index (`--git-path index` is `<git-dir>/index`), not a
/// temporary `GIT_INDEX_FILE` (`git commit -- <paths>`): the hint must not reset entries of the real index to a
/// temporary index's tree.
fn uses_own_index(d: &JournalData) -> bool {
    d.index_file.as_os_str().is_empty()
        || d.git_dir.as_os_str().is_empty()
        || same_path(&d.index_file, &d.git_dir.join("index"))
}

/// The kept list of a long intent-to-add list (ADR-084): `<base>.ita`, NUL-separated (for `git add
/// --pathspec-file-nul --pathspec-from-file`), written when missing or different. It is never deleted with a journal,
/// so a notice that names it stays true; next to a patch, gc removes it with the patch (both start with `patch`).
/// `None` for a short list (spelled out) or when it cannot be written (the notice then spells out the whole list).
pub(crate) fn kept_ita_file(base: &Path, ita: &[Vec<u8>]) -> Option<PathBuf> {
    let list: Vec<&[u8]> = ita.iter().map(Vec::as_slice).collect();
    if ita.is_empty() || messages::short_list(&list) {
        return None;
    }
    let mut name = base.as_os_str().to_owned();
    name.push(".ita");
    let file = PathBuf::from(name);
    let mut body = Vec::new();
    for p in ita {
        body.extend_from_slice(p);
        body.push(0);
    }
    if std::fs::read(&file).is_ok_and(|b| b == body) {
        return Some(file);
    }
    let tmp = {
        let mut t = file.as_os_str().to_owned();
        t.push(format!(".tmp{}", std::process::id()));
        PathBuf::from(t)
    };
    // synced through the handle that wrote it: Windows' FlushFileBuffers refuses a read-only handle (EACCES), which
    // made every long list fall back to the spelled-out form there
    let written = std::fs::File::create(&tmp)
        .and_then(|mut f| {
            std::io::Write::write_all(&mut f, &body)?;
            f.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, &file));
    if written.is_err() {
        let _ignored = std::fs::remove_file(&tmp);
        return None;
    }
    Some(file)
}

/// The store journal of `patch` (same directory, `data.patch == patch`).
fn journal_for(patch: &Path) -> Option<(PathBuf, JournalData)> {
    let dir = patch.parent()?;
    store_journals(dir).into_iter().find_map(|j| {
        let data = journal::read_small(&j).and_then(|b| JournalData::parse(&b))?;
        (data.patch.as_deref() == Some(patch)).then_some((j, data))
    })
}

/// The paths a `git diff-index --binary` patch touches, from its `diff --git a/<p> b/<p>` headers (C-quoted names
/// included; no renames: plumbing never detects them), each with whether the patch creates it (`new file mode`: not
/// in the stash-time tree). Stops after [`messages::LIST_MAX_PATHS`] + 1 paths (the hint then uses the whole-index
/// form). `None` when the patch is unreadable or a header does not parse.
pub(crate) fn patch_paths(patch: &Path) -> Option<Vec<(Vec<u8>, bool)>> {
    use std::io::BufRead;
    let f = std::fs::File::open(patch).ok()?;
    let mut r = std::io::BufReader::new(f);
    let mut out: Vec<(Vec<u8>, bool)> = Vec::new();
    let mut line = Vec::new();
    // Inside the extended header lines of the last `diff --git` (until `---`, `Binary`, `GIT binary patch` or `@@`).
    let mut in_header = false;
    loop {
        line.clear();
        if r.read_until(b'\n', &mut line).ok()? == 0 {
            break;
        }
        if let Some(rest) = line.strip_prefix(b"diff --git ") {
            if out.len() > messages::LIST_MAX_PATHS {
                break;
            }
            let rest = rest.strip_suffix(b"\n").unwrap_or(rest);
            out.push((diff_git_name(rest)?, false));
            in_header = true;
        } else if in_header {
            if line.starts_with(b"new file mode ") {
                if let Some(last) = out.last_mut() {
                    last.1 = true;
                }
            } else if !(line.starts_with(b"old mode ")
                || line.starts_with(b"new mode ")
                || line.starts_with(b"deleted file mode ")
                || line.starts_with(b"index "))
            {
                in_header = false;
            }
        }
    }
    Some(out)
}

/// `a/<p> b/<p>` (both sides equal: no renames) or the C-quoted form `"a/<p>" "b/<p>"` → `<p>`.
fn diff_git_name(rest: &[u8]) -> Option<Vec<u8>> {
    if rest.first() == Some(&b'"') {
        let (a, used) = c_unquote(rest)?;
        let b_side = rest.get(used..)?.strip_prefix(b" ")?;
        let b = if b_side.first() == Some(&b'"') {
            c_unquote(b_side)?.0
        } else {
            b_side.to_vec()
        };
        let a = a.strip_prefix(b"a/")?.to_vec();
        return (b.strip_prefix(b"b/")? == a.as_slice()).then_some(a);
    }
    // Unquoted: `a/` + p + ` b/` + p, so p has (len - 5) / 2 bytes.
    let n = rest.len().checked_sub(5)?;
    if n % 2 != 0 {
        return None;
    }
    let half = n / 2;
    let a = rest.get(2..half.checked_add(2)?)?;
    let b = rest.get(half.checked_add(5)?..)?;
    (rest.starts_with(b"a/")
        && rest.get(half.checked_add(2)?..half.checked_add(5)?)? == b" b/"
        && a == b)
        .then(|| a.to_vec())
}

/// git's `unquote_c_style`: the quoted string at the start of `s` → (bytes, bytes consumed).
fn c_unquote(quoted: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut pos = 1_usize;
    loop {
        let byte = *quoted.get(pos)?;
        pos = pos.checked_add(1)?;
        match byte {
            b'"' => return Some((out, pos)),
            b'\\' => {
                let esc = *quoted.get(pos)?;
                pos = pos.checked_add(1)?;
                let value = match esc {
                    b'a' => 0x07,
                    b'b' => 0x08,
                    b'f' => 0x0c,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'v' => 0x0b,
                    b'\\' | b'"' => esc,
                    b'0'..=b'3' => {
                        let mid = *quoted.get(pos)?;
                        let low = *quoted.get(pos.checked_add(1)?)?;
                        if !(b'0'..=b'7').contains(&mid) || !(b'0'..=b'7').contains(&low) {
                            return None;
                        }
                        pos = pos.checked_add(2)?;
                        esc.wrapping_sub(b'0')
                            .wrapping_mul(64)
                            .wrapping_add(mid.wrapping_sub(b'0').wrapping_mul(8))
                            .wrapping_add(low.wrapping_sub(b'0'))
                    }
                    _ => return None,
                };
                out.push(value);
            }
            _ => out.push(byte),
        }
    }
}

/// What a scan found: orphaned runs, and files that only describe finished or never-stashed runs.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    /// Runs whose patch was never restored.
    pub(crate) unrestored: Vec<Orphan>,
    /// Journals and pointers of this worktree that protect nothing (restored, or no patch): safe to delete while
    /// holding the lock.
    pub(crate) stale: Vec<PathBuf>,
    /// Runs with no unrestored patch whose intent-to-add re-add may not have run. Reported once (no unstaged
    /// content is at risk), then deleted.
    pub(crate) ita_only: Vec<ItaOnly>,
}

/// A run whose patch was never restored, with the files that journal it.
#[derive(Debug)]
pub(crate) struct Orphan {
    /// What the notice describes.
    pub(crate) unrestored: Unrestored,
    /// The intent-to-add paths the run cleared and the file that lists them.
    pub(crate) ita: (Vec<Vec<u8>>, Option<PathBuf>),
    /// The run's journal and pointer files (never the patch).
    pub(crate) files: Vec<PathBuf>,
}

impl Orphan {
    /// The intent-to-add notice for when the run's journal is retired, naming a list file that is kept (ADR-084).
    fn ita_notice(&self) -> Option<String> {
        let (ita, _) = &self.ita;
        if ita.is_empty() {
            return None;
        }
        let kept = kept_ita_file(&self.unrestored.patch, ita);
        Some(messages::ita_notice(ita, kept.as_deref()))
    }
}

/// A run whose only leftover is its cleared intent-to-add list.
#[derive(Debug)]
pub(crate) struct ItaOnly {
    /// The intent-to-add paths.
    pub(crate) paths: Vec<Vec<u8>>,
    /// The file that lists them (store journal, else pointer).
    pub(crate) listed: Option<PathBuf>,
    /// Every file of the run (deleted after the notice).
    pub(crate) files: Vec<PathBuf>,
}

/// The pointers in a git dir: `snp-stash.journal` and `snp-stash-<n>.journal`.
fn pointers(git_dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(git_dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n == journal::POINTER_NAME
                || n.strip_prefix("snp-stash-")
                    .and_then(|r| r.strip_suffix(".journal"))
                    .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// The `*.snp-journal` files of a patches directory.
fn store_journals(patches_dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(patches_dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with("run-") && n.ends_with(journal::JOURNAL_SUFFIX)
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// One run as recorded by its journal and/or pointer.
#[derive(Debug, Default)]
struct Run {
    data: Option<JournalData>,
    pointer: Option<PointerData>,
    files: Vec<PathBuf>,
}

impl Run {
    fn state(&self) -> String {
        // The journal is written before the pointer: it is the newer record when both exist.
        self.data
            .as_ref()
            .map(|d| d.state.clone())
            .or_else(|| self.pointer.as_ref().map(|p| p.state.clone()))
            .unwrap_or_default()
    }

    fn patch(&self) -> Option<PathBuf> {
        self.data
            .as_ref()
            .and_then(|d| d.patch.clone())
            .or_else(|| self.pointer.as_ref().and_then(|p| p.patch.clone()))
    }

    fn tree(&self) -> Option<String> {
        self.data
            .as_ref()
            .and_then(|d| d.tree.clone())
            .or_else(|| self.pointer.as_ref().and_then(|p| p.tree.clone()))
    }

    fn staged_fixed(&self) -> Vec<RepoPath> {
        let raw: &[Vec<u8>] = match (&self.data, &self.pointer) {
            (Some(d), _) if !d.staged_fixed.is_empty() => &d.staged_fixed,
            (_, Some(p)) => &p.staged_fixed,
            (Some(d), None) => &d.staged_fixed,
            (None, None) => &[],
        };
        raw.iter().filter_map(|b| RepoPath::new(b)).collect()
    }

    /// The intent-to-add paths and the file that lists them (the store journal, else the pointer).
    fn ita(&self) -> (Vec<Vec<u8>>, Option<PathBuf>) {
        let store = || self.files.iter().find(|f| is_store_journal(f)).cloned();
        match (&self.data, &self.pointer) {
            (Some(d), Some(p)) if d.ita.is_empty() && !p.ita.is_empty() => (
                p.ita.clone(),
                self.files.iter().find(|f| !is_store_journal(f)).cloned(),
            ),
            (Some(d), _) => (d.ita.clone(), store()),
            (None, Some(p)) => (
                p.ita.clone(),
                self.files.iter().find(|f| !is_store_journal(f)).cloned(),
            ),
            (None, None) => (Vec::new(), None),
        }
    }
}

/// `run-*.snp-journal` (a store journal, not a git-dir pointer).
fn is_store_journal(p: &Path) -> bool {
    p.file_name()
        .is_some_and(|n| n.to_string_lossy().ends_with(journal::JOURNAL_SUFFIX))
}

/// Same path, compared literally (both sides come from the same discovery).
fn same_path(a: &Path, b: &Path) -> bool {
    a == b || a.as_os_str() == b.as_os_str()
}

/// Reads every pointer of `git_dir` and every journal of `patches_dir` that belongs to `worktree_root`.
pub(crate) fn scan(git_dir: &Path, patches_dir: &Path, worktree_root: &Path) -> Scan {
    // Keyed by the journal path (or the pointer path for a store-less run).
    let mut runs: BTreeMap<PathBuf, Run> = BTreeMap::new();
    for p in pointers(git_dir) {
        let Some(ptr) = journal::read_small(&p).and_then(|b| PointerData::parse(&b)) else {
            continue;
        };
        if let Some(w) = &ptr.worktree
            && !same_path(w, worktree_root)
        {
            continue;
        }
        let key = ptr.journal.clone().unwrap_or_else(|| p.clone());
        let data = ptr
            .journal
            .as_deref()
            .and_then(journal::read_small)
            .and_then(|b| JournalData::parse(&b));
        let run = runs.entry(key).or_default();
        run.files.push(p);
        if let Some(j) = &ptr.journal
            && data.is_some()
        {
            run.files.push(j.clone());
        }
        if run.data.is_none() {
            run.data = data;
        }
        run.pointer = Some(ptr);
    }
    for j in store_journals(patches_dir) {
        let Some(data) = journal::read_small(&j).and_then(|b| JournalData::parse(&b)) else {
            continue;
        };
        if !same_path(&data.worktree, worktree_root) {
            continue;
        }
        let run = runs.entry(j.clone()).or_default();
        if !run.files.contains(&j) {
            run.files.push(j);
        }
        if run.data.is_none() {
            run.data = Some(data);
        }
    }
    let mut out = Scan::default();
    for run in runs.into_values() {
        let state = run.state();
        let patch = run.patch().filter(|p| p.is_file());
        match patch {
            Some(patch) if journal::is_unrestored(&state) => out.unrestored.push(Orphan {
                unrestored: Unrestored {
                    patch,
                    tree: run.tree(),
                    worktree: worktree_root.to_path_buf(),
                    pid: run.data.as_ref().map_or(0, |d| d.pid),
                    staged_fixed: run.staged_fixed(),
                    state: state.into_boxed_str(),
                },
                ita: run.ita(),
                files: run.files,
            }),
            _ => {
                // The journal is removed only after the intent-to-add re-add: a leftover one with a list (state
                // `started`, or a kill after `restored`) means the re-add may not have run.
                let (ita, listed) = run.ita();
                if ita.is_empty() {
                    out.stale.extend(run.files);
                } else {
                    out.ita_only.push(ItaOnly {
                        paths: ita,
                        listed,
                        files: run.files,
                    });
                }
            }
        }
    }
    out.unrestored
        .sort_by(|a, b| a.unrestored.patch.cmp(&b.unrestored.patch));
    out
}

/// Liveness is the worktree lock, never a pid (ADR-067): call only while holding `<git_dir>/snp-stash.lock` (or
/// after a successful non-blocking probe of it). Reads the git-dir pointer `<git_dir>/snp-stash.journal` (works with
/// any store) and the journals in `patches_dir` for this worktree. Never auto-applied.
///
/// Every journal whose last state is `stashed`, `staging-fixed`, `rolling-back` or `failed` and whose patch still
/// exists is reported, ordered by patch path; unreadable or foreign files are ignored.
pub fn find_unrestored(
    git_dir: &Path,
    patches_dir: &Path,
    worktree_root: &Path,
) -> Vec<Unrestored> {
    scan(git_dir, patches_dir, worktree_root)
        .unrestored
        .into_iter()
        .map(|o| o.unrestored)
        .collect()
}

/// Retires an orphaned run: its journal and pointer files are removed, so no notice names it again. The patch is
/// kept and its modification time reset, so `gc`'s 30-day retention of unjournaled patches (D-DATA-04) counts from
/// the retirement, not from the interrupted run.
fn retire(o: &Orphan) {
    for f in &o.files {
        let _ignored = std::fs::remove_file(f);
    }
    if let Ok(f) = std::fs::File::options()
        .write(true)
        .open(&o.unrestored.patch)
    {
        let _ignored = f.set_modified(std::time::SystemTime::now());
    }
}

/// One recovery pass with the worktree lock held (ARCHITECTURE §4.6, ADR-083): the stderr notices, in order.
///
/// Stale journals and pointers are removed. An orphaned run whose patch is already applied to the worktree (`git apply
/// --check --reverse`: the user followed the notice, or restored the changes another way) is retired with a one-time
/// notice; every other orphan gets its recovery notice, followed once by how to dismiss it. A run with only a cleared
/// intent-to-add list (or a retired one with such a list) gets the intent-to-add notice once.
pub(crate) fn report_locked(
    git: &Git,
    git_dir: &Path,
    patches_dir: &Path,
    worktree_root: &Path,
) -> Vec<String> {
    let scan = scan(git_dir, patches_dir, worktree_root);
    for stale in &scan.stale {
        let _ignored = std::fs::remove_file(stale);
    }
    let mut out = Vec::new();
    let mut pending = false;
    for o in &scan.unrestored {
        let patch = &o.unrestored.patch;
        if matches!(verify_restored(git, patch), Ok(true)) {
            out.push(messages::retired_notice(patch));
            if let Some(notice) = o.ita_notice() {
                out.push(notice);
            }
            retire(o);
        } else {
            out.push(o.unrestored.hint());
            pending = true;
        }
    }
    if pending {
        out.push(messages::DISMISS_HINT.to_owned());
    }
    for run in &scan.ita_only {
        // The list outlives the journal deleted below (ADR-084).
        let kept = run
            .listed
            .as_deref()
            .and_then(|l| kept_ita_file(l, &run.paths));
        out.push(messages::ita_notice(&run.paths, kept.as_deref()));
        for f in &run.files {
            let _ignored = std::fs::remove_file(f);
        }
    }
    out
}

/// Non-blocking: try the worktree lock, run `report_locked`, release (`run`/`hook-impl` start, ARCHITECTURE
/// §4.6). `None` when another run holds the lock (it owns the journals it is writing; a stash entered later reports
/// once it holds the lock). The lock token is not written: the probe never exempts a nested run.
pub fn recovery_report(
    git: &Git,
    git_dir: &Path,
    patches_dir: &Path,
    worktree_root: &Path,
) -> Option<Vec<String>> {
    let held = lock::try_probe(git_dir)?;
    let notices = report_locked(git, git_dir, patches_dir, worktree_root);
    drop(held);
    Some(notices)
}

/// An orphaned run that [`dismiss_unrestored`] retired.
#[derive(Debug, Clone)]
pub struct Dismissed {
    /// The kept patch.
    pub patch: PathBuf,
    /// Whether its changes were already present in the worktree.
    pub present: bool,
    /// The intent-to-add notice of the run, when it had cleared intent-to-add entries.
    pub ita_notice: Option<String>,
}

/// `snp doctor --fix --stash` (ARCHITECTURE §4.6, ADR-083): the explicit dismissal. With the worktree lock probed
/// (empty when another run holds it), every orphaned run of this worktree is retired — its recovery notice stops —
/// whether or not its changes are back; the patch is never deleted (the caller prints where it is).
pub fn dismiss_unrestored(
    git: &Git,
    git_dir: &Path,
    patches_dir: &Path,
    worktree_root: &Path,
) -> Vec<Dismissed> {
    let Some(held) = lock::try_probe(git_dir) else {
        return Vec::new();
    };
    let scan = scan(git_dir, patches_dir, worktree_root);
    let mut out = Vec::new();
    for o in &scan.unrestored {
        out.push(Dismissed {
            patch: o.unrestored.patch.clone(),
            present: matches!(verify_restored(git, &o.unrestored.patch), Ok(true)),
            ita_notice: o.ita_notice(),
        });
        retire(o);
    }
    drop(held);
    out
}

/// Non-blocking: try the worktree lock, scan, release (run/hook-impl start; skipped for nested runs and when held).
///
/// Empty when another run holds the lock (it owns the journals it is writing). The lock token is not written: the
/// probe never exempts a nested run.
pub fn recovery_scan(git_dir: &Path, patches_dir: &Path, worktree_root: &Path) -> Vec<Unrestored> {
    match lock::try_probe(git_dir) {
        Some(held) => {
            let found = find_unrestored(git_dir, patches_dir, worktree_root);
            drop(held);
            found
        }
        None => Vec::new(),
    }
}

/// `snp doctor --stash` / `SNP_STASH_VERIFY` (warn-only, 06's verify) and the retirement of orphans: whether the
/// worktree holds every change of `patch` (ADR-084).
///
/// `git -c apply.ignoreWhitespace=no apply --check --reverse --whitespace=nowarn <patch>` must succeed — the flags
/// make the match exact whatever the user's `apply.whitespace` / `apply.ignoreWhitespace` (with `fix`, git matches
/// whitespace-insensitively and would call a missing trailing-whitespace change present) — and every mode the patch
/// sets (`new mode`, `new file mode`) must be the worktree file's: git only warns about a mode that differs (`has type
/// 100644, expected 100755`) and exits 0, so a lost `chmod +x` would otherwise count as restored.
pub fn verify_restored(git: &Git, patch: &Path) -> Result<bool, Failure> {
    let c = git.run_raw(&[
        OsStr::new("-c"),
        OsStr::new("apply.ignoreWhitespace=no"),
        OsStr::new("apply"),
        OsStr::new("--check"),
        OsStr::new("--reverse"),
        OsStr::new("--whitespace=nowarn"),
        patch.as_os_str(),
    ])?;
    if c.status.0 != 0 {
        return Ok(false);
    }
    Ok(patch_modes(patch).is_some_and(|modes| {
        modes
            .iter()
            .all(|(path, mode)| mode_matches(git.root(), path, mode))
    }))
}

/// Every `(path, mode)` a patch sets: the `new mode <m>` and `new file mode <m>` lines of its extended headers (all
/// of them, not capped like [`patch_paths`]). `None` when the patch is unreadable or a header does not parse.
fn patch_modes(patch: &Path) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    use std::io::BufRead;
    let f = std::fs::File::open(patch).ok()?;
    let mut r = std::io::BufReader::new(f);
    let mut out = Vec::new();
    let mut line = Vec::new();
    let mut current: Option<Vec<u8>> = None;
    loop {
        line.clear();
        if r.read_until(b'\n', &mut line).ok()? == 0 {
            break;
        }
        let text = line.strip_suffix(b"\n").unwrap_or(&line);
        if let Some(rest) = text.strip_prefix(b"diff --git ") {
            current = Some(diff_git_name(rest)?);
        } else if let Some(path) = &current {
            if let Some(m) = text
                .strip_prefix(b"new mode ")
                .or_else(|| text.strip_prefix(b"new file mode "))
            {
                out.push((path.clone(), m.to_vec()));
            } else if !(text.starts_with(b"old mode ")
                || text.starts_with(b"deleted file mode ")
                || text.starts_with(b"index "))
            {
                current = None;
            }
        }
    }
    Some(out)
}

/// Whether the worktree file `root/path` has the git `mode` (`100644`, `100755`, `120000`; anything else, e.g. a
/// gitlink, is not checked). Non-Unix platforms do not record the executable bit: only the file type is compared.
fn mode_matches(root: &Path, path: &[u8], mode: &[u8]) -> bool {
    let Some(rel) = RepoPath::new(path).filter(RepoPath::is_contained) else {
        return false;
    };
    let Ok(md) = std::fs::symlink_metadata(root.join(rel.to_os())) else {
        return false;
    };
    match mode {
        b"120000" => md.file_type().is_symlink(),
        b"100644" | b"100755" if !md.is_dir() && !md.is_symlink() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let exec = md.permissions().mode() & 0o100 != 0;
                exec == (mode == b"100755")
            }
            #[cfg(not(unix))]
            true
        }
        b"100644" | b"100755" => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_git_names() {
        assert_eq!(diff_git_name(b"a/x b/x"), Some(b"x".to_vec()));
        assert_eq!(
            diff_git_name(b"a/d/a b c b/d/a b c"),
            Some(b"d/a b c".to_vec())
        );
        assert_eq!(diff_git_name(b"a/x b/y"), None);
        assert_eq!(diff_git_name(b"a/x b/"), None);
        assert_eq!(diff_git_name(b""), None);
        assert_eq!(
            diff_git_name(br#""a/q\"t\303\251\\n" "b/q\"t\303\251\\n""#),
            Some(b"q\"t\xc3\xa9\\n".to_vec())
        );
        assert_eq!(
            diff_git_name(br#""a/x\tz" "b/x\tz""#),
            Some(b"x\tz".to_vec())
        );
        assert_eq!(diff_git_name(br#""a/x" "b/y""#), None);
        assert_eq!(diff_git_name(br#""a/x"#), None);
        assert_eq!(diff_git_name(br#""a/\9" "b/\9""#), None);
    }

    #[test]
    fn patch_paths_marks_created_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("patch");
        std::fs::write(
            &p,
            b"diff --git a/foo b/foo\nindex 1..2 100644\n--- a/foo\n+++ b/foo\n@@ -1 +1 @@\n-a\n+b\n\
              diff --git a/new file b/new file\nnew file mode 100644\nindex 0..3\nGIT binary patch\nliteral 1\n\
              diff --git a/gone b/gone\ndeleted file mode 100644\nindex 4..0\n",
        )
        .unwrap();
        assert_eq!(
            patch_paths(&p),
            Some(vec![
                (b"foo".to_vec(), false),
                (b"new file".to_vec(), true),
                (b"gone".to_vec(), false)
            ])
        );
        assert_eq!(patch_paths(&d.path().join("missing")), None);
        std::fs::write(&p, b"diff --git a/x b/y\n").unwrap();
        assert_eq!(patch_paths(&p), None);
    }
}
