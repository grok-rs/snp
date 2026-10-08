//! Deterministic repository builder (10 §7.3.3).
//!
//! Owner: WP-18.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use snp_core::env::Env;

use crate::harness::{OrFail, git, make_executable, mkdirs, write};

/// One pending step.
#[derive(Debug)]
enum Op {
    File {
        path: String,
        bytes: Vec<u8>,
        exec: bool,
    },
    Symlink {
        path: String,
        target: String,
    },
    Stage(String),
    StageAll,
    Commit(String),
    Tag(String),
    Config(String, String),
}

/// Deterministic repository builder (identical SHAs across runs: fixed dates, author, umask, core.autocrlf=false).
///
/// Steps are recorded and run in order by [`RepoBuilder::build`]: the repository is created with
/// `git -c init.defaultBranch=master init` (pre-commit's `git_dir`) unless it already exists; every git call gets the
/// next tick of the [`crate::TestEnv`]'s date counter as `GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE`; file modes come
/// from the builder (644/755/120000), never from the umask.
#[derive(Debug)]
pub struct RepoBuilder {
    path: PathBuf,
    env: Env,
    dates: Arc<AtomicU64>,
    ops: Vec<Op>,
}

impl RepoBuilder {
    pub(crate) fn new(path: PathBuf, env: Env, dates: Arc<AtomicU64>) -> Self {
        Self {
            path,
            env,
            dates,
            ops: Vec::new(),
        }
    }

    fn push(mut self, op: Op) -> Self {
        self.ops.push(op);
        self
    }

    /// Writes a regular file (mode 100644 in the index).
    #[must_use]
    pub fn file(self, path: &str, bytes: &[u8]) -> Self {
        self.push(Op::File {
            path: path.to_owned(),
            bytes: bytes.to_vec(),
            exec: false,
        })
    }

    /// Writes an executable file (mode 100755 in the index on every OS).
    #[must_use]
    pub fn exec_file(self, path: &str, bytes: &[u8]) -> Self {
        self.push(Op::File {
            path: path.to_owned(),
            bytes: bytes.to_vec(),
            exec: true,
        })
    }

    /// Creates a symlink (mode 120000 in the index on every OS; on Windows without symlink privileges the link is
    /// recorded in the index only, its worktree file holding the target).
    #[must_use]
    pub fn symlink(self, path: &str, target: &str) -> Self {
        self.push(Op::Symlink {
            path: path.to_owned(),
            target: target.to_owned(),
        })
    }

    /// `git add -- <path>`.
    #[must_use]
    pub fn stage(self, path: &str) -> Self {
        self.push(Op::Stage(path.to_owned()))
    }

    /// `git add -A`.
    #[must_use]
    pub fn stage_all(self) -> Self {
        self.push(Op::StageAll)
    }

    /// `git commit --no-gpg-sign --allow-empty -m <msg>` with the next fixed date.
    #[must_use]
    pub fn commit(self, msg: &str) -> Self {
        self.push(Op::Commit(msg.to_owned()))
    }

    /// `git tag <name>` (lightweight).
    #[must_use]
    pub fn tag(self, name: &str) -> Self {
        self.push(Op::Tag(name.to_owned()))
    }

    /// `git config <key> <value>`.
    #[must_use]
    pub fn config(self, key: &str, value: &str) -> Self {
        self.push(Op::Config(key.to_owned(), value.to_owned()))
    }

    fn git<S: AsRef<OsStr>>(&self, args: &[S]) -> Vec<u8> {
        git(&self.env, &self.path, args, &self.dates)
    }

    /// Runs the pending operations; returns the repository path.
    pub fn build(self) -> PathBuf {
        mkdirs(&self.path);
        if !self.path.join(".git").exists() {
            self.git(&["-c", "init.defaultBranch=master", "init", "-q", "."]);
        }
        // Paths whose index entry needs an explicit mode (Windows has neither an exec bit nor, usually, symlinks).
        let mut exec_paths: Vec<String> = Vec::new();
        let mut link_paths: Vec<String> = Vec::new();
        for op in &self.ops {
            match op {
                Op::File { path, bytes, exec } => {
                    let p = self.path.join(path);
                    if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
                        std::fs::remove_file(&p).or_fail("replacing a symlink");
                    }
                    write(&p, bytes);
                    if *exec {
                        make_executable(&p);
                        exec_paths.push(path.clone());
                    }
                }
                Op::Symlink { path, target } => {
                    let p = self.path.join(path);
                    if let Some(parent) = p.parent() {
                        mkdirs(parent);
                    }
                    if !make_symlink(target, &p) {
                        write(&p, target.as_bytes());
                        link_paths.push(path.clone());
                    }
                }
                Op::Stage(path) => {
                    self.git(&["add", "--", path.as_str()]);
                    self.fix_modes(&exec_paths, &link_paths, Some(path));
                }
                Op::StageAll => {
                    self.git(&["add", "-A"]);
                    self.fix_modes(&exec_paths, &link_paths, None);
                }
                Op::Commit(msg) => {
                    self.git(&[
                        "commit",
                        "-q",
                        "--no-gpg-sign",
                        "--allow-empty",
                        "-m",
                        msg.as_str(),
                    ]);
                }
                Op::Tag(name) => {
                    self.git(&["tag", name.as_str()]);
                }
                Op::Config(k, v) => {
                    self.git(&["config", k.as_str(), v.as_str()]);
                }
            }
        }
        self.path
    }

    /// Windows: the index modes that the filesystem cannot carry (`update-index --chmod=+x`, symlink blobs as
    /// 120000 `--cacheinfo` entries). Unix needs nothing: git reads the exec bit and the links themselves.
    fn fix_modes(&self, exec: &[String], links: &[String], only: Option<&String>) {
        let wanted = |p: &&String| only.is_none_or(|o| o == *p);
        for p in exec.iter().filter(wanted) {
            if cfg!(windows) {
                self.git(&["update-index", "--chmod=+x", "--", p.as_str()]);
            }
        }
        for p in links.iter().filter(wanted) {
            let sha = self.git(&["hash-object", "-w", "--", p.as_str()]);
            let sha = String::from_utf8_lossy(&sha).trim().to_owned();
            self.git(&[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("120000,{sha},{p}"),
            ]);
        }
    }
}

/// A real symlink; `false` when the platform refuses (Windows without the privilege).
fn make_symlink(target: &str, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).or_fail("creating a symlink");
        true
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _: (&str, &Path) = (target, link);
        false
    }
}
