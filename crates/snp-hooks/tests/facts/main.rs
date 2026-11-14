//! `NativeHook::facts` (ADR-102): for every consumer × epoch × argument shape, the facts a native names are among
//! the git calls its run makes, and a run after `PchFacts::prefetch` of those facts prints, exits and crashes exactly
//! like a run without it, with the same set of git calls (the prefetched ones answered from the cache).
//!
//! Owner: WP-26.
#![cfg(test)]
#![cfg_attr(
    windows,
    allow(
        dead_code,
        unused_imports,
        reason = "the sandbox's shared helpers are partly posix-only"
    )
)]

#[path = "../framework/support.rs"]
mod support;

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_git::pch_facts::{FactSet, PchFacts};
use snp_git::{Git, GitBackend, GitExe, ProcBackend};
use snp_hooks::{Epoch, HookCx, NativeHook, NativeSource, Outcome};
use snp_proc::spawn::{Captured, Group, PyStatus, Role, SpawnError, Stdin};
use support::Sandbox;

/// The real git, with every call's arguments (after `argv[0]`) recorded.
#[derive(Debug)]
struct Recording {
    inner: ProcBackend,
    calls: Mutex<Vec<String>>,
}

impl Recording {
    fn record(&self, argv: &[OsString]) {
        let args: Vec<String> = argv
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        self.calls.lock().unwrap().push(args.join(" "));
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl GitBackend for Recording {
    fn capture(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
        group: Group,
    ) -> Result<Captured, SpawnError> {
        self.record(argv);
        self.inner.capture(argv, env, cwd, stdin, group)
    }

    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        self.record(argv);
        self.inner.stream(argv, env, cwd, sink)
    }

    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError> {
        self.record(argv);
        self.inner.inherit(argv, env, cwd, stdin)
    }
}

/// A work tree with a commit, a staged added file, an unstaged change and a case pair, on branch `master`.
fn repo() -> Sandbox {
    let sb = Sandbox::new(true);
    sb.write("README", b"readme\n");
    sb.write("dir/a.txt", b"a\n");
    sb.write("run.sh", b"#!/bin/sh\necho\n");
    sb.git(&["add", "."]);
    sb.git(&["commit", "-q", "-m", "init"]);
    sb.write("new.txt", b"new\n");
    sb.write("Dir/b.txt", b"b\n");
    sb.git(&["add", "new.txt", "Dir/b.txt"]);
    sb.write("dir/a.txt", b"changed\n");
    sb
}

/// One run of `hook` on `args + files` over a fresh facts cache: `(outcome, git calls of the prefetch, git calls of
/// the hook)`; the prefetch of `hook.facts(args, epoch)` comes first when `prefetch`.
fn run(
    sb: &Sandbox,
    hook: &dyn NativeHook,
    epoch: Epoch,
    args: &[&str],
    prefetch: bool,
) -> (Outcome, Vec<String>, Vec<String>) {
    let rec = Arc::new(Recording {
        inner: ProcBackend {
            cancel: CancelToken::new(),
            role: Role::Tool,
        },
        calls: Mutex::new(Vec::new()),
    });
    let exe = Arc::new(GitExe::resolve(&sb.env, &sb.root, None).unwrap());
    let backend: Arc<dyn GitBackend> = Arc::<Recording>::clone(&rec);
    let (git, _) = Git::discover(exe, &sb.env, &sb.root, backend).unwrap();
    rec.take();
    let facts = PchFacts::new(git.clone());
    let arcs: Vec<Arc<str>> = args.iter().map(|a| Arc::from(*a)).collect();
    if prefetch {
        facts.prefetch(hook.facts(&arcs, epoch), &|| false);
    }
    let fetched = rec.take();
    let cancel = CancelToken::new();
    let cx = HookCx {
        root: &sb.root,
        env: &sb.env,
        git: &git,
        color: false,
        epoch,
        windows: cfg!(windows),
        columns: 80,
        facts: &facts,
        cancel: &cancel,
    };
    let mut argv: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    argv.extend(["README", "new.txt", "Dir/b.txt", "run.sh"].map(OsStr::new));
    let o = hook.run(&cx, &argv);
    (o, fetched, rec.take())
}

/// Every consumer of ADR-102 with the epochs it supports and the argument shapes that change its facts.
/// `(source, hook id, epochs, argument shapes)`.
type Case = (NativeSource, &'static str, Vec<u16>, Vec<Vec<&'static str>>);

fn cases() -> Vec<Case> {
    use NativeSource::{Builtin, PreCommitHooks};
    vec![
        (
            PreCommitHooks,
            "check-merge-conflict",
            vec![1, 2, 3],
            vec![vec![]],
        ),
        (Builtin, "check-merge-conflict", vec![1], vec![vec![]]),
        (
            PreCommitHooks,
            "check-case-conflict",
            vec![1, 2, 3],
            vec![vec![]],
        ),
        (
            PreCommitHooks,
            "check-added-large-files",
            vec![1, 2, 3],
            vec![
                vec![],
                vec!["--enforce-all"],
                vec!["--enf"],
                vec!["--maxkb=1"],
            ],
        ),
        (
            Builtin,
            "check-added-large-files",
            vec![1],
            vec![vec![], vec!["--enforce-all"]],
        ),
        (
            PreCommitHooks,
            "check-executables-have-shebangs",
            vec![1, 2, 3, 4, 5],
            vec![vec![]],
        ),
        (
            Builtin,
            "check-executables-have-shebangs",
            vec![1],
            vec![vec![]],
        ),
        (
            PreCommitHooks,
            "no-commit-to-branch",
            vec![1, 2],
            vec![vec![], vec!["--branch", "dev"]],
        ),
        (Builtin, "no-commit-to-branch", vec![1], vec![vec![]]),
    ]
}

#[test]
fn facts_are_a_subset_of_the_run_and_a_prefetch_changes_nothing() {
    support::link_all();
    let sb = repo();
    let mut consumers = 0_usize;
    for (source, id, epochs, shapes) in cases() {
        let Some(hook) = snp_hooks::native(source, id) else {
            continue;
        };
        for e in epochs {
            let epoch = Epoch(e);
            for args in &shapes {
                let what = format!("{source:?} {id} epoch {e} {args:?}");
                let (plain, none, calls) = run(&sb, hook, epoch, args, false);
                assert!(none.is_empty(), "{what}");
                let (pre, fetched, rest) = run(&sb, hook, epoch, args, true);
                assert_eq!(
                    (plain.code, plain.crashed, &plain.output),
                    (pre.code, pre.crashed, &pre.output),
                    "{what}: {} vs {}",
                    String::from_utf8_lossy(&plain.output),
                    String::from_utf8_lossy(&pre.output)
                );
                // an argparse error stops before any git call (the hint covers the normal path only)
                if plain.code == 2 && plain.output.starts_with(b"usage:") {
                    assert!(calls.is_empty(), "{what}");
                    continue;
                }
                for f in &fetched {
                    assert!(
                        calls.contains(f),
                        "{what}: prefetched `{f}`, the run makes {calls:?}"
                    );
                }
                let mut with: Vec<&String> = fetched.iter().chain(&rest).collect();
                let mut without: Vec<&String> = calls.iter().collect();
                with.sort();
                with.dedup();
                without.sort();
                without.dedup();
                assert_eq!(with, without, "{what}");
                // a prefetched fact is answered from the cache: the hook does not ask git again
                for f in &fetched {
                    assert!(!rest.contains(f), "{what}: `{f}` asked twice");
                }
                consumers += usize::from(!hook.facts(&[], epoch).is_empty());
            }
        }
    }
    assert!(consumers >= 12, "{consumers}");
}

/// The hint of each consumer, spelled out (ADR-102 decision (1)).
#[test]
fn facts_follow_each_hooks_branches() {
    let up = |id| snp_hooks::native(NativeSource::PreCommitHooks, id).unwrap();
    let bi = |id| snp_hooks::native(NativeSource::Builtin, id);
    let none: &[Arc<str>] = &[];
    let enforce: &[Arc<str>] = &[Arc::from("--enforce-all")];
    let abbrev: &[Arc<str>] = &[Arc::from("--enforce")];
    let mcf = up("check-merge-conflict");
    assert_eq!(mcf.facts(none, Epoch(1)), FactSet::NONE);
    assert_eq!(mcf.facts(none, Epoch(2)), FactSet::GIT_DIR);
    if let Some(b) = bi("check-merge-conflict") {
        assert_eq!(b.facts(none, Epoch(1)), FactSet::GIT_DIR);
    }
    assert_eq!(
        up("check-case-conflict").facts(none, Epoch(1)),
        FactSet::TRACKED.union(FactSet::ADDED)
    );
    let large = up("check-added-large-files");
    assert_eq!(large.facts(none, Epoch(3)), FactSet::ADDED);
    assert_eq!(large.facts(enforce, Epoch(1)), FactSet::ADDED);
    assert_eq!(large.facts(enforce, Epoch(2)), FactSet::NONE);
    assert_eq!(large.facts(abbrev, Epoch(3)), FactSet::NONE);
    if let Some(b) = bi("check-added-large-files") {
        assert_eq!(b.facts(none, Epoch(1)), FactSet::NONE);
    }
    let shebangs = up("check-executables-have-shebangs");
    assert_eq!(shebangs.facts(none, Epoch(4)), FactSet::NONE);
    assert_eq!(shebangs.facts(none, Epoch(5)), FactSet::FILE_MODE);
    assert_eq!(
        up("no-commit-to-branch").facts(none, Epoch(1)),
        FactSet::BRANCH
    );
    // everything else names nothing
    for h in snp_hooks::pch::p0::HOOKS
        .iter()
        .chain(snp_hooks::pch::p1::HOOKS)
        .chain(snp_hooks::pch::p2::HOOKS)
        .chain(snp_hooks::pch::caveats::HOOKS)
    {
        if ![
            "check-merge-conflict",
            "check-case-conflict",
            "check-added-large-files",
            "check-executables-have-shebangs",
            "no-commit-to-branch",
        ]
        .contains(&h.id())
        {
            assert!(h.facts(none, Epoch(6)).is_empty(), "{}", h.id());
        }
    }
}
