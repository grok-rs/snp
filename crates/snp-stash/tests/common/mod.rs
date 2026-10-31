//! A hermetic git sandbox for snp-stash's tests (the environment mirrors 10 §7.3.2): no system/global config leaks,
//! fixed identity, `master` branch; a capturing `Printer`; a scriptable backend with call log and side effects.

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::clock::{Clock, SystemClock};
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::output::{Printer, PrinterOpts};
use snp_git::{Git, GitBackend, GitExe, ProcBackend, RepoInfo};
use snp_proc::spawn::{Captured, Group, PyStatus, Role, SpawnError, Stdin};
use snp_stash::StashCtx;

/// `FOO_CONTENTS` of `tests/staged_files_only_test.py`.
pub(crate) const FOO: &str = "1\n2\n3\n4\n5\n6\n7\n8\n";

/// A shared in-memory writer.
#[derive(Clone, Debug, Default)]
pub(crate) struct Buf(pub Arc<Mutex<Vec<u8>>>);

impl Buf {
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for Buf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        snp_core::sync::lock(&self.0).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A writer that always fails with EPIPE (X13-STASH-03).
#[derive(Debug)]
pub(crate) struct Broken;

impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

/// A clock with a fixed `unix_secs`.
#[derive(Debug)]
pub(crate) struct FixedClock(pub u64);

impl Clock for FixedClock {
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }

    fn unix_secs(&self) -> u64 {
        self.0
    }
}

pub(crate) struct Sandbox {
    _tmp: tempfile::TempDir,
    pub(crate) root: PathBuf,
    pub(crate) env: Env,
    dates: AtomicU64,
}

impl Sandbox {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = snp_pycompat::ospath::realpath(dir.path().as_os_str(), dir.path()).unwrap();
        let home = root.join("home");
        let template = root.join("git-template");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&template).unwrap();
        let gitconfig = home.join(".gitconfig");
        std::fs::write(
            &gitconfig,
            "[init]\n\tdefaultBranch = master\n[user]\n\tname = test\n\temail = test@example.com\n\
             [commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n[core]\n\tautocrlf = false\n\
             [protocol \"file\"]\n\tallow = always\n[advice]\n\tdetachedHead = false\n[safe]\n\tdirectory = *\n",
        )
        .unwrap();
        let process = Env::capture_process();
        let mut pairs: Vec<(OsString, OsString)> = Vec::new();
        for k in [
            "PATH",
            "SYSTEMROOT",
            "PATHEXT",
            "COMSPEC",
            "WINDIR",
            "TEMP",
            "TMP",
        ] {
            if let Some(v) = process.get(k) {
                pairs.push((k.into(), v.to_os_string()));
            }
        }
        for (k, v) in [
            ("HOME", home.as_os_str()),
            ("USERPROFILE", home.as_os_str()),
            ("GIT_CONFIG_GLOBAL", gitconfig.as_os_str()),
            ("GIT_TEMPLATE_DIR", template.as_os_str()),
        ] {
            pairs.push((k.into(), v.to_os_string()));
        }
        for (k, v) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "test"),
            ("GIT_COMMITTER_NAME", "test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
            ("GIT_ALLOW_PROTOCOL", "file"),
            ("LC_ALL", "C"),
            ("LANG", "C"),
            ("TZ", "UTC"),
        ] {
            pairs.push((k.into(), v.into()));
        }
        Self {
            _tmp: dir,
            root,
            env: Env::from_pairs(pairs),
            dates: AtomicU64::new(0),
        }
    }

    pub(crate) fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn command<S: AsRef<OsStr>>(&self, cwd: &Path, args: &[S]) -> Command {
        let mut c = match snp_proc::resolve::find_executable(OsStr::new("git"), &self.env, cwd) {
            Some(exe) => Command::new(exe),
            None => Command::new("git"),
        };
        self.env.apply_to(&mut c);
        let n = self.dates.fetch_add(1, Ordering::SeqCst);
        let date = format!("{} +0000", 1_577_836_800 + n);
        c.env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date);
        c.args(args).current_dir(cwd);
        c
    }

    /// Runs git, returning (status, stdout, stderr).
    pub(crate) fn git_status<S: AsRef<OsStr>>(
        &self,
        cwd: &Path,
        args: &[S],
    ) -> (i32, Vec<u8>, Vec<u8>) {
        let out = self.command(cwd, args).output().unwrap();
        (out.status.code().unwrap_or(-1), out.stdout, out.stderr)
    }

    /// Runs git; panics on failure; returns stdout.
    pub(crate) fn git<S: AsRef<OsStr>>(&self, cwd: &Path, args: &[S]) -> Vec<u8> {
        let (code, out, err) = self.git_status(cwd, args);
        let shown: Vec<String> = args
            .iter()
            .map(|a| a.as_ref().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            code,
            0,
            "git {shown:?} failed: {}",
            String::from_utf8_lossy(&err)
        );
        out
    }

    pub(crate) fn git_text<S: AsRef<OsStr>>(&self, cwd: &Path, args: &[S]) -> String {
        String::from_utf8(self.git(cwd, args))
            .unwrap()
            .trim()
            .to_owned()
    }

    /// `git init` in `<root>/<name>`.
    pub(crate) fn init(&self, name: &str) -> PathBuf {
        let p = self.path(name);
        std::fs::create_dir_all(&p).unwrap();
        self.git(&p, &["init", "-q", "."]);
        p
    }

    /// pre-commit's `git_commit`.
    pub(crate) fn commit(&self, repo: &Path, msg: &str) {
        self.git(
            repo,
            &[
                "commit",
                "-q",
                "--allow-empty",
                "--no-gpg-sign",
                "-a",
                "-m",
                msg,
            ],
        );
    }

    #[expect(
        clippy::unused_self,
        reason = "a method for symmetry with the other sandbox helpers"
    )]
    pub(crate) fn write(&self, repo: &Path, rel: &str, bytes: &[u8]) {
        let p = repo.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, bytes).unwrap();
    }

    #[expect(
        clippy::unused_self,
        reason = "a method for symmetry with the other sandbox helpers"
    )]
    pub(crate) fn read(&self, repo: &Path, rel: &str) -> Vec<u8> {
        std::fs::read(repo.join(rel)).unwrap()
    }

    /// `{path: XY}` of `git status --porcelain` (pre-commit's `get_short_git_status`).
    pub(crate) fn status(&self, repo: &Path) -> Vec<(String, String)> {
        let out = String::from_utf8(self.git(
            repo,
            &[
                "status",
                "--porcelain",
                "--untracked-files=no",
                "--no-renames",
            ],
        ))
        .unwrap();
        out.lines()
            .map(|l| {
                (
                    l.get(3..).unwrap().to_owned(),
                    l.get(..2).unwrap().trim().to_owned(),
                )
            })
            .collect()
    }

    pub(crate) fn status_of(&self, repo: &Path, path: &str) -> String {
        self.status(repo)
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, s)| s)
            .unwrap_or_default()
    }

    /// `git ls-files -s` (the index, K3).
    pub(crate) fn index(&self, repo: &Path) -> String {
        String::from_utf8(self.git(repo, &["ls-files", "-s"])).unwrap()
    }

    /// The unstaged diff bytes (`git diff --binary` worktree vs index).
    pub(crate) fn unstaged_diff(&self, repo: &Path) -> Vec<u8> {
        self.git(repo, &["diff", "--binary", "--no-ext-diff", "--no-color"])
    }

    pub(crate) fn exe(&self) -> Arc<GitExe> {
        Arc::new(GitExe::resolve(&self.env, &self.root, None).unwrap())
    }

    /// Discovery with a real Tool backend; then the stash's Critical handle.
    pub(crate) fn discover(&self, repo: &Path) -> (Git, RepoInfo) {
        let tool: Arc<dyn GitBackend> = Arc::new(ProcBackend {
            cancel: CancelToken::new(),
            role: Role::Tool,
        });
        let (git, info) = Git::discover(self.exe(), &self.env, repo, tool).unwrap();
        let git = git.with_brand(Brand::PreCommit);
        let critical: Arc<dyn GitBackend> = Arc::new(ProcBackend {
            cancel: CancelToken::new(),
            role: Role::Critical,
        });
        (git.with_backend(critical), info)
    }

    /// Discovery with `backend` for every later call.
    pub(crate) fn discover_with(
        &self,
        repo: &Path,
        backend: Arc<dyn GitBackend>,
    ) -> (Git, RepoInfo) {
        let (git, info) = self.discover(repo);
        (git.with_backend(backend), info)
    }

    /// The store's patches dir.
    pub(crate) fn patches(&self) -> PathBuf {
        self.root.join("store").join("patches")
    }

    /// A stash context on `repo` with a capturing printer; returns (ctx, stdout, stderr).
    pub(crate) fn ctx(&self, repo: &Path) -> (StashCtx, Buf, Buf) {
        let (git, info) = self.discover(repo);
        self.ctx_with(git, info)
    }

    pub(crate) fn ctx_with(&self, git: Git, info: RepoInfo) -> (StashCtx, Buf, Buf) {
        let out = Buf::default();
        let err = Buf::default();
        let printer = Arc::new(printer(
            Box::new(out.clone()),
            Box::new(err.clone()),
            Brand::PreCommit,
            false,
        ));
        (
            StashCtx {
                git,
                info,
                patches_dir: self.patches(),
                printer,
                cancel: CancelToken::new(),
                clock: Arc::new(SystemClock),
                signals: None,
                lock_timeout: Duration::from_secs(5),
                verify: false,
                old_git: false,
            },
            out,
            err,
        )
    }

    /// pre-commit's `foo_staged` fixture: `foo` = `FOO_CONTENTS`, added (no commit).
    pub(crate) fn foo_staged(&self, name: &str) -> PathBuf {
        let repo = self.init(name);
        self.write(&repo, "foo", FOO.as_bytes());
        self.git(&repo, &["add", "foo"]);
        repo
    }

    /// The files in the patches dir (sorted names).
    pub(crate) fn patch_files(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.patches())
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }
}

pub(crate) fn printer(
    out: Box<dyn Write + Send>,
    err: Box<dyn Write + Send>,
    brand: Brand,
    color: bool,
) -> Printer {
    Printer::new(
        out,
        err,
        PrinterOpts {
            brand,
            color,
            stdout_is_tty: false,
            windows_text_mode: false,
            cancel: CancelToken::new(),
        },
    )
}

/// The `(type, text, hint)` of an exception failure.
pub(crate) fn exc(f: &Failure) -> (String, String, Option<String>) {
    match f {
        Failure::Exc { exc, .. } => (
            exc.ty.to_string(),
            String::from_utf8_lossy(&exc.text).into_owned(),
            exc.hint.clone(),
        ),
        other => panic!("not an exception: {other:?}"),
    }
}

/// A side effect run before a matching call.
pub(crate) type Effect = Box<dyn Fn(&[OsString]) + Send + Sync>;

/// What a rule answers.
#[derive(Clone, Debug)]
pub(crate) enum Reply {
    /// Run the real git.
    Real,
    /// Exit with this code and output.
    Exit(i64, Vec<u8>, Vec<u8>),
    /// Exit like a git stopped by SIGTTIN (group killed): -9 and `stopped_by_tty`.
    TtyStop,
}

/// `(argument prefix, only the n-th match, matches seen, action)`.
type Rule<T> = (Vec<String>, Option<usize>, usize, T);

/// A real Critical backend with argument-prefix rules (`n`-th match only, or every match), side effects and a call
/// log.
pub(crate) struct Spy {
    real: ProcBackend,
    rules: Mutex<Vec<Rule<Reply>>>,
    effects: Mutex<Vec<Rule<Effect>>>,
    calls: Mutex<Vec<Vec<OsString>>>,
}

impl std::fmt::Debug for Spy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Spy")
    }
}

fn args_of(argv: &[OsString]) -> Vec<String> {
    argv.iter()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn matches(prefix: &[String], args: &[String]) -> bool {
    args.len() >= prefix.len() && args.iter().zip(prefix).all(|(a, b)| a == b)
}

impl Spy {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            real: ProcBackend {
                cancel: CancelToken::new(),
                role: Role::Critical,
            },
            rules: Mutex::new(Vec::new()),
            effects: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        })
    }

    /// Calls starting with `prefix` (arguments after argv[0]) answer `reply`; `nth` = only the n-th match (1-based).
    pub(crate) fn on(&self, prefix: &[&str], nth: Option<usize>, reply: Reply) {
        self.rules.lock().unwrap().push((
            prefix.iter().map(|s| (*s).to_owned()).collect(),
            nth,
            0,
            reply,
        ));
    }

    /// Runs `f` before calls starting with `prefix` (`nth` as in `on`).
    pub(crate) fn before(&self, prefix: &[&str], nth: Option<usize>, f: Effect) {
        self.effects.lock().unwrap().push((
            prefix.iter().map(|s| (*s).to_owned()).collect(),
            nth,
            0,
            f,
        ));
    }

    /// The git arguments of every call (argv[0] dropped), joined by spaces.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|a| args_of(a).join(" "))
            .collect()
    }

    fn react(&self, argv: &[OsString]) -> Reply {
        self.calls.lock().unwrap().push(argv.to_vec());
        let args = args_of(argv);
        {
            let mut effects = self.effects.lock().unwrap();
            for (prefix, nth, seen, f) in effects.iter_mut() {
                if matches(prefix, &args) {
                    *seen += 1;
                    if nth.is_none_or(|n| n == *seen) {
                        f(argv);
                    }
                }
            }
        }
        let mut rules = self.rules.lock().unwrap();
        let mut reply = Reply::Real;
        for (prefix, nth, seen, r) in rules.iter_mut() {
            if matches(prefix, &args) {
                *seen += 1;
                if nth.is_none_or(|n| n == *seen) {
                    reply = r.clone();
                }
            }
        }
        reply
    }
}

impl GitBackend for Spy {
    fn capture(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
        group: Group,
    ) -> Result<Captured, SpawnError> {
        match self.react(argv) {
            Reply::Real => self.real.capture(argv, env, cwd, stdin, group),
            Reply::Exit(code, stdout, stderr) => Ok(Captured {
                status: PyStatus(code),
                stdout,
                stderr,
                stopped_by_tty: false,
            }),
            Reply::TtyStop => Ok(Captured {
                status: PyStatus(-9),
                stdout: Vec::new(),
                stderr: Vec::new(),
                stopped_by_tty: true,
            }),
        }
    }

    fn stream(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        sink: &mut dyn Write,
    ) -> Result<(PyStatus, Vec<u8>), SpawnError> {
        match self.react(argv) {
            Reply::Real => self.real.stream(argv, env, cwd, sink),
            Reply::Exit(code, stdout, stderr) => {
                sink.write_all(&stdout).map_err(|io| SpawnError {
                    program: argv.first().cloned().unwrap_or_default(),
                    io,
                })?;
                Ok((PyStatus(code), stderr))
            }
            Reply::TtyStop => Ok((PyStatus(-9), Vec::new())),
        }
    }

    fn inherit(
        &self,
        argv: &[OsString],
        env: &Env,
        cwd: &Path,
        stdin: Stdin,
    ) -> Result<PyStatus, SpawnError> {
        match self.react(argv) {
            Reply::Real => self.real.inherit(argv, env, cwd, stdin),
            Reply::Exit(code, ..) => Ok(PyStatus(code)),
            Reply::TtyStop => Ok(PyStatus(-9)),
        }
    }
}
