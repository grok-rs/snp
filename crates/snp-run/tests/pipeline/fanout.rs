//! ADR-154: the pre-chain fan-out. The intent-to-add query C runs in step 6's fan-out (on the run's Tool-role
//! backend) when the index is proved free of intent-to-add entries, and the stash reuses its empty answer while the
//! index stamp holds; otherwise the stash asks C itself at pre-commit's position. Checked here: the B2 shape asks C
//! once, in the fan-out; an intent-to-add entry, a split index (the proof refuses) and an index rewritten after the
//! early answer (another `git add -N` meanwhile) each ask C at the stash's position, with pre-commit's warning and
//! the entry re-added; a refused proof keeps today's `GIT_TRACE` order (C after every step-6 query, then
//! `write-tree`, then `diff-index`).
//!
//! The speculative pre-chain (step 6 asked beside discovery's `rev-parse`, held here so the order is visible): taken
//! at the root of a repository whose `.git` is a directory, and its results kept (each query once); not started
//! with `GIT_DIR` set, from a subdirectory, in a linked worktree or for `--all-files`; started and discarded when
//! discovery reports another layout (a cwd reached through a symbolic link: git names the real git directory), where
//! step 6 asks again. Every case ends as the plain run does.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_git::{GitBackend, ProcBackend};
use snp_proc::spawn::{Captured, Group, PyStatus, Role, SpawnError, Stdin};

use crate::harness::{H, Out, args, local, sys};

/// Query C, as `GIT_TRACE` and the backend print it.
const C: &str =
    "diff --no-renames --no-ext-diff --ignore-submodules --diff-filter=A --name-only -z";

/// pre-commit's warning when C finds entries.
const ITA_WARNING: &str = "[WARNING] Unstaged intent-to-add files detected.";

/// The run's Tool-role backend, recording every call's arguments (the stash's own Critical backend is not this one,
/// so a C recorded here is the fan-out's), with an optional action after the first C returns.
struct Rec {
    inner: ProcBackend,
    calls: Mutex<Vec<String>>,
    /// `start <args>` and `end <args>` in the order they happened.
    events: Mutex<Vec<String>>,
    after_c: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// How long discovery's `rev-parse` is held before it runs (so anything speculated beside it starts first).
    hold_rev_parse: Duration,
}

impl std::fmt::Debug for Rec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rec")
    }
}

impl Rec {
    fn new(after_c: Option<Box<dyn FnOnce() + Send>>) -> Arc<Self> {
        Arc::new(Self {
            inner: ProcBackend {
                cancel: CancelToken::new(),
                role: Role::Tool,
            },
            calls: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            after_c: Mutex::new(after_c),
            hold_rev_parse: Duration::ZERO,
        })
    }

    /// A recorder that holds discovery's `rev-parse` for 300 ms.
    fn holding() -> Arc<Self> {
        let mut r = Arc::into_inner(Self::new(None)).unwrap();
        r.hold_rev_parse = Duration::from_millis(300);
        Arc::new(r)
    }

    /// The position of the first event that `pred` accepts.
    fn event(&self, pred: impl Fn(&str) -> bool) -> Option<usize> {
        self.events.lock().unwrap().iter().position(|e| pred(e))
    }

    /// Calls whose arguments start with `prefix`.
    fn count(&self, prefix: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.starts_with(prefix))
            .count()
    }

    /// Whether step 6's B started before discovery's `rev-parse` ended (the speculative pre-chain ran).
    fn speculated(&self) -> bool {
        let b = self.event(|e| e.starts_with("start diff --quiet"));
        let rev_parse = self.event(|e| e.starts_with("end rev-parse --show-cdup"));
        b.zip(rev_parse).is_some_and(|(b, r)| b < r)
    }

    fn around<T>(&self, argv: &[OsString], call: impl FnOnce() -> T) -> T {
        let args: Vec<String> = argv
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let args = args.join(" ");
        self.calls.lock().unwrap().push(args.clone());
        if args.starts_with("rev-parse --show-cdup") {
            std::thread::sleep(self.hold_rev_parse);
        }
        self.events.lock().unwrap().push(format!("start {args}"));
        let r = call();
        self.events.lock().unwrap().push(format!("end {args}"));
        if args == C
            && let Some(f) = self.after_c.lock().unwrap().take()
        {
            f();
        }
        r
    }

    fn fan_out_cs(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|a| *a == C)
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

/// A committed one-hook repository (the B2 shape) with an unstaged edit of a tracked file, so the stash saves a
/// patch, and `GIT_TRACE` going to a file.
fn repo() -> (H, PathBuf) {
    let mut h = H::new();
    h.config(&local(&sys(
        "noop",
        "noop",
        "'true'",
        "always_run: true\npass_filenames: false",
    )));
    h.write("t.txt", b"t\n");
    h.stage("t.txt");
    h.commit("t");
    h.write("t.txt", b"t\nunstaged\n");
    let trace = h.base.join("git.trace");
    h.env_set("GIT_TRACE", trace.as_os_str());
    (h, trace)
}

/// The git subcommands in `GIT_TRACE` order (process start order).
fn traced(trace: &Path) -> Vec<String> {
    std::fs::read_to_string(trace)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            l.split_once("trace: built-in: git ")
                .map(|(_, a)| a.to_owned())
        })
        .collect()
}

/// The intent-to-add entries of the index, as `git diff --diff-filter=A` lists them.
fn ita_entries(h: &H) -> String {
    let out = h.git(&["diff", "--no-renames", "--diff-filter=A", "--name-only"]);
    String::from_utf8(out.stdout).unwrap()
}

fn with_backend(mut h: H, rec: &Arc<Rec>) -> H {
    h.backend = Some(Arc::clone(rec) as Arc<dyn GitBackend>);
    h
}

fn assert_passed(o: &Out) {
    assert_eq!(o.code, 0, "{o:?}");
    assert!(o.stdout.contains("noop"), "{}", o.stdout);
    assert!(o.stdout.contains("Passed"), "{}", o.stdout);
}

/// The B2 shape: C runs once, in the fan-out, and the stash reuses its empty answer (no second C in the trace).
#[test]
fn c_runs_in_the_fan_out_and_the_stash_reuses_its_empty_answer() {
    let (h, trace) = repo();
    let rec = Rec::new(None);
    let h = with_backend(h, &rec);
    let o = h.run(args());
    assert_passed(&o);
    assert!(!o.stderr.contains("intent-to-add"), "{}", o.stderr);
    assert_eq!(rec.fan_out_cs(), 1, "{:?}", rec.calls.lock().unwrap());
    let t = traced(&trace);
    assert_eq!(t.iter().filter(|a| *a == C).count(), 1, "{t:#?}");
    assert_eq!(h.read("t.txt"), "t\nunstaged\n");
}

/// The fallback's `GIT_TRACE` order: C after every step-6 query (it is asked by the stash, serially), then
/// `write-tree`, then `diff-index`.
fn assert_todays_order(t: &[String]) {
    let pos = |pred: &dyn Fn(&str) -> bool| t.iter().position(|a| pred(a));
    let c = pos(&|a| a == C).expect("C asked");
    // the first of each: F (or A) and G come back at step 14
    let last_step6 = [
        pos(&|a| a.starts_with("ls-files")),
        pos(&|a| a.starts_with("config --list")),
        pos(&|a| a.starts_with("diff --quiet")),
    ]
    .into_iter()
    .flatten()
    .max()
    .expect("step-6 queries");
    let write_tree = pos(&|a| a == "write-tree").expect("write-tree");
    let diff_index =
        pos(&|a| a.starts_with("diff-index --ignore-submodules --binary")).expect("diff-index");
    assert!(
        last_step6 < c && c < write_tree && write_tree < diff_index,
        "{t:#?}"
    );
    assert_eq!(t.iter().filter(|a| *a == C).count(), 1, "{t:#?}");
}

/// An intent-to-add entry: the proof refuses, so C is not asked in the fan-out; the stash asks it at pre-commit's
/// position, warns, clears and re-adds the entry, in today's `GIT_TRACE` order.
#[test]
fn an_intent_to_add_entry_is_asked_at_the_stash_position() {
    let (h, trace) = repo();
    h.write("n.txt", b"n\n");
    h.git(&["add", "-N", "n.txt"]);
    let rec = Rec::new(None);
    let h = with_backend(h, &rec);
    let o = h.run(args());
    assert_passed(&o);
    assert!(o.stdout.contains(ITA_WARNING), "{}", o.stdout);
    assert_eq!(rec.fan_out_cs(), 0, "{:?}", rec.calls.lock().unwrap());
    assert_todays_order(&traced(&trace));
    assert_eq!(ita_entries(&h), "n.txt\n");
    assert_eq!(h.read("t.txt"), "t\nunstaged\n");
}

/// A split index (the strict reader refuses `link`): no C in the fan-out, today's order and outcome.
#[test]
fn a_split_index_keeps_todays_order() {
    let (h, trace) = repo();
    h.git(&["update-index", "--split-index"]);
    let rec = Rec::new(None);
    let h = with_backend(h, &rec);
    let o = h.run(args());
    assert_passed(&o);
    assert_eq!(rec.fan_out_cs(), 0, "{:?}", rec.calls.lock().unwrap());
    assert_todays_order(&traced(&trace));
    assert_eq!(h.read("t.txt"), "t\nunstaged\n");
}

/// The index is rewritten after the fan-out's empty answer (another `git add -N` meanwhile): the stamp changed, so
/// the stash asks C again and finds the new entry, with pre-commit's warning and the entry re-added at the end.
#[test]
fn an_index_rewritten_after_the_early_answer_asks_again() {
    let (h, trace) = repo();
    h.write("n.txt", b"n\n");
    let (root, env) = (h.root.clone(), h.env.clone());
    let rec = Rec::new(Some(Box::new(move || {
        let st = Command::new("git")
            .args(["add", "-N", "n.txt"])
            .current_dir(&root)
            .env_clear()
            .envs(
                env.iter()
                    .filter(|(k, _)| k != "GIT_TRACE")
                    .map(|(k, v)| (k, v)),
            )
            .status()
            .unwrap();
        assert!(st.success());
    })));
    let h = with_backend(h, &rec);
    let o = h.run(args());
    assert_passed(&o);
    assert_eq!(rec.fan_out_cs(), 1, "{:?}", rec.calls.lock().unwrap());
    assert!(o.stdout.contains(ITA_WARNING), "{}", o.stdout);
    let t = traced(&trace);
    assert_eq!(t.iter().filter(|a| *a == C).count(), 2, "{t:#?}");
    assert_eq!(ita_entries(&h), "n.txt\n");
    assert_eq!(h.read("t.txt"), "t\nunstaged\n");
}

/// The B2 shape at the root: step 6 starts beside discovery, and its answers are kept: B and C once each, the
/// stash's C reused.
#[test]
fn step6_is_asked_beside_discovery_at_the_root() {
    let (h, trace) = repo();
    let rec = Rec::holding();
    let h = with_backend(h, &rec);
    let o = h.run(args());
    assert_passed(&o);
    assert!(rec.speculated(), "{:#?}", rec.events.lock().unwrap());
    assert_eq!(rec.count("diff --quiet"), 1);
    assert_eq!(rec.fan_out_cs(), 1);
    let t = traced(&trace);
    assert_eq!(t.iter().filter(|a| *a == C).count(), 1, "{t:#?}");
    assert_eq!(
        t.iter().filter(|a| a.starts_with("diff --quiet")).count(),
        1,
        "{t:#?}"
    );
    assert_eq!(h.read("t.txt"), "t\nunstaged\n");
}

/// The gates refuse: no query starts before discovery ends, and the run ends as usual.
#[test]
fn no_speculation_with_git_dir_from_a_subdirectory_in_a_worktree_or_for_all_files() {
    for case in ["GIT_DIR", "subdirectory", "worktree", "all-files"] {
        let (mut h, _trace) = repo();
        let mut a = args();
        match case {
            "GIT_DIR" => {
                let git_dir = h.root.join(".git");
                let root = h.root.clone();
                h.env_set("GIT_DIR", git_dir.as_os_str());
                h.env_set("GIT_WORK_TREE", root.as_os_str());
            }
            "subdirectory" => {
                h.write("sub/s.txt", b"s\n");
                h.cwd = Some(h.root.join("sub"));
            }
            "worktree" => {
                h.git(&["commit", "-qam", "unstaged"]);
                let wt = h.base.join("wt");
                h.git(&["worktree", "add", &wt.to_string_lossy(), "HEAD"]);
                std::fs::write(wt.join("t.txt"), b"t\nunstaged\nagain\n").unwrap();
                h.cwd = Some(wt);
            }
            _ => a.all_files = true,
        }
        let rec = Rec::holding();
        let h = with_backend(h, &rec);
        let o = h.run(a);
        assert_passed(&o);
        assert!(
            !rec.speculated(),
            "{case}: {:#?}",
            rec.events.lock().unwrap()
        );
        assert!(rec.count("diff --quiet") <= 1, "{case}");
    }
}

/// Discovery reports another layout than the cwd's (the cwd is a symbolic link to the repository: git prints the
/// real git directory): the speculative answers are dropped and step 6 asks again; the run ends as usual.
#[test]
fn speculation_is_discarded_when_discovery_reports_another_layout() {
    let (mut h, trace) = repo();
    let link = h.base.join("link");
    std::os::unix::fs::symlink(&h.root, &link).unwrap();
    h.cwd = Some(link);
    let rec = Rec::holding();
    let h = with_backend(h, &rec);
    let o = h.run(args());
    assert_passed(&o);
    assert!(rec.speculated(), "{:#?}", rec.events.lock().unwrap());
    assert_eq!(rec.count("diff --quiet"), 2, "speculated, then asked again");
    let t = traced(&trace);
    assert_eq!(
        t.iter().filter(|a| a.starts_with("diff --quiet")).count(),
        2,
        "{t:#?}"
    );
    assert_eq!(h.read("t.txt"), "t\nunstaged\n");
}
