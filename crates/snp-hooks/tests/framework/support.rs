//! Shared test support for snp-hooks' integration tests (included by every test target with `#[path]`): a hermetic
//! git sandbox mirroring the environment `cargo xtask pch-goldens` runs the Python oracles in, the case steps of
//! `tests/*/cases.json`, latin-1 byte strings and a `HookCx` over a real repository.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_git::pch_facts::PchFacts;
use snp_git::{Git, GitBackend, GitExe, ProcBackend};
use snp_hooks::{Epoch, HookCx};
use snp_proc::spawn::Role;

/// The bytes of a list of paths after a run (`None`: missing).
pub(crate) type Files = Vec<(String, Option<Vec<u8>>)>;

/// A latin-1 JSON string as bytes (one char per byte).
pub(crate) fn latin1(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).expect("latin-1 string"))
        .collect()
}

/// Bytes as a latin-1 string.
pub(crate) fn to_latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// An `OsString` of raw bytes.
#[cfg(unix)]
pub(crate) fn os(b: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStrExt as _;
    OsStr::from_bytes(b).to_os_string()
}

/// An `OsString` of raw bytes (UTF-8 on Windows).
#[cfg(not(unix))]
pub(crate) fn os(b: &[u8]) -> OsString {
    OsString::from(String::from_utf8(b.to_vec()).expect("UTF-8 path"))
}

/// The manifest directory of snp-hooks.
pub(crate) fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A JSON file under `tests/`.
pub(crate) fn json(rel: &str) -> serde_json::Value {
    let path = crate_dir().join("tests").join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

/// A hermetic directory with a git identity, fixed dates and `master` (the driver's `hermetic_env`).
pub(crate) struct Sandbox {
    _tmp: tempfile::TempDir,
    /// The work tree.
    pub(crate) root: PathBuf,
    /// The environment of git and of the hooks.
    pub(crate) env: Env,
}

impl Sandbox {
    /// A sandbox whose work tree is a fresh `git init` (when `git_init`).
    pub(crate) fn new(git_init: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = snp_pycompat::ospath::realpath(dir.path().as_os_str(), dir.path()).unwrap();
        let home = base.join("home");
        let template = home.join("git-template");
        std::fs::create_dir_all(&template).unwrap();
        let gitconfig = home.join(".gitconfig");
        std::fs::write(
            &gitconfig,
            "[init]\n\tdefaultBranch = master\n[user]\n\tname = test\n\temail = test@example.com\n\
             [commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n[core]\n\tautocrlf = false\n\
             [advice]\n\tdetachedHead = false\n[safe]\n\tdirectory = *\n",
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
            ("GIT_AUTHOR_DATE", "1577836800 +0000"),
            ("GIT_COMMITTER_DATE", "1577836800 +0000"),
            ("LC_ALL", "C.UTF-8"),
            ("LANG", "C.UTF-8"),
            ("TZ", "UTC"),
            ("PRE_COMMIT", "1"),
        ] {
            pairs.push((k.into(), v.into()));
        }
        let root = base.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let sb = Self {
            _tmp: dir,
            root,
            env: Env::from_pairs(pairs),
        };
        if git_init {
            sb.git(&["init", "-q", "."]);
        }
        sb
    }

    /// Runs git in the work tree; panics on failure.
    pub(crate) fn git<S: AsRef<OsStr>>(&self, args: &[S]) {
        let exe = snp_proc::resolve::find_executable(OsStr::new("git"), &self.env, &self.root)
            .unwrap_or_else(|| PathBuf::from("git"));
        let mut c = Command::new(exe);
        self.env.apply_to(&mut c);
        let out = c.args(args).current_dir(&self.root).output().unwrap();
        assert!(
            out.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A path under the work tree from latin-1 bytes.
    pub(crate) fn path(&self, rel: &str) -> PathBuf {
        self.root.join(os(&latin1(rel)))
    }

    /// Writes a file (creating parents).
    pub(crate) fn write(&self, rel: &str, bytes: &[u8]) {
        let p = self.path(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, bytes).unwrap();
    }

    /// `chmod mode`.
    #[cfg(unix)]
    pub(crate) fn chmod(&self, rel: &str, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(self.path(rel), std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// `chmod mode` (Windows: read-only for 0o444, else nothing).
    #[cfg(not(unix))]
    pub(crate) fn chmod(&self, rel: &str, mode: u32) {
        let mut p = std::fs::metadata(self.path(rel)).unwrap().permissions();
        p.set_readonly(mode & 0o222 == 0);
        std::fs::set_permissions(self.path(rel), p).unwrap();
    }

    /// Applies the steps of a case (`w`, `x`, `ro`, `mkdir`, `git`, `gitfile`, `ln`); returns the written paths in
    /// first-write order.
    pub(crate) fn steps(&self, steps: &serde_json::Value) -> Vec<String> {
        let mut written: Vec<String> = Vec::new();
        for step in steps.as_array().into_iter().flatten() {
            let step: Vec<&str> = step
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            match step.as_slice() {
                ["w", path, bytes] => {
                    self.write(path, &latin1(bytes));
                    if !written.iter().any(|w| w == path) {
                        written.push((*path).to_owned());
                    }
                }
                ["x", path] => self.chmod(path, 0o755),
                ["ro", path] => self.chmod(path, 0o444),
                ["mkdir", path] => std::fs::create_dir_all(self.path(path)).unwrap(),
                ["git", args @ ..] => self.git(args),
                ["gitfile", name, bytes] => {
                    let p = self.root.join(".git").join(name);
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(p, latin1(bytes)).unwrap();
                }
                #[cfg(unix)]
                ["ln", target, path] => {
                    std::os::unix::fs::symlink(target, self.path(path)).unwrap();
                }
                other => panic!("unknown step {other:?}"),
            }
        }
        written
    }

    /// The current bytes of each path (`None` when missing).
    pub(crate) fn snapshot(&self, paths: &[String]) -> Files {
        paths
            .iter()
            .map(|p| (p.clone(), std::fs::read(self.path(p)).ok()))
            .collect()
    }

    /// The git handle and pre-commit-hooks facts of the work tree.
    pub(crate) fn git_handle(&self) -> (Git, PchFacts) {
        let exe = Arc::new(GitExe::resolve(&self.env, &self.root, None).unwrap());
        let backend: Arc<dyn GitBackend> = Arc::new(ProcBackend {
            cancel: CancelToken::new(),
            role: Role::Tool,
        });
        let (git, _) = Git::discover(exe, &self.env, &self.root, backend).unwrap();
        let facts = PchFacts::new(git.clone());
        (git, facts)
    }
}

/// Everything a `HookCx` borrows.
pub(crate) struct Ctx {
    /// The sandbox.
    pub(crate) sb: Sandbox,
    git: Git,
    facts: PchFacts,
    cancel: CancelToken,
}

impl Ctx {
    /// A context over `sb` (a git work tree).
    pub(crate) fn new(sb: Sandbox) -> Self {
        let (git, facts) = sb.git_handle();
        Self {
            sb,
            git,
            facts,
            cancel: CancelToken::new(),
        }
    }

    /// The cancellation token the hooks see.
    pub(crate) fn cancel(&self) -> &CancelToken {
        &self.cancel
    }

    /// The hook context at `epoch` (80 columns, the host platform).
    pub(crate) fn cx(&self, epoch: Epoch) -> HookCx<'_> {
        HookCx {
            root: &self.sb.root,
            env: &self.sb.env,
            git: &self.git,
            color: false,
            epoch,
            windows: cfg!(windows),
            columns: 80,
            facts: &self.facts,
            cancel: &self.cancel,
        }
    }
}

/// `argv` strings of a JSON array (UTF-8 strings, as Python's `str` argv).
pub(crate) fn argv_of(v: &serde_json::Value) -> Vec<OsString> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|a| OsString::from(a.as_str().unwrap()))
        .collect()
}

/// A short, readable rendering of bytes for failure messages.
pub(crate) fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The hook epoch of `hook` at `tag` (the generated per-tag profile).
pub(crate) fn epoch_at(tag: &str, hook: &str) -> Option<Epoch> {
    let profile = snp_hooks_data::profile_for_rev(tag)?;
    profile
        .hooks
        .iter()
        .find(|(id, _, _)| *id == hook)
        .map(|(_, _, e)| *e)
}

/// `true` when the path exists relative to `root`.
pub(crate) fn exists(root: &Path, rel: &str) -> bool {
    root.join(os(&latin1(rel))).exists()
}

/// References every helper: each test target uses a subset of this module and calls this once, so none of them
/// sees dead code.
pub(crate) fn link_all() {
    let _: fn(&str) -> Vec<u8> = latin1;
    let _: fn(&[u8]) -> String = to_latin1;
    let _: fn(&[u8]) -> OsString = os;
    let _: fn() -> PathBuf = crate_dir;
    let _: fn(&str) -> serde_json::Value = json;
    let _: fn(&serde_json::Value) -> Vec<OsString> = argv_of;
    let _: fn(&[u8]) -> String = show;
    let _: fn(&str, &str) -> Option<Epoch> = epoch_at;
    let _: fn(&Path, &str) -> bool = exists;
    let _: fn(bool) -> Sandbox = Sandbox::new;
    let _: fn(Sandbox) -> Ctx = Ctx::new;
    let _: fn(&Sandbox, &[&'static str]) = Sandbox::git::<&'static str>;
    let _: fn(&Sandbox, &str) -> PathBuf = Sandbox::path;
    let _: fn(&Sandbox, &str, &[u8]) = Sandbox::write;
    let _: fn(&Sandbox, &str, u32) = Sandbox::chmod;
    let _: fn(&Sandbox, &serde_json::Value) -> Vec<String> = Sandbox::steps;
    let _: fn(&Sandbox, &[String]) -> Files = Sandbox::snapshot;
    let _: fn(&Ctx) -> &CancelToken = Ctx::cancel;
    let _: for<'a> fn(&'a Ctx, Epoch) -> HookCx<'a> = Ctx::cx;
}
