//! ADR-134: in Git mode (here: two process hooks on a tree that is not small) the index-stat proof replaces every
//! `git diff` it proves empty. A clean, non-racy index runs no per-hook diff (a counting probe); a hook that modifies a
//! file, a racy index and `SNP_DIFF_MODE=git` take the real diffs. Every run's output, exit code and repository equal
//! the always-diff path's, and `SNP_VERIFY_INDEX_PROOF=1` never finds a proven diff non-empty.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_git::{GitBackend, ProcBackend};
use snp_proc::spawn::{Captured, Group, PyStatus, Role, SpawnError, Stdin};

use crate::harness::{H, Out, StepClock, all, args, local, sys};

const DIFF: &str = "diff --no-ext-diff --no-textconv --ignore-submodules";

/// Counts the parity `git diff` calls.
#[derive(Debug)]
struct Counting {
    inner: ProcBackend,
    diffs: AtomicUsize,
}

impl Counting {
    fn note(&self, argv: &[OsString]) {
        let a: Vec<String> = argv
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        if a.join(" ") == DIFF {
            self.diffs.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl GitBackend for Counting {
    fn capture(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
        group: Group,
    ) -> Result<Captured, SpawnError> {
        self.note(argv);
        self.inner.capture(argv, env, cwd, stdin, group)
    }

    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        self.note(argv);
        self.inner.stream(argv, env, cwd, sink)
    }

    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError> {
        self.note(argv);
        self.inner.inherit(argv, env, cwd, stdin)
    }
}

/// 300 tracked files in 30 directories (not a small tree: Git mode for one or two process hooks, ADR-104), two
/// process hooks; `second` is the second hook's entry. With `settle`, the index is rebuilt with exact stat data and
/// every file's mtime 10 s in the past (non-racy).
fn make(second: &str, settle: bool) -> H {
    let mut h = H::new();
    h.clock = Some(Arc::new(StepClock::new(Duration::from_millis(1))));
    h.config(&local(&format!(
        "{}{}",
        sys("one", "one", "'true'", "always_run: true"),
        sys("two", "two", second, "always_run: true")
    )));
    for i in 0..300 {
        h.write(
            &format!("d{:02}/f{i:03}.txt", i % 30),
            format!("{i}\n").as_bytes(),
        );
    }
    h.git(&["add", "."]);
    h.commit("files");
    if settle {
        let past = SystemTime::now() - Duration::from_secs(10);
        let listed = h.git(&["ls-files", "-z"]).stdout;
        for p in listed.split(|&b| b == 0).filter(|p| !p.is_empty()) {
            let f = std::fs::File::options()
                .write(true)
                .open(h.path(std::str::from_utf8(p).unwrap()))
                .unwrap();
            f.set_modified(past).unwrap();
        }
        h.git(&["read-tree", "HEAD"]);
        drop(h.git_any(&["update-index", "-q", "--refresh"]));
    }
    h
}

/// Runs `a` on a fresh `make(second, settle)` with `env`; returns the outcome and the parity `git diff` calls.
fn run(second: &str, settle: bool, all_files: bool, env: &[(&str, &str)]) -> (String, usize) {
    let mut h = make(second, settle);
    for (k, v) in env {
        h.env_set(k, *v);
    }
    let rec = Arc::new(Counting {
        inner: ProcBackend {
            cancel: CancelToken::new(),
            role: Role::Tool,
        },
        diffs: AtomicUsize::new(0),
    });
    let backend: Arc<dyn GitBackend> = Arc::<Counting>::clone(&rec);
    h.backend = Some(backend);
    let o = h.run(if all_files { all() } else { args() });
    (outcome(&h, &o), rec.diffs.load(Ordering::SeqCst))
}

fn outcome(h: &H, o: &Out) -> String {
    let base = h.base.to_string_lossy().into_owned();
    let diff = h.git_any(&["diff", "--no-ext-diff", "--no-textconv"]);
    let status = h.git_any(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
    format!(
        "code {}\nstdout\n{}\nstderr\n{}\ndiff {}\nstatus {}\n",
        o.code,
        o.stdout.replace(&base, "<base>"),
        o.stderr.replace(&base, "<base>"),
        String::from_utf8_lossy(&diff.stdout),
        String::from_utf8_lossy(&status.stdout),
    )
}

/// The run equals the always-diff path and the verify mode agrees; returns the default run's `git diff` calls and the
/// always-diff path's.
fn check(second: &str, settle: bool, all_files: bool) -> (usize, usize) {
    let (got, diffs) = run(second, settle, all_files, &[]);
    let (always, always_diffs) = run(second, settle, all_files, &[("SNP_DIFF_MODE", "git")]);
    assert_eq!(got, always, "{second} {settle} {all_files}");
    let (verified, _) = run(
        second,
        settle,
        all_files,
        &[("SNP_VERIFY_INDEX_PROOF", "1")],
    );
    assert!(!verified.contains("SNP_VERIFY_INDEX_PROOF"), "{verified}");
    assert_eq!(verified, always, "{second} {settle} {all_files}");
    (diffs, always_diffs)
}

fn proof_possible() -> bool {
    let d = tempfile::tempdir().unwrap();
    snp_os::fsinfo::stat_oracle_safe(d.path()).unwrap_or(false)
}

/// A clean, non-racy index: no per-hook `git diff` (a `--all-files` run keeps step 15's before-diff, ADR-106; the
/// other run's baseline is ADR-078(3)'s, elided after a clean `diff-index`).
#[test]
fn clean_index_runs_no_per_hook_diff() {
    for all_files in [false, true] {
        let (diffs, always) = check("'true'", true, all_files);
        // the always-diff path: one after each hook, and step 15's baseline on `--all-files`
        let baseline = usize::from(all_files);
        assert_eq!(always, 2 + baseline, "all files: {all_files}");
        let want = if proof_possible() { baseline } else { always };
        assert_eq!(diffs, want, "all files: {all_files}");
    }
}

/// A hook that modifies a tracked file: the proof fails after it and the real diff reports it; a racy index never
/// proves.
#[test]
fn modifications_and_racy_indexes_take_the_real_diff() {
    let (diffs, always) = check("sh -c \"echo x >> d00/f000.txt\" --", true, true);
    assert_eq!(always, 3);
    assert_eq!(diffs, if proof_possible() { 2 } else { 3 });
    let (diffs, always) = check("'true'", false, false);
    assert_eq!((diffs, always), (2, 2), "a racy index");
}
