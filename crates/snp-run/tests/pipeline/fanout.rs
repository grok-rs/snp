//! ADR-154: the pre-chain fan-out. The intent-to-add query C runs in step 6's fan-out (on the run's Tool-role
//! backend) when the index is proved free of intent-to-add entries, and the stash reuses its empty answer while the
//! index stamp holds; otherwise the stash asks C itself at pre-commit's position. Checked here: the B2 shape asks C
//! once, in the fan-out; an intent-to-add entry, a split index (the proof refuses) and an index rewritten after the
//! early answer (another `git add -N` meanwhile) each ask C at the stash's position, with pre-commit's warning and
//! the entry re-added; a refused proof keeps today's `GIT_TRACE` order (C after every step-6 query, then
//! `write-tree`, then `diff-index`).

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

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
    after_c: Mutex<Option<Box<dyn FnOnce() + Send>>>,
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
            after_c: Mutex::new(after_c),
        })
    }

    fn around<T>(&self, argv: &[OsString], call: impl FnOnce() -> T) -> T {
        let args: Vec<String> = argv
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let args = args.join(" ");
        self.calls.lock().unwrap().push(args.clone());
        let r = call();
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
