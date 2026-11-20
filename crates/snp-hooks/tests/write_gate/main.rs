//! The deferred baseline's write gate (ADR-132): with a [`WriteGate`] in the hook context, the first write of a hook
//! runs the real `git diff` before any byte is written, concurrent partitions that want to write wait for it, a hook
//! that writes nothing takes no `git diff`, and every fixer's outcome and files are the ones it has without a gate.
//! A recording git backend checks, when the diff starts and again when it ends, that no file was written yet.
//!
//! Owner: WP-26.
#![cfg(test)]
#![allow(
    dead_code,
    reason = "the sandbox's shared helpers serve every test target; this one uses a few"
)]

#[path = "../framework/support.rs"]
mod support;

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_git::pch_facts::PchFacts;
use snp_git::{DiffFingerprint, Git, GitBackend, GitExe, ProcBackend};
use snp_hooks::{HookCx, NativeSource, Outcome, WriteGate};
use snp_proc::spawn::{Captured, Group, PyStatus, Role, SpawnError, Stdin};
use support::{Sandbox, epoch_at};

/// The baseline `git diff`'s arguments.
const DIFF: &str = "diff --no-ext-diff --no-textconv --ignore-submodules";

/// The real git; every `git diff` checks the watched files are unwritten when it starts and when it ends.
#[derive(Debug)]
struct Watching {
    inner: ProcBackend,
    /// `(path, the bytes it must still hold during the diff)`.
    watched: Vec<(PathBuf, Vec<u8>)>,
    /// How long the diff holds the gate (concurrent writers wait meanwhile).
    hold: Duration,
    diffs: Mutex<Vec<String>>,
}

impl Watching {
    fn unwritten(&self) -> Vec<String> {
        self.watched
            .iter()
            .filter(|(p, b)| std::fs::read(p).ok().as_ref() != Some(b))
            .map(|(p, _)| p.display().to_string())
            .collect()
    }
}

impl GitBackend for Watching {
    fn capture(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
        group: Group,
    ) -> Result<Captured, SpawnError> {
        self.inner.capture(argv, env, cwd, stdin, group)
    }

    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        let args: Vec<String> = argv
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let args = args.join(" ");
        let at_start = self.unwritten();
        std::thread::sleep(self.hold);
        let r = self.inner.stream(argv, env, cwd, sink);
        let at_end = self.unwritten();
        assert!(
            at_start.is_empty() && at_end.is_empty(),
            "written before the baseline diff ended: {at_start:?} {at_end:?}"
        );
        self.diffs.lock().unwrap().push(args);
        r
    }

    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError> {
        self.inner.inherit(argv, env, cwd, stdin)
    }
}

/// A committed work tree whose `n` files `f00.txt`.. hold `content`, and a git handle over the watching backend.
fn repo(n: usize, content: &[u8], hold: Duration) -> (Sandbox, Git, Arc<Watching>, Vec<String>) {
    let sb = Sandbox::new(true);
    let names: Vec<String> = (0..n).map(|i| format!("f{i:02}.txt")).collect();
    for name in &names {
        sb.write(name, content);
    }
    sb.git(&["add", "."]);
    sb.git(&["commit", "-q", "-m", "init"]);
    let rec = Arc::new(Watching {
        inner: ProcBackend {
            cancel: CancelToken::new(),
            role: Role::Tool,
        },
        watched: names
            .iter()
            .map(|n| (sb.path(n), content.to_vec()))
            .collect(),
        hold,
        diffs: Mutex::new(Vec::new()),
    });
    let exe = Arc::new(GitExe::resolve(&sb.env, &sb.root, None).unwrap());
    let backend: Arc<dyn GitBackend> = Arc::<Watching>::clone(&rec);
    let (git, _) = Git::discover(exe, &sb.env, &sb.root, backend).unwrap();
    (sb, git, rec, names)
}

/// Runs the v6.0.0 native `id` over `names`, split into `parts` partitions run on concurrent threads (as the runner's
/// pool does), with `gate`.
fn run_parts(
    sb: &Sandbox,
    git: &Git,
    id: &str,
    names: &[String],
    parts: usize,
    gate: Option<&WriteGate>,
) -> Vec<Outcome> {
    let hook = snp_hooks::native(NativeSource::PreCommitHooks, id).unwrap();
    let facts = PchFacts::new(git.clone());
    let cancel = CancelToken::new();
    let cx = HookCx {
        root: &sb.root,
        env: &sb.env,
        git,
        color: false,
        epoch: epoch_at("v6.0.0", id).unwrap(),
        windows: cfg!(windows),
        columns: 80,
        facts: &facts,
        cancel: &cancel,
        cache: None,
        gate,
    };
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..parts)
            .map(|p| {
                let cx = &cx;
                std::thread::Builder::new()
                    .spawn_scoped(s, move || {
                        let argv: Vec<&OsStr> = names
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| i % parts == p)
                            .map(|(_, n)| OsStr::new(n))
                            .collect();
                        hook.run(cx, &argv)
                    })
                    .unwrap()
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

/// What a run left: every outcome's code, output and modified paths, and the files.
fn result(sb: &Sandbox, outs: &[Outcome], names: &[String]) -> (Vec<String>, Vec<Vec<u8>>) {
    let o = outs
        .iter()
        .map(|o| {
            format!(
                "{} {:?} {:?}",
                o.code,
                String::from_utf8_lossy(&o.output),
                o.modified
            )
        })
        .collect();
    let files = names
        .iter()
        .map(|n| std::fs::read(sb.path(n)).unwrap())
        .collect();
    (o, files)
}

/// Four partitions of trailing-whitespace that all write: one `git diff`, taken before any write (the backend
/// holds it 300 ms, so the other partitions reach their first write meanwhile and must wait), its baseline is the
/// clean pre-hook tree, and the outcomes and files equal an ungated run's.
#[test]
fn concurrent_writers_wait_for_one_baseline_taken_before_any_write() {
    let hold = Duration::from_millis(300);
    let (sb, git, rec, names) = repo(16, b"x  \n", hold);
    let gate = WriteGate::new(&git);
    let gated = run_parts(&sb, &git, "trailing-whitespace", &names, 4, Some(&gate));
    assert!(gate.fired());
    let baseline = gate.into_baseline().unwrap().unwrap();
    assert_eq!(baseline, DiffFingerprint::EMPTY);
    assert_eq!(*rec.diffs.lock().unwrap(), [DIFF]);
    let got = result(&sb, &gated, &names);

    let (sb2, git2, rec2, names2) = repo(16, b"x  \n", Duration::ZERO);
    let plain = run_parts(&sb2, &git2, "trailing-whitespace", &names2, 4, None);
    assert!(rec2.diffs.lock().unwrap().is_empty());
    assert_eq!(got, result(&sb2, &plain, &names2));
    assert!(got.1.iter().all(|b| b == b"x\n"), "{:?}", got.1);
}

/// end-of-file-fixer's two writes (append, truncate) go through the gate too.
#[test]
fn end_of_file_fixer_appends_and_truncates_after_the_baseline() {
    for (content, fixed) in [(&b"x"[..], &b"x\n"[..]), (b"x\n\n\n", b"x\n")] {
        let (sb, git, rec, names) = repo(4, content, Duration::from_millis(100));
        let gate = WriteGate::new(&git);
        let outs = run_parts(&sb, &git, "end-of-file-fixer", &names, 2, Some(&gate));
        assert!(outs.iter().all(|o| o.code == 1 && !o.modified.is_empty()));
        assert_eq!(*rec.diffs.lock().unwrap(), [DIFF]);
        assert!(gate.into_baseline().unwrap().is_ok());
        for n in &names {
            assert_eq!(std::fs::read(sb.path(n)).unwrap(), fixed);
        }
    }
}

/// A fixer with nothing to fix and a read-only native never fire the gate: no `git diff` at all.
#[test]
fn a_hook_that_writes_nothing_takes_no_diff() {
    let (sb, git, rec, names) = repo(8, b"clean\n", Duration::ZERO);
    for id in [
        "trailing-whitespace",
        "end-of-file-fixer",
        "mixed-line-ending",
        "check-added-large-files",
        "detect-private-key",
    ] {
        let gate = WriteGate::new(&git);
        let outs = run_parts(&sb, &git, id, &names, 2, Some(&gate));
        assert!(outs.iter().all(|o| o.code == 0), "{id}");
        assert!(!gate.fired(), "{id}");
        assert!(gate.into_baseline().is_none(), "{id}");
    }
    assert!(rec.diffs.lock().unwrap().is_empty());
}

/// Without a gate (the baseline is already known) the token is handed out at once: no `git diff`.
#[test]
fn without_a_gate_the_writer_takes_no_diff() {
    let (sb, git, rec, names) = repo(2, b"x  \n", Duration::ZERO);
    let outs = run_parts(&sb, &git, "trailing-whitespace", &names, 1, None);
    assert_eq!(outs.first().map(|o| o.code), Some(1));
    assert!(rec.diffs.lock().unwrap().is_empty());
}
