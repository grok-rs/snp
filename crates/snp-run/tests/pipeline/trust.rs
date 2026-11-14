//! Trusted index modes on `--all-files` runs (ADR-106): the COMPATIBILITY §6.2 equivalence cases (every hook gets the
//! files pre-commit 4.7.0 gives it, whether the trust gate holds or not), the git queries of each path (the tagged
//! listing, the before-diff and the configuration gate before the first hook; today's queries when the trust cannot
//! apply; pre-commit's error when the tagged listing fails), and the invalidation after a hook whose `git rm` keeps
//! `git diff` empty (D-CRASH-08: a deletion between hooks is always seen).

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_git::{GitBackend, ProcBackend};
use snp_proc::spawn::{Captured, Group, PyStatus, Role, SpawnError, Stdin};

use crate::harness::{H, all, local, sys};

/// A listing hook: prints its files, one per line, and fails so pre-commit's output shows them without `verbose`
/// (no duration line). It also leaves an untracked marker (`git diff` never shows untracked files).
fn lister(id: &str, extra: &str) -> String {
    sys(
        id,
        id,
        "sh -c 'touch .hook-ran; printf \"%s\\n\" \"$@\"; exit 1' --",
        extra,
    )
}

/// The files a [`lister`] hook printed (empty when it was skipped), sorted: pre-commit shuffles a hook's files with
/// its fixed seed before partitioning them, and the table's order is git's index order on either path.
fn listed(stdout: &str, id: &str) -> Vec<String> {
    let head = format!("- hook id: {id}\n- exit code: 1\n\n");
    let Some(at) = stdout.find(&head) else {
        return Vec::new();
    };
    let mut files: Vec<String> = stdout
        .get(at + head.len()..)
        .unwrap_or_default()
        .lines()
        .take_while(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    files.sort();
    files
}

/// The four listing hooks of the equivalence cases: the default `[file]`, `[symlink]`, `[directory]` and `[python]`
/// (a shebang filter: identify reads the shebang of an executable file whose name gives no tags).
fn four_hooks() -> String {
    local(&format!(
        "{}{}{}{}",
        lister("files", ""),
        lister("links", "types: [symlink]"),
        lister("dirs", "types: [directory]"),
        lister("py", "types: [python]"),
    ))
}

fn committed(files: &[(&str, &[u8])]) -> H {
    let h = H::new();
    h.config_staged(&four_hooks());
    for (name, content) in files {
        h.write(name, content);
    }
    h.git(&["add", "."]);
    h.commit("init");
    h
}

/// Runs `--all-files` and returns the four hooks' lists.
fn lists(h: &H) -> [Vec<String>; 4] {
    let o = h.run(all());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(!o.stdout.contains("files were modified"), "{}", o.stdout);
    ["files", "links", "dirs", "py"].map(|id| listed(&o.stdout, id))
}

fn v(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|&s| s.to_owned()).collect()
}

fn is_root() -> bool {
    std::fs::metadata("/proc/self").is_ok_and(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0)
}

/// A clean tree (the gate holds): symlinks, a gitlink and an executable are classified as `lstat` would.
#[test]
fn equivalence_symlink_gitlink_and_executable_on_a_clean_tree() {
    let h = committed(&[
        ("a.py", b"print(1)\n"),
        ("b.txt", b"b\n"),
        ("run.sh", b"#!/bin/sh\n"),
    ]);
    std::fs::set_permissions(h.path("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("b.txt", h.path("link")).unwrap();
    std::fs::create_dir_all(h.path("sub")).unwrap();
    h.git(&["add", "."]);
    h.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        "160000,78981922613b2afb6025042ff6bd878ac1994e85,sub",
    ]);
    h.commit("more");
    let [files, links, dirs, py] = lists(&h);
    assert_eq!(
        files,
        v(&[".pre-commit-config.yaml", "a.py", "b.txt", "run.sh"])
    );
    assert_eq!(links, v(&["link"]));
    assert_eq!(
        dirs,
        v(&["sub"]),
        "the gitlink keeps its lstat: a directory"
    );
    assert_eq!(py, v(&["a.py"]));
}

/// A tracked file under an unsearchable directory: `git diff` reports the `EACCES` on stderr only (not clean, no
/// trust), and `os.path.lexists` drops the file.
#[test]
fn equivalence_chmod_000_parent_directory() {
    if is_root() {
        return;
    }
    let h = committed(&[("a.py", b"print(1)\n"), ("sub/c.py", b"print(2)\n")]);
    std::fs::set_permissions(h.path("sub"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let got = lists(&h);
    std::fs::set_permissions(h.path("sub"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(got[0], v(&[".pre-commit-config.yaml", "a.py"]));
    assert_eq!(got[3], v(&["a.py"]));
}

/// Assume-unchanged and skip-worktree entries whose files were deleted: `git diff` stays empty, the `h`/`S` tags turn
/// the trust off, and `lexists` drops them.
#[test]
fn equivalence_flagged_then_deleted() {
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        let h = committed(&[("a.py", b"print(1)\n"), ("gone.py", b"print(2)\n")]);
        h.git(&["update-index", flag, "gone.py"]);
        std::fs::remove_file(h.path("gone.py")).unwrap();
        let [files, _, _, py] = lists(&h);
        assert_eq!(files, v(&[".pre-commit-config.yaml", "a.py"]), "{flag}");
        assert_eq!(py, v(&["a.py"]), "{flag}");
    }
}

/// An intent-to-add entry (the diff shows it as new: no trust) is passed like any existing file.
#[test]
fn equivalence_intent_to_add_entry() {
    let h = committed(&[("a.py", b"print(1)\n")]);
    h.write("new.py", b"print(3)\n");
    h.git(&["add", "-N", "new.py"]);
    let [files, _, _, py] = lists(&h);
    assert_eq!(files, v(&[".pre-commit-config.yaml", "a.py", "new.py"]));
    assert_eq!(py, v(&["a.py", "new.py"]));
}

/// An unmerged entry (`diff --cc`: no trust; listed once, as `--deduplicate` does) is passed once.
#[test]
fn equivalence_unmerged_entry() {
    let h = committed(&[("f.py", b"base\n")]);
    h.git(&["checkout", "-q", "-b", "other"]);
    h.write("f.py", b"other\n");
    h.git(&["commit", "-q", "--no-verify", "-am", "other"]);
    h.git(&["checkout", "-q", "main"]);
    h.write("f.py", b"mine\n");
    h.git(&["commit", "-q", "--no-verify", "-am", "mine"]);
    assert!(!h.git_any(&["merge", "-q", "other"]).status.success());
    let [files, _, _, py] = lists(&h);
    assert_eq!(files, v(&[".pre-commit-config.yaml", "f.py"]));
    assert_eq!(py, v(&["f.py"]));
}

/// A FIFO in place of an empty tracked file: `git diff` compares their empty content and stays clean, but the empty
/// blob is never trusted, so the FIFO is identified by `lstat` (a file, read without blocking: D-CRASH-09 needs a
/// name without tags, so this one has `.txt`). A socket there would be `[socket]`, never `[file]`.
#[test]
fn equivalence_fifo_and_socket_over_empty_tracked_files() {
    let h = committed(&[("a.py", b"print(1)\n"), ("ff.txt", b""), ("so.txt", b"")]);
    std::fs::remove_file(h.path("ff.txt")).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(h.path("ff.txt"))
            .status()
            .unwrap()
            .success()
    );
    std::fs::set_permissions(h.path("ff.txt"), std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::remove_file(h.path("so.txt")).unwrap();
    drop(std::os::unix::net::UnixListener::bind(h.path("so.txt")).unwrap());
    std::fs::set_permissions(h.path("so.txt"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let [files, links, dirs, _] = lists(&h);
    assert_eq!(files, v(&[".pre-commit-config.yaml", "a.py", "ff.txt"]));
    assert!(links.is_empty() && dirs.is_empty());
}

/// An extensionless shebang script with mode 0o654 is staged as 100644 (no owner `x`): identify's `os.access` gives
/// root any `x` bit (then the shebang makes it `python`) and the owner only the owner bit. The index's owner bit is
/// never used: a `[python]` filter makes the file take its `lstat`.
#[test]
fn equivalence_extensionless_0o654_shebang_file() {
    let h = committed(&[
        ("a.py", b"print(1)\n"),
        ("tool", b"#!/usr/bin/env python3\nprint(2)\n"),
    ]);
    std::fs::set_permissions(h.path("tool"), std::fs::Permissions::from_mode(0o654)).unwrap();
    let mode = h.git(&["ls-files", "-s", "tool"]).stdout;
    assert!(
        mode.starts_with(b"100644 "),
        "{}",
        String::from_utf8_lossy(&mode)
    );
    let [files, _, _, py] = lists(&h);
    assert_eq!(files, v(&[".pre-commit-config.yaml", "a.py", "tool"]));
    if is_root() {
        assert_eq!(py, v(&["a.py", "tool"]));
    } else {
        assert_eq!(py, v(&["a.py"]));
    }
}

/// An fsmonitor hook that reports no change (protocol 2: a token and no path; protocol 1: no path).
const FSMONITOR_NONE: &str =
    "#!/bin/sh\nif test \"$1\" = 2; then printf 'last_update_token\\0'; fi\n";

/// git turns fsmonitor on from the environment when `core.fsmonitor` is unset (`GIT_TEST_FSMONITOR`, named
/// `GIT_FSMONITOR_TEST` before git 2.20), which the configuration query cannot see. With a hook that reports no
/// change, `git diff` skips the `lstat` of the fsmonitor-valid entries and stays empty and clean after a deletion;
/// those entries are tagged `h` (`ls-files -f`) and the gate refuses on the variable, so the deleted file takes its
/// `lstat` and is dropped, as pre-commit drops it.
#[test]
fn equivalence_fsmonitor_from_the_environment_then_deleted() {
    let mut h = committed(&[("a.py", b"print(1)\n"), ("gone.py", b"print(2)\n")]);
    let hook = h.base.join("fsmonitor-none");
    std::fs::write(&hook, FSMONITOR_NONE).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    for k in ["GIT_TEST_FSMONITOR", "GIT_FSMONITOR_TEST"] {
        h.env_set(k, hook.as_os_str());
    }
    // the first refresh writes the fsmonitor extension, the second marks the entries fsmonitor-valid
    h.git(&["update-index", "-q", "--refresh"]);
    h.git(&["update-index", "-q", "--refresh"]);
    std::fs::remove_file(h.path("gone.py")).unwrap();
    let [files, _, _, py] = lists(&h);
    assert_eq!(files, v(&[".pre-commit-config.yaml", "a.py"]));
    assert_eq!(py, v(&["a.py"]));
}

/// Under `core.symlinks=false` git checks a symlink out as a regular file holding its target and keeps comparing it
/// as a symlink, so `git diff` stays empty and clean: the gate reads the setting and the entry takes its `lstat` (a
/// file, as identify sees it). A gate that cannot ask git (the `git config` child fails to spawn: `EAGAIN`, `EMFILE`,
/// `ENOMEM`) grants nothing either (K5), whatever the configuration says.
#[test]
fn equivalence_symlinks_false_and_a_gate_that_cannot_spawn() {
    for spawn_fails in [false, true] {
        let mut h = committed(&[("a.py", b"print(1)\n"), ("b.txt", b"b\n")]);
        std::os::unix::fs::symlink("b.txt", h.path("link")).unwrap();
        h.git(&["add", "link"]);
        h.commit("link");
        h.git(&["config", "core.symlinks", "false"]);
        std::fs::remove_file(h.path("link")).unwrap();
        h.write("link", b"b.txt");
        assert!(h.git(&["diff"]).stdout.is_empty(), "git diff stays empty");
        let rec = if spawn_fails {
            Rec::spawn_failing(h.path(".hook-ran"), GATE, 11)
        } else {
            Rec::new(h.path(".hook-ran"), None)
        };
        let backend: Arc<dyn GitBackend> = Arc::<Rec>::clone(&rec);
        h.backend = Some(backend);
        let [files, links, _, py] = lists(&h);
        assert_eq!(
            files,
            v(&[".pre-commit-config.yaml", "a.py", "b.txt", "link"]),
            "spawn fails: {spawn_fails}"
        );
        assert!(links.is_empty(), "spawn fails: {spawn_fails}: {links:?}");
        assert_eq!(py, v(&["a.py"]));
        let before = before_hook(&rec.calls());
        assert!(
            before.iter().any(|a| a.starts_with(GATE)) && before.iter().any(|a| a == TAGGED),
            "{before:?}"
        );
    }
}

/// One git call through the run's backend, and whether a hook had run by then.
#[derive(Debug, Clone)]
struct Call {
    argv: String,
    after_hook: bool,
}

/// A Tool-role backend that records every call and can fail calls whose argv starts with `fail`: with exit 128, or
/// with a spawn error of errno `spawn_errno` (the child never ran).
#[derive(Debug)]
struct Rec {
    inner: ProcBackend,
    marker: PathBuf,
    fail: Option<&'static str>,
    spawn_errno: Option<i32>,
    calls: Mutex<Vec<Call>>,
}

impl Rec {
    fn new(marker: PathBuf, fail: Option<&'static str>) -> Arc<Self> {
        Self::with(marker, fail, None)
    }

    /// Calls whose argv starts with `fail` fail to spawn with `errno`.
    fn spawn_failing(marker: PathBuf, fail: &'static str, errno: i32) -> Arc<Self> {
        Self::with(marker, Some(fail), Some(errno))
    }

    fn with(marker: PathBuf, fail: Option<&'static str>, spawn_errno: Option<i32>) -> Arc<Self> {
        Arc::new(Self {
            inner: ProcBackend {
                cancel: CancelToken::new(),
                role: Role::Tool,
            },
            marker,
            fail,
            spawn_errno,
            calls: Mutex::new(Vec::new()),
        })
    }

    fn spawn_error(&self, argv: &[OsString]) -> Option<SpawnError> {
        self.spawn_errno.map(|errno| SpawnError {
            program: argv.first().cloned().unwrap_or_default(),
            io: io::Error::from_raw_os_error(errno),
        })
    }

    fn note(&self, argv: &[OsString]) -> bool {
        let argv = argv
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        let fail = self.fail.is_some_and(|f| argv.starts_with(f));
        self.calls.lock().unwrap().push(Call {
            argv,
            after_hook: self.marker.exists(),
        });
        fail
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn failed() -> Captured {
        Captured {
            status: PyStatus(128),
            stdout: Vec::new(),
            stderr: b"fatal: injected\n".to_vec(),
            stopped_by_tty: false,
        }
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
        if self.note(argv) {
            return match self.spawn_error(argv) {
                Some(e) => Err(e),
                None => Ok(Self::failed()),
            };
        }
        self.inner.capture(argv, env, cwd, stdin, group)
    }

    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn io::Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        if self.note(argv) {
            return match self.spawn_error(argv) {
                Some(e) => Err(e),
                None => Ok((PyStatus(128), b"fatal: injected\n".to_vec())),
            };
        }
        self.inner.stream(argv, env, cwd, sink)
    }

    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError> {
        if self.note(argv)
            && let Some(e) = self.spawn_error(argv)
        {
            return Err(e);
        }
        self.inner.inherit(argv, env, cwd, stdin)
    }
}

const TAGGED: &str = "ls-files -s -v -f -z";
const DEDUP: &str = "ls-files -z --deduplicate";
const DIFF: &str = "diff --no-ext-diff --no-textconv --ignore-submodules";
const GATE: &str = "config -z --bool --get-regexp";

fn recorded(h: &mut H, fail: Option<&'static str>) -> Arc<Rec> {
    let rec = Rec::new(h.path(".hook-ran"), fail);
    let backend: Arc<dyn GitBackend> = Arc::<Rec>::clone(&rec);
    h.backend = Some(backend);
    rec
}

fn before_hook(calls: &[Call]) -> Vec<String> {
    calls
        .iter()
        .filter(|c| !c.after_hook && !c.argv.starts_with("rev-parse"))
        .map(|c| c.argv.clone())
        .collect()
}

fn after_hook(calls: &[Call]) -> Vec<String> {
    calls
        .iter()
        .filter(|c| c.after_hook)
        .map(|c| c.argv.clone())
        .collect()
}

/// A trust-capable run asks the tagged listing, the before-diff and the gate before the first hook (in any order:
/// they run at once), never `ls-files -z --deduplicate`, and one `git diff` after the hook; so does a dirty tree,
/// where the gate fails and every file takes its `lstat`.
#[test]
fn trusted_run_queries_listing_diff_and_gate_before_the_first_hook() {
    for dirty in [false, true] {
        let mut h = committed(&[("a.py", b"print(1)\n")]);
        h.config(&local(&lister("files", "")));
        if dirty {
            h.write("a.py", b"print(2)\n");
        }
        let rec = recorded(&mut h, None);
        let o = h.run(all());
        assert_eq!(o.code, 1, "{o:?}");
        assert_eq!(
            listed(&o.stdout, "files"),
            v(&[".pre-commit-config.yaml", "a.py"])
        );
        let calls = rec.calls();
        let mut before = before_hook(&calls);
        before.sort();
        assert_eq!(before.len(), 3, "{calls:?}");
        assert!(before[0].starts_with(GATE), "{before:?}");
        assert_eq!(before[1], DIFF);
        assert_eq!(before[2], TAGGED);
        assert_eq!(after_hook(&calls), v(&[DIFF]), "{calls:?}");
    }
}

/// A filter on `executable`, or no hook that can run: today's queries (`ls-files -z --deduplicate`, the lazy
/// before-diff only when a hook runs), no tagged listing and no gate.
#[test]
fn runs_the_trust_cannot_help_keep_todays_queries() {
    let mut h = committed(&[("a.py", b"print(1)\n")]);
    h.config(&local(&format!(
        "{}{}",
        lister("files", ""),
        lister("exe", "types: [executable]")
    )));
    let rec = recorded(&mut h, None);
    let o = h.run(all());
    assert_eq!(o.code, 1, "{o:?}");
    let calls = rec.calls();
    assert_eq!(before_hook(&calls), v(&[DEDUP, DIFF]), "{calls:?}");
    // every hook skipped: no diff at all
    let mut h = committed(&[("a.py", b"print(1)\n")]);
    h.config(&local(&lister("files", "")));
    h.env_set("SKIP", "files");
    let rec = recorded(&mut h, None);
    let o = h.run(all());
    assert_eq!(o.code, 0, "{o:?}");
    let argv: Vec<String> = rec.calls().into_iter().map(|c| c.argv).collect();
    assert!(!argv.iter().any(|a| a == DIFF || a == TAGGED), "{argv:?}");
}

/// A failing tagged listing costs nothing but the trust: `ls-files -z --deduplicate` is asked at the same position
/// and the run's output is unchanged; when that fails too, its error is pre-commit's (`CalledProcessError` naming
/// `ls-files -z --deduplicate`).
#[test]
fn a_failing_tagged_listing_falls_back_to_pre_commits_query() {
    let mut h = committed(&[("a.py", b"print(1)\n")]);
    h.config(&local(&lister("files", "")));
    let rec = recorded(&mut h, Some(TAGGED));
    let o = h.run(all());
    assert_eq!(o.code, 1, "{o:?}");
    assert_eq!(
        listed(&o.stdout, "files"),
        v(&[".pre-commit-config.yaml", "a.py"])
    );
    let before = before_hook(&rec.calls());
    assert!(before.iter().any(|a| a == DEDUP), "{before:?}");
    let mut h = committed(&[("a.py", b"print(1)\n")]);
    h.config(&local(&lister("files", "")));
    let rec = Rec::new(h.path(".hook-ran"), Some("ls-files"));
    let backend: Arc<dyn GitBackend> = rec;
    h.backend = Some(backend);
    let o = h.run(all());
    assert_eq!(o.code, 3, "{o:?}");
    match o.failure {
        Some(Failure::Exc { exc, .. }) => {
            assert_eq!(exc.ty, "CalledProcessError");
            let text = String::from_utf8_lossy(&exc.text);
            assert!(
                text.contains("'ls-files', '-z', '--deduplicate')\nreturn code: 128"),
                "{text}"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// D-CRASH-08 with trusted modes: a hook that runs `git rm` keeps `git diff` empty, yet the next hook does not get
/// the deleted file (the oracle cleared the trust after the first hook, so its first identification takes an
/// `lstat`). pre-commit crashes here (`ValueError: b.py does not exist.`); snp drops the file.
#[test]
fn a_deletion_that_keeps_the_diff_empty_is_seen() {
    let h = committed(&[("a.py", b"print(1)\n"), ("b.py", b"print(2)\n")]);
    h.config(&local(&format!(
        "{}{}",
        sys(
            "rm",
            "rm",
            "git rm -q b.py",
            "files: ^nomatch$\nalways_run: true\npass_filenames: false"
        ),
        lister("files", ""),
    )));
    let o = h.run(all());
    assert_eq!(o.code, 1, "{o:?}");
    assert!(!h.path("b.py").exists());
    assert_eq!(
        listed(&o.stdout, "files"),
        v(&[".pre-commit-config.yaml", "a.py"])
    );
}
