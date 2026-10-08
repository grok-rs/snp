//! A hermetic git sandbox for snp-git's own tests (snp-git is an ancestor of snp-testkit, so it cannot use it;
//! the environment mirrors 10 §7.3.2): no system/global config leaks, fixed identity and dates, `master` branch.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use snp_core::env::Env;
use snp_git::{Git, GitBackend, GitExe, ProcBackend, RepoInfo};
use snp_proc::spawn::Role;

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

    fn command(&self, cwd: &Path, args: &[&OsStr]) -> Command {
        // `Command::new("git")` would search the parent's PATH: resolve against the sandbox PATH instead.
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

    /// Runs git with extra environment variables; panics on failure; returns stdout.
    pub(crate) fn git_with(&self, cwd: &Path, args: &[&str], vars: &[(&str, &str)]) -> Vec<u8> {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        let mut c = self.command(cwd, &args);
        c.envs(vars.iter().copied());
        let out = c.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    /// Runs git, returning (status, stdout, stderr).
    pub(crate) fn git_status<S: AsRef<OsStr>>(
        &self,
        cwd: &Path,
        args: &[S],
    ) -> (i32, Vec<u8>, Vec<u8>) {
        let args: Vec<&OsStr> = args.iter().map(AsRef::as_ref).collect();
        let out = self.command(cwd, &args).output().unwrap();
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

    /// `git -c init.defaultBranch=master init <name>` (pre-commit's `git_dir`).
    pub(crate) fn init(&self, name: &str) -> PathBuf {
        let p = self.path(name);
        std::fs::create_dir_all(&p).unwrap();
        self.git(&p, &["init", "-q", "."]);
        p
    }

    /// pre-commit's `git_commit`: `git commit --allow-empty --no-gpg-sign -a -m <msg>`.
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

    pub(crate) fn head(&self, repo: &Path) -> String {
        self.git_text(repo, &["rev-parse", "HEAD"])
    }

    pub(crate) fn exe(&self) -> Arc<GitExe> {
        Arc::new(GitExe::resolve(&self.env, &self.root, None).unwrap())
    }

    #[expect(
        clippy::unused_self,
        reason = "a method for symmetry with the other sandbox helpers"
    )]
    pub(crate) fn backend(&self) -> Arc<dyn GitBackend> {
        Arc::new(ProcBackend {
            cancel: snp_core::cancel::CancelToken::new(),
            role: Role::Tool,
        })
    }

    pub(crate) fn discover(&self, cwd: &Path) -> (Git, RepoInfo) {
        Git::discover(self.exe(), &self.env, cwd, self.backend()).unwrap()
    }

    /// `in_merge_conflict` (`tests/conftest.py:53-84`): a consuming repo (config committed) + `placeholder`, cloned;
    /// in the clone branches `foo`/`bar` conflict on `conflict_file` and `git merge foo` stops. Returns the clone.
    pub(crate) fn merge_conflict(&self, name: &str) -> PathBuf {
        let origin = self.init(&format!("{name}-origin"));
        self.write(&origin, ".pre-commit-config.yaml", b"repos: []\n");
        self.git(&origin, &["add", "."]);
        self.commit(&origin, "add_config_to_repo");
        self.write(&origin, "placeholder", b"");
        self.git(&origin, &["add", "placeholder"]);
        self.commit(&origin, "in_merge_conflict");
        let clone = self.path(name);
        self.git(
            &self.root,
            &[
                OsStr::new("clone"),
                OsStr::new("-q"),
                origin.as_os_str(),
                clone.as_os_str(),
            ],
        );
        self.make_conflict(&clone);
        clone
    }

    /// `_make_conflict` (`tests/conftest.py:53-70`).
    pub(crate) fn make_conflict(&self, repo: &Path) {
        self.git(repo, &["checkout", "-q", "origin/master", "-b", "foo"]);
        self.write(repo, "conflict_file", b"herp\nderp\n");
        self.git(repo, &["add", "conflict_file"]);
        self.write(repo, "foo_only_file", b"foo");
        self.git(repo, &["add", "foo_only_file"]);
        self.commit(repo, "_make_conflict");
        self.git(repo, &["checkout", "-q", "origin/master", "-b", "bar"]);
        self.write(repo, "conflict_file", b"harp\nddrp\n");
        self.git(repo, &["add", "conflict_file"]);
        self.write(repo, "bar_only_file", b"bar");
        self.git(repo, &["add", "bar_only_file"]);
        self.commit(repo, "_make_conflict");
        let (code, _, _) = self.git_status(repo, &["merge", "foo"]);
        assert_ne!(code, 0, "the merge must conflict");
    }

    /// `resolve_conflict` (`tests/git_test.py`).
    pub(crate) fn resolve_conflict(&self, repo: &Path) {
        self.write(repo, "conflict_file", b"herp\nderp\n");
        self.git(repo, &["add", "conflict_file"]);
    }

    /// `in_conflicting_submodule`: repo1 gets repo2 as submodule `sub`; the conflict is made inside `sub`.
    pub(crate) fn conflicting_submodule(&self, name: &str) -> PathBuf {
        let one = self.init(&format!("{name}-1"));
        let two = self.init(&format!("{name}-2"));
        self.commit(&two, "in_conflicting_submodule");
        self.git(
            &one,
            &[
                OsStr::new("submodule"),
                OsStr::new("add"),
                OsStr::new("-q"),
                two.as_os_str(),
                OsStr::new("sub"),
            ],
        );
        let sub = one.join("sub");
        self.make_conflict(&sub);
        sub
    }
}

pub(crate) fn zl(z: &snp_git::ZList) -> Vec<Vec<u8>> {
    z.iter().map(<[u8]>::to_vec).collect()
}

pub(crate) fn strs(xs: &[&str]) -> Vec<Vec<u8>> {
    xs.iter().map(|s| s.as_bytes().to_vec()).collect()
}

pub(crate) fn sorted(mut v: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    v.sort();
    v
}

/// The `(type, text)` of an exception failure.
pub(crate) fn exc(f: &snp_core::failure::Failure) -> (String, String) {
    match f {
        snp_core::failure::Failure::Exc { exc, .. } => (
            exc.ty.to_string(),
            String::from_utf8_lossy(&exc.text).into_owned(),
        ),
        other => panic!("not an exception: {other:?}"),
    }
}

/// `c * n` (`str::repeat` is a disallowed method).
pub(crate) fn rep(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}
