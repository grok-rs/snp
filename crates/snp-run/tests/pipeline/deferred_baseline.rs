//! ADR-132: in a run whose selected, unskipped hooks all run in process (no multi-hook group, no active
//! `stage_fixed`, no baseline already known), the baseline `git diff` is taken by the first native write of a hook
//! instead of before every hook. The B6 shape (natives over a clean tree) takes no `git diff` at all (a counting
//! probe); a fixer's output, exit code and files equal the fallback's (the same run with an unskipped process hook
//! that has no files, which keeps today's position), with `SNP_JOBS` 1, 2 and 4, and a recording backend checks that
//! no file is written before the baseline diff ends; a mixed run keeps today's `GIT_TRACE` order; edits injected
//! before, inside and after a writer's baseline give the verdicts of the panel's race table
//! (docs/DECISIONS.md, ADR-132); `--show-diff-on-failure` with no writer still sees the run's diff.

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

use crate::harness::{H, Out, all, args, sys};

/// The oracle's `git diff` (the baseline and every comparison).
const DIFF: &str = "diff --no-ext-diff --no-textconv --ignore-submodules";

/// When an injected edit happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    /// Right before the first git call of the run (before any hook).
    FirstCall,
    /// Right before the first call whose arguments are these.
    Before(&'static str),
    /// Right after the first call whose arguments are these returned.
    After(&'static str),
}

/// An edit by another process (an IDE autosave), injected once.
#[derive(Debug)]
struct Edit {
    when: When,
    path: PathBuf,
    bytes: Vec<u8>,
}

/// The run's Tool-role git backend: records every call's arguments in order, holds every baseline-shaped
/// `git diff` for `hold` after checking that the watched files still hold their bytes (and checks again when it
/// ends), and injects an edit.
#[derive(Debug, Default)]
struct Rec {
    inner: Option<ProcBackend>,
    calls: Mutex<Vec<String>>,
    watched: Vec<(PathBuf, Vec<u8>)>,
    hold: Duration,
    edit: Mutex<Option<Edit>>,
    /// Per `git diff`, at its start and at its end: how many watched files were already written.
    early: Mutex<Vec<usize>>,
}

impl Rec {
    fn new(watched: Vec<(PathBuf, Vec<u8>)>, hold: Duration, edit: Option<Edit>) -> Arc<Self> {
        Arc::new(Self {
            inner: Some(ProcBackend {
                cancel: CancelToken::new(),
                role: Role::Tool,
            }),
            watched,
            hold,
            edit: Mutex::new(edit),
            ..Self::default()
        })
    }

    fn inner(&self) -> &ProcBackend {
        self.inner.as_ref().unwrap()
    }

    /// Applies the pending edit when `now` matches it.
    fn maybe_edit(&self, now: When) {
        let mut slot = self.edit.lock().unwrap();
        if slot.as_ref().is_some_and(|e| e.when == now) {
            let e = slot.take().unwrap();
            std::fs::write(&e.path, &e.bytes).unwrap();
        }
    }

    fn check_watched(&self) {
        let written = self
            .watched
            .iter()
            .filter(|(p, b)| std::fs::read(p).ok().as_ref() != Some(b))
            .count();
        self.early.lock().unwrap().push(written);
    }

    fn around<T>(&self, argv: &[OsString], call: impl FnOnce() -> T) -> T {
        let args: Vec<String> = argv
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let args = args.join(" ");
        let first = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(args.clone());
            calls.len() == 1
        };
        if first {
            self.maybe_edit(When::FirstCall);
        }
        let key: &'static str = match args.as_str() {
            DIFF => DIFF,
            "diff --staged --name-only --diff-filter=A" => {
                "diff --staged --name-only --diff-filter=A"
            }
            _ => "",
        };
        self.maybe_edit(When::Before(key));
        let diff = args == DIFF;
        if diff {
            self.check_watched();
            std::thread::sleep(self.hold);
        }
        let r = call();
        if diff {
            self.check_watched();
        }
        self.maybe_edit(When::After(key));
        r
    }

    fn diffs(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|a| *a == DIFF)
            .count()
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
        self.around(argv, || self.inner().capture(argv, env, cwd, stdin, group))
    }

    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        self.around(argv, || self.inner().stream(argv, env, cwd, sink))
    }

    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError> {
        self.around(argv, || self.inner().inherit(argv, env, cwd, stdin))
    }
}

/// pre-commit-hooks v6.0.0 hooks (the native fast path).
fn pch(hooks: &str) -> String {
    format!(
        "-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v6.0.0\n    hooks:\n{hooks}"
    )
}

/// The fallback's extra hook: an unskipped process hook that matches no file, so the static gate fails and every
/// hook keeps today's baseline position; it prints one `(no files to check)Skipped` line.
fn no_files_hook() -> String {
    format!(
        "-   repo: local\n    hooks:\n{}",
        sys("nofiles", "nofiles", "'true'", "files: ^nomatch$")
    )
}

/// A hermetic repository with the real resolver; `files` are committed.
fn repo(jobs: usize, config: &str, files: &[(&str, &[u8])]) -> H {
    let mut h = H::new();
    h.jobs = jobs;
    h.resolver = Some(Arc::new(StoreResolver::default()));
    h.env_set("GIT_ALLOW_PROTOCOL", "file");
    for (p, b) in files {
        h.write(p, b);
    }
    h.config_staged(config);
    h.stage(".");
    h.commit("init");
    h
}

/// Runs on another thread: a hang (a gate deadlock) fails the test instead of blocking the suite.
fn run_bounded(h: H, a: snp_cli::RunArgs) -> (H, Out) {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .spawn(move || {
            let o = h.run(a);
            let _sent = tx.send((h, o));
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(120))
        .expect("the run hung (write gate deadlock?)")
}

/// The bytes of `paths` after a run.
fn read_all(h: &H, paths: &[&str]) -> Vec<Option<Vec<u8>>> {
    paths
        .iter()
        .map(|p| std::fs::read(h.path(p)).ok())
        .collect()
}

/// `stdout` without the fallback hook's line.
fn without_fallback(stdout: &str) -> String {
    stdout
        .lines()
        .filter(|l| !l.starts_with("nofiles"))
        .map(|l| format!("{l}\n"))
        .collect()
}

/// The B6 shape: thirteen-ish natives over a clean tree, `--all-files`. Nothing writes, so no `git diff` runs.
#[test]
fn the_b6_shape_takes_no_git_diff() {
    let config = format!(
        "repos:\n{}",
        pch(
            "    -   id: check-yaml\n    -   id: check-json\n    -   id: check-toml\n    -   id: end-of-file-fixer\n    \
             -   id: trailing-whitespace\n    -   id: mixed-line-ending\n    -   id: check-added-large-files\n    \
             -   id: check-merge-conflict\n    -   id: detect-private-key\n    \
             -   id: check-executables-have-shebangs\n    -   id: check-case-conflict\n    \
             -   id: check-symlinks\n    -   id: fix-byte-order-marker\n"
        )
    );
    let files: Vec<(String, Vec<u8>)> = (0..40)
        .map(|i| (format!("src/f{i:02}.txt"), b"clean\n".to_vec()))
        .chain([
            ("a.yaml".to_owned(), b"a: 1\n".to_vec()),
            ("b.json".to_owned(), b"{\"a\": 1}\n".to_vec()),
        ])
        .collect();
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    for jobs in [1, 4] {
        let mut h = repo(jobs, &config, &refs);
        let rec = Rec::new(Vec::new(), Duration::ZERO, None);
        h.backend = Some(Arc::<Rec>::clone(&rec) as Arc<dyn GitBackend>);
        let (_h, o) = run_bounded(h, all());
        assert_eq!(o.code, 0, "SNP_JOBS={jobs}: {}{}", o.stdout, o.stderr);
        assert_eq!(
            rec.diffs(),
            0,
            "SNP_JOBS={jobs}: {:#?}",
            rec.calls.lock().unwrap()
        );
        // the fallback takes today's baseline: one `git diff`
        let mut f = repo(jobs, &format!("{config}{}", no_files_hook()), &refs);
        let frec = Rec::new(Vec::new(), Duration::ZERO, None);
        f.backend = Some(Arc::<Rec>::clone(&frec) as Arc<dyn GitBackend>);
        let (_f, fo) = run_bounded(f, all());
        assert_eq!(frec.diffs(), 1, "{:#?}", frec.calls.lock().unwrap());
        assert_eq!(without_fallback(&fo.stdout), o.stdout);
    }
}

/// A fixer that writes, alone and after read-only natives: the same output, exit code and files as the fallback,
/// two `git diff`s (the gate's baseline, then the comparison), and no file written before the baseline ended (the
/// backend holds it 300 ms while the other partitions reach their first write).
#[test]
fn a_writer_matches_the_fallback_and_writes_after_the_baseline() {
    let names: Vec<String> = (0..40).map(|i| format!("w{i:02}.txt")).collect();
    let mut files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x  \n"[..])).collect();
    files.push(("a.json", b"{\"a\": 1}\n"));
    let paths: Vec<&str> = files.iter().map(|(p, _)| *p).collect();
    for hooks in [
        "    -   id: trailing-whitespace\n",
        "    -   id: check-json\n    -   id: detect-private-key\n    -   id: trailing-whitespace\n    \
         -   id: check-merge-conflict\n",
    ] {
        let config = format!("repos:\n{}", pch(hooks));
        for jobs in [1, 2, 4] {
            let mut h = repo(jobs, &config, &files);
            let watched = names
                .iter()
                .map(|n| (h.path(n), b"x  \n".to_vec()))
                .collect();
            let rec = Rec::new(watched, Duration::from_millis(300), None);
            h.backend = Some(Arc::<Rec>::clone(&rec) as Arc<dyn GitBackend>);
            let (h, o) = run_bounded(h, all());
            let what = format!("SNP_JOBS={jobs} {hooks:?}");
            assert_eq!(o.code, 1, "{what}: {}{}", o.stdout, o.stderr);
            assert!(
                o.stdout.contains("- files were modified by this hook"),
                "{what}: {}",
                o.stdout
            );
            assert_eq!(rec.diffs(), 2, "{what}: {:#?}", rec.calls.lock().unwrap());
            // the baseline found every file unwritten when it started and when it ended; the comparison runs
            // after the hook (all written)
            let early = rec.early.lock().unwrap().clone();
            assert_eq!(early, [0, 0, names.len(), names.len()], "{what}");
            let mut f = repo(jobs, &format!("{config}{}", no_files_hook()), &files);
            let frec = Rec::new(Vec::new(), Duration::ZERO, None);
            f.backend = Some(Arc::<Rec>::clone(&frec) as Arc<dyn GitBackend>);
            let (f, fo) = run_bounded(f, all());
            assert_eq!(
                (o.code, &o.stdout, &o.stderr),
                (fo.code, &without_fallback(&fo.stdout), &fo.stderr),
                "{what}"
            );
            assert_eq!(read_all(&h, &paths), read_all(&f, &paths), "{what}");
        }
    }
}

/// A mixed run (natives around a process hook) keeps today's baseline position: its `GIT_TRACE` from the first
/// `git diff` on is the list recorded from the branch's base (perf/w5-o1), byte for byte (Git mode, so the process
/// hook's comparison is a real `git diff` whatever the filesystem; `--files`, so neither a stash nor ADR-106's
/// parallel before-diff; no prefetch consumer, so no concurrent git).
#[test]
fn a_mixed_run_keeps_todays_git_order() {
    let config = format!(
        "repos:\n{}-   repo: local\n    hooks:\n{}{}",
        pch("    -   id: trailing-whitespace\n"),
        sys(
            "proc",
            "proc",
            "git rev-parse --git-dir",
            "always_run: true\npass_filenames: false"
        ),
        pch("    -   id: end-of-file-fixer\n"),
    );
    for jobs in [1, 4] {
        let mut h = repo(jobs, &config, &[("a.txt", b"x  ")]);
        let trace = h.base.join("git.trace");
        h.env_set("GIT_TRACE", trace.as_os_str());
        h.env_set("SNP_DIFF_MODE", "git");
        let a = snp_cli::RunArgs {
            files: vec![OsString::from("a.txt")],
            ..args()
        };
        let (_h, o) = run_bounded(h, a);
        assert_eq!(o.code, 1, "{}{}", o.stdout, o.stderr);
        let text = std::fs::read_to_string(&trace).unwrap();
        let got: Vec<&str> = text
            .lines()
            .filter_map(|l| l.split_once("trace: built-in: git ").map(|(_, a)| a))
            .skip_while(|a| *a != DIFF)
            .collect();
        assert_eq!(
            got,
            [DIFF, DIFF, "rev-parse --git-dir", DIFF, DIFF],
            "SNP_JOBS={jobs}"
        );
    }
}

/// One race-table case: `hooks` run on `--files names`, with `committed` files (and `e.txt`) committed and
/// `untracked` ones written after; an edit of the committed `e.txt` is injected `when`. `(exit code, stdout)`.
fn injected(
    hooks: &str,
    committed: &[(&str, &[u8])],
    untracked: &[(&str, &[u8])],
    names: &[&str],
    when: When,
) -> (i32, String) {
    let config = format!("repos:\n{}", pch(hooks));
    let mut files = committed.to_vec();
    files.push(("e.txt", b"e\n"));
    let mut h = repo(2, &config, &files);
    for (p, b) in untracked {
        h.write(p, b);
    }
    let edit = Edit {
        when,
        path: h.path("e.txt"),
        bytes: b"edited by another process\n".to_vec(),
    };
    let rec = Rec::new(Vec::new(), Duration::ZERO, Some(edit));
    h.backend = Some(Arc::<Rec>::clone(&rec) as Arc<dyn GitBackend>);
    let a = snp_cli::RunArgs {
        files: names.iter().map(OsString::from).collect(),
        ..args()
    };
    let (h, o) = run_bounded(h, a);
    assert!(o.failure.is_none(), "{:?}", o.failure);
    assert!(
        rec.edit.lock().unwrap().is_none(),
        "the edit was never injected: {:#?}",
        rec.calls.lock().unwrap()
    );
    assert_eq!(h.read("e.txt"), "edited by another process\n");
    (o.code, o.stdout)
}

/// The panel's race table (ADR-132) for all-in-process runs, case by case. A fix of an untracked file (`--files
/// u.txt`) is one `git diff` cannot see; a fix of a tracked file is visible.
#[test]
fn injected_edits_match_the_race_table() {
    const MODIFIED: &str = "- files were modified by this hook";
    const ADDED: &str = "diff --staged --name-only --diff-filter=A";
    let tw = "    -   id: trailing-whitespace\n";
    let dirty: &[(&str, &[u8])] = &[("u.txt", b"x  \n")];
    // an edit before the first hook is absorbed: only the writer's own (invisible) fix is left
    let (code, out) = injected(tw, &[], dirty, &["u.txt"], When::FirstCall);
    assert!(
        code == 1 && out.contains("Fixing u.txt") && !out.contains(MODIFIED),
        "{out}"
    );
    // an edit inside the writer before its first write, its fix visible: the writer modified files (as today)
    let (code, out) = injected(
        tw,
        &[("t.txt", b"x  \n")],
        &[],
        &["t.txt"],
        When::Before(DIFF),
    );
    assert!(code == 1 && out.contains(MODIFIED), "{out}");
    // the same with an invisible fix: the edit is in the baseline, so nothing is reported (the accepted race
    // window: today the writer was charged with the edit; the exit code is the fixer's 1 either way)
    let (code, out) = injected(tw, &[], dirty, &["u.txt"], When::Before(DIFF));
    assert!(
        code == 1 && out.contains("Fixing u.txt") && !out.contains(MODIFIED),
        "{out}"
    );
    // an edit inside the writer after its baseline: the writer modified files (pre-commit's verdict)
    let (code, out) = injected(tw, &[], dirty, &["u.txt"], When::After(DIFF));
    assert!(code == 1 && out.contains(MODIFIED), "{out}");
    // an edit during a read-only native, no writer: nobody is charged (as today), exit 0
    let calf = "    -   id: check-added-large-files\n";
    let clean: &[(&str, &[u8])] = &[("t.txt", b"clean\n")];
    let (code, out) = injected(calf, clean, &[], &["t.txt"], When::Before(ADDED));
    assert!(code == 0 && !out.contains(MODIFIED), "{out}");
    // the same, then a writer with an invisible fix: the edit predates the writer's baseline (today it was charged)
    let (code, out) = injected(
        &format!("{calf}{tw}"),
        clean,
        dirty,
        &["t.txt", "u.txt"],
        When::Before(ADDED),
    );
    assert!(
        code == 1 && out.contains("Fixing u.txt") && !out.contains(MODIFIED),
        "{out}"
    );
}

/// `--show-diff-on-failure` with no writer: the run's diff is taken at the end (pre-commit's last `diff_after`), so
/// a failing check over a dirty tree prints the "All changes made by hooks:" block exactly like the fallback, and a
/// clean tree prints none.
#[test]
fn show_diff_without_a_writer_sees_the_runs_diff() {
    let config = format!("repos:\n{}", pch("    -   id: check-json\n"));
    for dirty in [true, false] {
        let make = |config: &str| {
            let h = repo(
                2,
                config,
                &[("bad.json", b"{\"a\": }\n"), ("d.txt", b"d\n")],
            );
            if dirty {
                h.write("d.txt", b"changed\n");
            }
            h
        };
        let a = || snp_cli::RunArgs {
            show_diff_on_failure: true,
            ..all()
        };
        let mut h = make(&config);
        let rec = Rec::new(Vec::new(), Duration::ZERO, None);
        h.backend = Some(Arc::<Rec>::clone(&rec) as Arc<dyn GitBackend>);
        let (_h, o) = run_bounded(h, a());
        assert_eq!(o.code, 1, "{}{}", o.stdout, o.stderr);
        assert_eq!(
            o.stdout.contains("All changes made by hooks:"),
            dirty,
            "{}",
            o.stdout
        );
        // the show-diff gate's lazy diff is the run's only oracle diff
        assert_eq!(rec.diffs(), 1, "{:#?}", rec.calls.lock().unwrap());
        let (_f, fo) = run_bounded(make(&format!("{config}{}", no_files_hook())), a());
        assert_eq!(
            (o.code, &o.stdout, &o.stderr),
            (fo.code, &without_fallback(&fo.stdout), &fo.stderr)
        );
    }
}
