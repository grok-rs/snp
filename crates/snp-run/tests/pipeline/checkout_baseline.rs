//! ADR-119: after a stash that checked out the patch's files, the baseline `git diff` is skipped only when the
//! oracle's snapshot proves it empty. The B8 shape skips it (a counting probe); every condition that can make the
//! checkout's result differ from what `git diff` reads (filters and every attributes file, a tracked `.gitattributes`
//! only the index has, `core.autocrlf`, `post-checkout` and `post-index-change` hooks however configured,
//! configuration-based hooks, intent-to-add entries, symbolic links, sparse checkout, `core.ignoreStat`, fsmonitor)
//! runs it; and in every case, randomized ones included,
//! the run's output, exit code and resulting repository equal the always-diff path's (`SNP_DIFF_MODE=git`), and
//! `SNP_VERIFY_BASELINE=1` never finds a proven baseline non-empty.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_git::{GitBackend, ProcBackend};
use snp_proc::spawn::{Captured, Group, PyStatus, Role, SpawnError, Stdin};

use crate::harness::{H, Out, StepClock, args, local, sys};

const DIFF: &str = "diff --no-ext-diff --no-textconv --ignore-submodules";
const MARK: &str = ".hook-ran";
/// The hook marks its start and touches `big.txt` without changing it: a stat change whose real `git diff` equals
/// the baseline, so a wrongly elided (empty) baseline in place of a non-empty one reads as "files were modified".
const HOOK: &str = "sh -c 'touch .hook-ran big.txt' --";

/// Counts the baseline-shaped `git diff` calls before and after the hook started.
#[derive(Debug)]
struct Counting {
    inner: ProcBackend,
    marker: PathBuf,
    before: AtomicUsize,
    after: AtomicUsize,
}

impl Counting {
    fn new(marker: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            inner: ProcBackend {
                cancel: CancelToken::new(),
                role: Role::Tool,
            },
            marker,
            before: AtomicUsize::new(0),
            after: AtomicUsize::new(0),
        })
    }

    fn note(&self, argv: &[OsString]) {
        let a: Vec<String> = argv
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        if a.join(" ") == DIFF {
            let n = if self.marker.exists() {
                &self.after
            } else {
                &self.before
            };
            n.fetch_add(1, Ordering::SeqCst);
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

/// Numbered lines; `salt` changes every line.
fn text(salt: u32, lines: usize) -> Vec<u8> {
    let mut s = String::new();
    let mut x = salt;
    for i in 0..lines {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        _ = writeln!(s, "line {i:05} {:06} lorem ipsum", (x >> 8) % 1_000_000);
    }
    s.into_bytes()
}

/// One repository state: `before` runs after the first commit (it may commit), `after` once the B8 changes are in
/// place (a staged `small.txt` for the hook, an unstaged rewrite of `big.txt` for the stash).
struct Case<'a> {
    name: &'a str,
    before: &'a dyn Fn(&H),
    after: &'a dyn Fn(&H),
}

fn nothing(_: &H) {}

fn make(case: &Case<'_>) -> H {
    let mut h = H::new();
    // the warm window of ADR-078(3) must hold however loaded the machine is
    h.clock = Some(Arc::new(StepClock::new(Duration::from_millis(1))));
    h.config(&local(&sys("p", "touch", HOOK, "files: '^small\\.txt$'")));
    h.write("big.txt", &text(1, 400));
    h.write("small.txt", b"small\n");
    h.write("other.txt", b"other\n");
    h.git(&["add", "."]);
    h.commit("init");
    (case.before)(&h);
    h.write("small.txt", b"small, changed\n");
    h.stage("small.txt");
    let mut rewrite = text(2, 400);
    rewrite.extend_from_slice(b"one more line\n");
    h.write("big.txt", &rewrite);
    (case.after)(&h);
    h
}

/// What a run leaves: exit code, stdout with the temporary directory folded, stderr, and the repository (the
/// worktree's and the index's view through git, and the mode and bytes of every file the cases touch).
fn outcome(h: &H, o: &Out) -> String {
    let base = h.base.to_string_lossy().into_owned();
    let diff = h.git_any(&["diff", "--no-ext-diff", "--no-textconv"]);
    let cached = h.git_any(&["diff", "--cached", "--no-ext-diff", "--no-textconv"]);
    let status = h.git_any(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
    let mut files = String::new();
    for f in ["big.txt", "small.txt", "other.txt", "link", "d/f.txt"] {
        let seen = std::fs::symlink_metadata(h.path(f)).ok().map(|md| {
            let mut hasher = DefaultHasher::new();
            std::fs::read(h.path(f))
                .unwrap_or_default()
                .hash(&mut hasher);
            (md.permissions().mode(), md.len(), hasher.finish())
        });
        _ = writeln!(files, "{f}: {seen:?}");
    }
    format!(
        "code {}\nstdout\n{}\nstderr\n{}\ndiff {}\ncached {}\nstatus {}\n{files}",
        o.code,
        o.stdout.replace(&base, "<base>"),
        o.stderr.replace(&base, "<base>"),
        String::from_utf8_lossy(&diff.stdout),
        String::from_utf8_lossy(&cached.stdout),
        String::from_utf8_lossy(&status.stdout),
    )
}

/// Runs `case` with `env`; returns the outcome and the baseline-shaped `git diff` calls before the hook.
fn run(case: &Case<'_>, env: &[(&str, &str)]) -> (String, usize, bool) {
    let mut h = make(case);
    for (k, v) in env {
        h.env_set(k, *v);
    }
    let rec = Counting::new(h.path(MARK));
    let backend: Arc<dyn GitBackend> = Arc::<Counting>::clone(&rec);
    h.backend = Some(backend);
    let o = h.run(args());
    assert!(
        o.stdout.contains("[INFO] Stashing unstaged files to "),
        "{}: the case must stash: {o:?}",
        case.name
    );
    // ADR-119's proof is attempted on Linux only
    let stat =
        cfg!(target_os = "linux") && snp_os::fsinfo::stat_oracle_safe(&h.root).unwrap_or(false);
    (outcome(&h, &o), rec.before.load(Ordering::SeqCst), stat)
}

/// The run equals the always-diff path and `SNP_VERIFY_BASELINE` agrees; returns the default run's baseline-shaped
/// `git diff` calls before the hook (0: elided) and whether the proof can apply at all (Linux, a Stat-mode
/// filesystem).
fn check(case: &Case<'_>) -> (usize, bool) {
    check_with(case, &[])
}

/// [`check`] with `extra` in the environment of every run.
fn check_with(case: &Case<'_>, extra: &[(&str, &str)]) -> (usize, bool) {
    let with = |kv: &[(&'static str, &'static str)]| -> Vec<(&str, &str)> {
        extra.iter().copied().chain(kv.iter().copied()).collect()
    };
    let (got, before, stat) = run(case, extra);
    let (always, always_before, _) = run(case, &with(&[("SNP_DIFF_MODE", "git")]));
    assert_eq!(
        always_before, 1,
        "{}: the always-diff path takes the baseline",
        case.name
    );
    assert_eq!(
        got, always,
        "{}: the outcome differs from the always-diff path",
        case.name
    );
    let (verified, _, _) = run(case, &with(&[("SNP_VERIFY_BASELINE", "1")]));
    assert!(
        !verified.contains("SNP_VERIFY_BASELINE"),
        "{}: a proven baseline was not empty:\n{verified}",
        case.name
    );
    assert_eq!(
        verified, always,
        "{}: the verified outcome differs",
        case.name
    );
    (before, stat)
}

/// Asserts that `case` skips the baseline (where the proof can apply) and matches the always-diff path.
fn skips(case: &Case<'_>) {
    let (before, stat) = check(case);
    assert_eq!(
        before,
        usize::from(!stat),
        "{}: the baseline is elided",
        case.name
    );
}

/// Asserts that `case` takes the baseline and matches the always-diff path.
fn runs_the_diff(case: &Case<'_>) {
    let (before, _) = check(case);
    assert_eq!(before, 1, "{}: the baseline `git diff` runs", case.name);
}

fn config(h: &H, kv: &[(&str, &str)]) {
    for (k, v) in kv {
        h.git(&["config", k, v]);
    }
}

fn hook(dir: &Path, name: &str, body: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The B8 shape (a large unstaged rewrite, a staged change for one process hook) skips the baseline `git diff`; the
/// one after the hook (its `touch` changed a stat) still runs, and the run equals the always-diff path.
#[test]
fn the_b8_shape_skips_the_baseline_diff() {
    skips(&Case {
        name: "b8",
        before: &nothing,
        after: &nothing,
    });
    // an unstaged deletion and an executable file: the checkout recreates them with their modes
    skips(&Case {
        name: "deleted and executable",
        before: &|h| {
            h.exec("run.sh", b"#!/bin/sh\n");
            h.git(&["add", "run.sh"]);
            h.commit("exec");
        },
        after: &|h| {
            std::fs::remove_file(h.path("other.txt")).unwrap();
            std::fs::write(h.path("run.sh"), b"#!/bin/sh\necho changed\n").unwrap();
        },
    });
}

/// Mode-only changes are in the patch; the checkout restores the mode, which the proof reads back (git compares the
/// owner-executable bit). With `core.fileMode=false` the mode change is not in the patch at all.
#[test]
fn a_mode_only_change_is_restored_and_proven() {
    let chmod = |h: &H| {
        std::fs::set_permissions(h.path("other.txt"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    };
    skips(&Case {
        name: "mode only",
        before: &nothing,
        after: &chmod,
    });
    skips(&Case {
        name: "mode only, core.fileMode=false",
        before: &|h| config(h, &[("core.fileMode", "false")]),
        after: &chmod,
    });
}

/// Assume-unchanged and skip-worktree entries are invisible to both `diff-index` and `git diff` (git does not read
/// their worktree files), so they are never checked out and never printed: the proof holds, and the outcome is git's.
#[test]
fn assume_unchanged_and_skip_worktree_entries_stay_invisible() {
    skips(&Case {
        name: "assume-unchanged, edited",
        before: &|h| {
            h.git(&["update-index", "--assume-unchanged", "other.txt"]);
        },
        after: &|h| h.write("other.txt", b"edited behind git's back\n"),
    });
    skips(&Case {
        name: "skip-worktree, deleted",
        before: &|h| {
            h.git(&["update-index", "--skip-worktree", "other.txt"]);
        },
        after: &|h| std::fs::remove_file(h.path("other.txt")).unwrap(),
    });
}

/// Every attributes file, wherever git reads it from, and `core.autocrlf` run the baseline. The non-round-tripping
/// smudge filter makes the real baseline non-empty: an elided one would blame the hook.
#[test]
fn attributes_filters_and_autocrlf_run_the_diff() {
    let filter = |h: &H| {
        h.write(".gitattributes", b"big.txt filter=upper\n");
        config(
            h,
            &[
                ("filter.upper.smudge", "sed s/lorem/LOREM/"),
                ("filter.upper.clean", "cat"),
            ],
        );
        h.git(&["add", ".gitattributes"]);
        h.commit("attributes");
    };
    runs_the_diff(&Case {
        name: "a smudge filter that does not round-trip",
        before: &filter,
        after: &nothing,
    });
    runs_the_diff(&Case {
        name: "core.autocrlf=true",
        before: &|h| config(h, &[("core.autocrlf", "true")]),
        after: &nothing,
    });
    runs_the_diff(&Case {
        name: "a nested .gitattributes",
        before: &|h| {
            h.write("d/f.txt", b"f\n");
            h.write("d/.gitattributes", b"*.txt text eol=crlf\n");
            h.git(&["add", "d"]);
            h.commit("nested");
        },
        after: &|h| h.write("d/f.txt", b"f, changed\n"),
    });
    runs_the_diff(&Case {
        name: "an untracked .gitattributes",
        before: &nothing,
        after: &|h| h.write(".gitattributes", b"other.txt -text\n"),
    });
    runs_the_diff(&Case {
        name: "info/attributes",
        before: &|h| h.write(".git/info/attributes", b"*.txt ident\n"),
        after: &nothing,
    });
    runs_the_diff(&Case {
        name: "core.attributesFile",
        before: &|h| {
            let f = h.base.join("attrs");
            std::fs::write(&f, b"*.txt text\n").unwrap();
            config(h, &[("core.attributesFile", f.to_str().unwrap())]);
        },
        after: &nothing,
    });
    // a tracked `.gitattributes` missing from the worktree but not in the patch (skip-worktree, or assume-unchanged
    // and deleted): git reads the index's copy instead, first when the checkout writes a file, as a fall-back when
    // `git diff` reads one, so the smudge filter applies although no attributes file exists
    for (bit, name) in [
        (
            "--skip-worktree",
            "an index-only .gitattributes, skip-worktree",
        ),
        (
            "--assume-unchanged",
            "an index-only .gitattributes, assume-unchanged",
        ),
    ] {
        runs_the_diff(&Case {
            name,
            before: &|h| {
                filter(h);
                h.git(&["update-index", bit, ".gitattributes"]);
                std::fs::remove_file(h.path(".gitattributes")).unwrap();
            },
            after: &nothing,
        });
    }
    runs_the_diff(&Case {
        name: "the default attributes file",
        before: &|h| {
            let d = h.base.join("home/.config/git");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("attributes"), b"*.txt text\n").unwrap();
        },
        after: &nothing,
    });
}

/// A `post-checkout` or `post-index-change` hook can change the worktree after the checkout (here it appends to a
/// tracked file, so the real baseline is not empty): the baseline runs wherever git finds the hook, and so it does
/// for any configuration-based hook.
#[test]
fn checkout_hooks_run_the_diff() {
    let edit = "echo hooked >> other.txt";
    runs_the_diff(&Case {
        name: "post-checkout",
        before: &|h| hook(&h.root.join(".git/hooks"), "post-checkout", edit),
        after: &nothing,
    });
    runs_the_diff(&Case {
        name: "post-index-change",
        before: &|h| hook(&h.root.join(".git/hooks"), "post-index-change", edit),
        after: &nothing,
    });
    runs_the_diff(&Case {
        name: "a non-executable post-checkout",
        before: &|h| {
            hook(&h.root.join(".git/hooks"), "post-checkout", edit);
            std::fs::set_permissions(
                h.root.join(".git/hooks/post-checkout"),
                std::fs::Permissions::from_mode(0o644),
            )
            .unwrap();
        },
        after: &nothing,
    });
    runs_the_diff(&Case {
        name: "core.hooksPath, relative",
        before: &|h| {
            hook(&h.root.join(".githooks"), "post-checkout", edit);
            config(h, &[("core.hooksPath", ".githooks")]);
        },
        after: &nothing,
    });
    runs_the_diff(&Case {
        name: "core.hooksPath, absolute",
        before: &|h| {
            let d = h.base.join("hooks");
            hook(&d, "post-index-change", edit);
            config(h, &[("core.hooksPath", d.to_str().unwrap())]);
        },
        after: &nothing,
    });
    runs_the_diff(&Case {
        name: "core.hooksPath with ~",
        before: &|h| config(h, &[("core.hooksPath", "~/hooks")]),
        after: &nothing,
    });
    runs_the_diff(&Case {
        name: "a configuration-based hook",
        before: &|h| {
            config(
                h,
                &[
                    ("hook.mark.command", "true"),
                    ("hook.mark.event", "post-checkout"),
                ],
            );
        },
        after: &nothing,
    });
    // a hooks directory with other hooks only proves nothing is run
    skips(&Case {
        name: "other hooks",
        before: &|h| hook(&h.root.join(".git/hooks"), "pre-push", edit),
        after: &nothing,
    });
}

/// Intent-to-add entries (cleared by the stash), a symbolic link in the patch, sparse checkout (`core.sparseCheckout`,
/// `index.sparse`), `core.ignoreStat` and an fsmonitor (configured, or git's test hook in the environment) run the
/// baseline; the untracked cache, which `git diff` never reads, does not stop the proof.
#[test]
fn intent_to_add_symlinks_sparse_and_ignore_stat_run_the_diff() {
    // a hook git calls and then distrusts (it fails), so git scans everything as without it
    let failing_monitor = |h: &H| {
        let d = h.base.join("monitor");
        hook(&d, "query", "exit 1");
        d.join("query")
    };
    runs_the_diff(&Case {
        name: "core.fsmonitor, a hook",
        before: &|h| {
            let q = failing_monitor(h);
            config(h, &[("core.fsmonitor", q.to_str().unwrap())]);
        },
        after: &nothing,
    });
    let probe = H::new();
    let q = failing_monitor(&probe);
    let (before, _) = check_with(
        &Case {
            name: "GIT_TEST_FSMONITOR",
            before: &nothing,
            after: &nothing,
        },
        &[("GIT_TEST_FSMONITOR", q.to_str().unwrap())],
    );
    assert_eq!(
        before, 1,
        "GIT_TEST_FSMONITOR: the baseline `git diff` runs"
    );
    runs_the_diff(&Case {
        name: "index.sparse",
        before: &|h| config(h, &[("index.sparse", "true")]),
        after: &nothing,
    });
    skips(&Case {
        name: "core.untrackedCache",
        before: &|h| config(h, &[("core.untrackedCache", "true")]),
        after: &nothing,
    });
    runs_the_diff(&Case {
        name: "intent-to-add",
        before: &nothing,
        after: &|h| {
            h.write("new.txt", b"new\n");
            h.git(&["add", "-N", "new.txt"]);
        },
    });
    runs_the_diff(&Case {
        name: "a retargeted symlink",
        before: &|h| {
            std::os::unix::fs::symlink("small.txt", h.path("link")).unwrap();
            h.git(&["add", "link"]);
            h.commit("link");
        },
        after: &|h| {
            std::fs::remove_file(h.path("link")).unwrap();
            std::os::unix::fs::symlink("other.txt", h.path("link")).unwrap();
        },
    });
    runs_the_diff(&Case {
        name: "core.sparseCheckout",
        before: &|h| config(h, &[("core.sparseCheckout", "true")]),
        after: &nothing,
    });
    runs_the_diff(&Case {
        name: "core.ignoreStat",
        before: &|h| config(h, &[("core.ignoreStat", "true")]),
        after: &nothing,
    });
}

/// A tracked directory replaced by a symbolic link: its files are beyond the link, so `diff-index` reports them
/// deleted and the checkout makes the directory real again; the proof reads every directory back.
#[test]
fn a_directory_replaced_by_a_symlink_matches_git() {
    let (before, stat) = check(&Case {
        name: "a directory behind a symlink",
        before: &|h| {
            h.write("d/f.txt", b"f\n");
            h.git(&["add", "d"]);
            h.commit("d");
        },
        after: &|h| {
            h.write("elsewhere/f.txt", b"elsewhere\n");
            std::fs::remove_dir_all(h.path("d")).unwrap();
            std::os::unix::fs::symlink("elsewhere", h.path("d")).unwrap();
        },
    });
    assert!(before <= 1 && (stat || before == 1));
}

/// Randomized differential (fixed seed): repositories mixing the cases above; every run equals the always-diff path,
/// `SNP_VERIFY_BASELINE` never finds a proven baseline non-empty, and some runs do skip the diff.
#[test]
fn randomized_states_match_the_always_diff_path() {
    let mut seed = 0x5eed_u32;
    let mut next = move |n: u32| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 24) % n
    };
    let mut elided = 0_usize;
    let mut stat_capable = false;
    for i in 0..16 {
        let picks: Vec<u32> = (0..8).map(|_| next(4)).collect();
        let before = |h: &H| {
            if picks[0] == 0 {
                config(h, &[("core.autocrlf", "true")]);
            }
            if picks[1] <= 1 {
                h.write(".gitattributes", b"*.txt text\n");
                h.git(&["add", ".gitattributes"]);
                h.commit("attrs");
            }
            if picks[1] == 1 {
                // tracked, but only the index has it
                h.git(&["update-index", "--skip-worktree", ".gitattributes"]);
                std::fs::remove_file(h.path(".gitattributes")).unwrap();
            }
            if picks[2] == 0 {
                hook(
                    &h.root.join(".git/hooks"),
                    "post-checkout",
                    "echo hooked >> other.txt",
                );
            }
            if picks[3] == 0 {
                h.git(&["update-index", "--assume-unchanged", "other.txt"]);
            }
            if picks[4] == 0 {
                config(h, &[("core.fileMode", "false")]);
            }
        };
        let after = |h: &H| {
            match picks[5] {
                0 => std::fs::remove_file(h.path("other.txt")).unwrap(),
                1 => h.write("other.txt", b"other, edited\n"),
                _ => {}
            }
            if picks[6] == 0 {
                std::fs::set_permissions(h.path("big.txt"), std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
            if picks[7] == 0 {
                h.write("big.txt", b"short\r\nwith crlf\r\n");
            }
        };
        let name = format!("random {i}: {picks:?}");
        let (b, stat) = check(&Case {
            name: &name,
            before: &before,
            after: &after,
        });
        stat_capable |= stat;
        elided += usize::from(b == 0);
    }
    assert!(
        !stat_capable || elided > 0,
        "no randomized state skipped the diff"
    );
}
