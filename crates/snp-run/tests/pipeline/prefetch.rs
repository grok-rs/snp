//! The pre-commit-hooks fact prefetch in the scheduler (ADR-102): it adds no git call to a run, and no snp git runs
//! while a user process hook runs (§3.4). The prefetch is joined before a sequential process hook and before a
//! priority bucket that holds one; each case runs with 4 pool permits (where a dropped join lets the delayed prefetch
//! spawn git inside the hook's window, so the test fails) and with 1 (the permit order: no deadlock, no hang). The
//! join at the end of the loop, before show-diff, `stage_fixed` and the restore, is also structural: the prefetch is a
//! scoped thread of `run_hooks`' `thread::scope`.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_git::{GitBackend, ProcBackend};
use snp_proc::spawn::{Captured, Group, PyStatus, Role, SpawnError, Stdin};
use snp_repo::StoreResolver;

use crate::harness::{H, Out, args, sys};

/// The prefetch thread's name (`sched.rs`).
const PREFETCH_THREAD: &str = "snp-prefetch";

/// One git call through the run's backend.
#[derive(Debug, Clone)]
struct Call {
    thread: String,
    argv: String,
    /// The process hook's marker existed when the call started or when it returned.
    overlapped: bool,
}

/// A Tool-role backend that records every call, delays the prefetch thread's calls (so a prefetch is still in
/// flight when the next hook starts) and notes calls made while `marker` exists (a process hook is running).
#[derive(Debug)]
struct Rec {
    inner: ProcBackend,
    marker: PathBuf,
    delay: Duration,
    calls: Mutex<Vec<Call>>,
}

impl Rec {
    fn new(marker: PathBuf, delay: Duration) -> Self {
        Self {
            inner: ProcBackend {
                cancel: CancelToken::new(),
                role: Role::Tool,
            },
            marker,
            delay,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn around<T>(&self, argv: &[OsString], call: impl FnOnce() -> T) -> T {
        let thread = std::thread::current().name().unwrap_or("").to_owned();
        if thread == PREFETCH_THREAD {
            std::thread::sleep(self.delay);
        }
        let before = self.marker.exists();
        let r = call();
        let after = self.marker.exists();
        self.calls.lock().unwrap().push(Call {
            thread,
            argv: argv
                .iter()
                .skip(1)
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" "),
            overlapped: before || after,
        });
        r
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl GitBackend for Rec {
    fn capture(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
        group: Group,
    ) -> Result<Captured, SpawnError> {
        self.around(argv, || self.inner.capture(argv, env, cwd, stdin, group))
    }

    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        self.around(argv, || self.inner.stream(argv, env, cwd, sink))
    }

    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError> {
        self.around(argv, || self.inner.inherit(argv, env, cwd, stdin))
    }
}

/// The pre-commit-hooks repository at a profiled tag: its hooks take the native fast path (no clone, no network).
fn pch(hooks: &str) -> String {
    format!(
        "-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v6.0.0\n    hooks:\n{hooks}"
    )
}

/// A test repository with the real resolver, hermetic (`GIT_ALLOW_PROTOCOL=file`: a regressed fast path that tries
/// to clone fails at once instead of reaching the network).
fn repo(jobs: usize) -> H {
    let mut h = H::new();
    h.jobs = jobs;
    h.resolver = Some(Arc::new(StoreResolver::default()));
    h.env_set("GIT_ALLOW_PROTOCOL", "file");
    h
}

/// Runs `run` (staged files) on another thread: a hang fails the test instead of blocking the suite.
fn run_bounded(h: H) -> (H, Out) {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .spawn(move || {
            let o = h.run(args());
            let _sent = tx.send((h, o));
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(120))
        .expect("the run hung (prefetch join / pool permit deadlock?)")
}

/// Stages 300 files: the hooks before the process hook take long enough for the prefetch thread to start fetching.
fn stage_files(h: &H) {
    for i in 0..300 {
        h.write(&format!("src/f{i:03}.txt"), b"a\n");
    }
    h.stage("src");
}

/// A process hook that holds `marker` for 1 s (the window in which no snp git may run).
fn marker_hook(marker: &Path, extra: &str) -> String {
    sys(
        "proc",
        "proc",
        &format!(
            "sh -c 'touch {m}; sleep 1; rm -f {m}' --",
            m = marker.display()
        ),
        &format!("always_run: true\npass_filenames: false\n{extra}"),
    )
}

/// Asserts the run passed, the prefetch ran (with more than one permit: with one, it may be stopped before its first
/// fetch, as it should), and no git call overlapped the process hook.
fn assert_no_overlap(jobs: usize, o: &Out, rec: &Rec) {
    let what = format!("SNP_JOBS={jobs}");
    assert_eq!(o.code, 0, "{what}: {}{}", o.stdout, o.stderr);
    assert!(o.stdout.contains("proc....") && o.stdout.contains("Passed"));
    let calls = rec.calls();
    assert!(
        jobs == 1 || calls.iter().any(|c| c.thread == PREFETCH_THREAD),
        "{what}: the prefetch never ran, the test proves nothing: {calls:#?}"
    );
    let bad: Vec<String> = calls
        .iter()
        .filter(|c| c.overlapped)
        .map(|c| format!("[{}] git {}", c.thread, c.argv))
        .collect();
    assert!(
        bad.is_empty(),
        "{what}: snp git ran beside a user process hook (§3.4): {bad:#?}"
    );
}

/// Natives (a consumer whose facts nobody ends up asking for, so the delayed prefetch is still in flight), then a
/// `language: system` hook, then natives: the prefetch is joined before the process hook starts.
#[test]
fn prefetch_is_joined_before_a_sequential_process_hook() {
    for jobs in [1, 4] {
        let mut h = repo(jobs);
        let marker = h.base.join("proc.running");
        let rec = Arc::new(Rec::new(marker.clone(), Duration::from_millis(250)));
        let backend: Arc<dyn GitBackend> = Arc::<Rec>::clone(&rec);
        h.backend = Some(backend);
        h.config(&format!(
            "repos:\n{}-   repo: local\n    hooks:\n{}{}",
            pch("    -   id: trailing-whitespace\n    -   id: check-case-conflict\n        files: ^nomatch$\n"),
            marker_hook(&marker, ""),
            pch("    -   id: end-of-file-fixer\n    -   id: check-merge-conflict\n"),
        ));
        stage_files(&h);
        let (_h, o) = run_bounded(h);
        assert_no_overlap(jobs, &o, &rec);
    }
}

/// The same with the process hook in a priority bucket beside a native: the prefetch is joined before the mixed
/// bucket starts.
#[test]
fn prefetch_is_joined_before_a_mixed_priority_bucket() {
    for jobs in [1, 4] {
        let mut h = repo(jobs);
        let marker = h.base.join("proc.running");
        let rec = Arc::new(Rec::new(marker.clone(), Duration::from_millis(250)));
        let backend: Arc<dyn GitBackend> = Arc::<Rec>::clone(&rec);
        h.backend = Some(backend);
        h.config(&format!(
            "repos:\n{}-   repo: local\n    hooks:\n{}{}",
            pch(
                "    -   id: trailing-whitespace\n        priority: 0\n    -   id: check-case-conflict\n        \
                 files: ^nomatch$\n        priority: 1\n"
            ),
            marker_hook(&marker, "priority: 2"),
            pch("    -   id: end-of-file-fixer\n        priority: 2\n"),
        ));
        stage_files(&h);
        let (_h, o) = run_bounded(h);
        assert_no_overlap(jobs, &o, &rec);
    }
}

/// The git argv list of a run (`GIT_TRACE`): every git process, Critical (stash) and Tool alike, in a stable order
/// with the temp dir replaced by `<base>`.
fn traced_git(trace: &Path, base: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(trace).unwrap_or_default();
    let base = base.display().to_string();
    let mut v: Vec<String> = text
        .lines()
        .filter_map(|l| l.split_once("trace: built-in: git ").map(|(_, a)| a))
        .map(|a| {
            a.replace(&base, "<base>")
                .split(' ')
                .map(|w| {
                    if w.len() == 40 && w.bytes().all(|b| b.is_ascii_hexdigit()) {
                        "<oid>"
                    } else {
                        w
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    v.sort();
    v
}

/// A single-partition `check-added-large-files` commit (the upstream variant, whose `added` fact is prefetched)
/// spawns exactly the git calls it spawned before the prefetch existed: the `added` query once, nothing extra.
#[test]
fn prefetch_adds_no_git_call_to_a_check_added_large_files_commit() {
    for jobs in [1, 4] {
        let mut h = repo(jobs);
        h.config(&format!(
            "repos:\n{}",
            pch("    -   id: check-added-large-files\n")
        ));
        h.write("a.txt", b"a\n");
        h.stage("a.txt");
        let trace = h.base.join("git.trace");
        h.env_set("GIT_TRACE", trace.as_os_str());
        let base = h.base.clone();
        let (env, root) = (h.env.clone(), h.root.clone());
        let (_h, o) = run_bounded(h);
        assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
        assert!(
            o.stdout.contains("check for added large files"),
            "{}",
            o.stdout
        );
        let mut got = traced_git(&trace, &base);
        // the baseline `git diff` of ADR-078's cold path depends on wall time (the 250 ms warm window, which a loaded
        // test machine misses), not on the prefetch
        got.retain(|a| a != "diff --no-ext-diff --no-textconv --ignore-submodules");
        // on every git: the prefetched `added` query runs once (the hook's own call is answered from the cache)
        let added = got
            .iter()
            .filter(|a| *a == "diff --staged --name-only --diff-filter=A")
            .count();
        assert_eq!(added, 1, "SNP_JOBS={jobs}: {got:#?}");
        // the exact list holds for the git range it was taken in (GitCaps changes other calls outside it)
        if exact_list_applies(&env, &root) {
            assert_eq!(got, EXPECTED_CALF_GIT, "SNP_JOBS={jobs}");
        }
    }
}

/// Whether the git on the test PATH has the capabilities [`EXPECTED_CALF_GIT`] was recorded with (2.31 ≤ git <
/// 2.54: one `rev-parse --path-format=absolute`, no config-based hooks).
fn exact_list_applies(env: &[(String, OsString)], cwd: &Path) -> bool {
    let env = Env::from_pairs(env.iter().cloned());
    snp_git::GitExe::resolve(&env, cwd, None)
        .is_ok_and(|g| g.caps.version.is_some() && g.caps.path_format && !g.caps.config_hooks)
}

/// The sorted `GIT_TRACE` argv list of `snp run` on that commit, taken from a 7692989 build (no prefetch) at
/// `SNP_JOBS=1` and `4`; identical for this branch's binary (see
/// [`prefetch_adds_no_git_call_to_a_check_added_large_files_commit`]).
const EXPECTED_CALF_GIT: &[&str] = &[
    "check-attr filter -z --stdin",
    "diff --no-renames --no-ext-diff --ignore-submodules --diff-filter=A --name-only -z",
    "diff --quiet --no-ext-diff .pre-commit-config.yaml",
    "diff --staged --name-only --diff-filter=A",
    "diff --staged --name-only --no-ext-diff -z --diff-filter=ACMRTUXB --no-renames",
    "diff-index --ignore-submodules --binary --exit-code --no-color --no-ext-diff <oid> --",
    "ls-files --unmerged",
    "rev-parse --show-cdup --is-inside-git-dir --path-format=absolute --git-dir --git-common-dir --git-path index",
    "version",
    "write-tree",
];
