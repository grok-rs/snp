//! snp-owned repositories: clones, local clones, autoupdate facts, the try-repo shadow (03 §7.6, §11.2, §12).
//!
//! Every call here runs with `no_git_env(base)` + the `GIT_CONFIG_COUNT` fsmonitor injection (02 G.7 rows 4–5,
//! ADR-018, D-SEC-05): an exported `GIT_DIR`/`GIT_INDEX_FILE`/`GIT_WORK_TREE` can never redirect them into the user's
//! repository (hazard c), and no fsmonitor daemon is ever started in a store or temp dir.
//!
//! Owner: WP-18.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::{Env, EnvPatch};
use snp_core::failure::Failure;
use snp_proc::spawn::Group;
use snp_pycompat::text::{bytes_strip, split_whitespace, strip};

use crate::run::{self, Out, os_from_bytes};
use crate::{Git, GitBackend, GitExe};

/// snp-owned repositories: env = `no_git_env(base)` + `GIT_CONFIG_COUNT`{core.fsmonitor=, core.useBuiltinFSMonitor=false}.
#[derive(Clone, Debug)]
pub struct OwnedRepo {
    exe: Arc<GitExe>,
    env: Env,
    dir: Arc<Path>,
    backend: Arc<dyn GitBackend>,
}

/// autoupdate facts of one repository (`autoupdate.RevInfo`, 03 §11.2).
#[derive(Clone, Debug)]
pub struct RevInfo {
    /// The repo URL.
    pub repo: String,
    /// The new rev.
    pub rev: String,
    /// The frozen sha (`--freeze`).
    pub frozen: Option<String>,
    /// Hook ids of the manifest at `rev`.
    pub hook_ids: Vec<String>,
}

/// Foreground clone (prompts allowed) or background prefetch (non-interactive).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CloneMode {
    /// pre-commit's env and prompts.
    Foreground,
    /// Background attempt, non-interactive (ADR-064).
    Prefetch,
}

/// `NO_FS_MONITOR` (`git.py:16`): kept in argv exactly where pre-commit has it.
const NO_FS_MONITOR: [&str; 2] = ["-c", "core.useBuiltinFSMonitor=false"];
/// `C.MANIFEST_FILE`.
const MANIFEST_FILE: &str = ".pre-commit-hooks.yaml";
/// `git.commit` identity (`git.py:196-202`).
const COMMIT_NAME: &str = "pre-commit";
const COMMIT_EMAIL: &str = "asottile+pre-commit@umich.edu";

fn os(s: &str) -> OsString {
    OsString::from(s)
}

/// A failed step of the clone sequence: its exception and git's stderr (for the D-SEC-06 retry decision).
struct StepError {
    failure: Failure,
    stderr: Vec<u8>,
}

impl From<Failure> for StepError {
    fn from(failure: Failure) -> Self {
        Self {
            failure,
            stderr: Vec::new(),
        }
    }
}

/// The object format a `mismatched algorithms` failure asks for (D-SEC-06): git's message is
/// `fatal: mismatched algorithms: client <a>; server <b>` (translated, but the algorithm names are `%s` arguments, so
/// the last name mentioned is the server's in every locale).
fn server_object_format(stderr: &[u8]) -> Option<&'static str> {
    let text = String::from_utf8_lossy(stderr);
    let english = text.contains("mismatched algorithms");
    let sha1 = text.rfind("sha1");
    let sha256 = text.rfind("sha256");
    if !english && (sha1.is_none() || sha256.is_none()) {
        return None;
    }
    match (sha1, sha256) {
        (Some(a), Some(b)) => Some(if a > b { "sha1" } else { "sha256" }),
        (Some(_), None) => Some("sha1"),
        (None, Some(_)) => Some("sha256"),
        (None, None) => None,
    }
}

impl OwnedRepo {
    fn new(exe: Arc<GitExe>, env: Env, dir: &Path, backend: Arc<dyn GitBackend>) -> Self {
        Self {
            exe,
            env,
            dir: Arc::from(dir),
            backend,
        }
    }

    /// pre-commit's exact clone sequence (03 §7.6) + one object-format retry on `mismatched algorithms` (D-SEC-06).
    /// `CloneMode::Foreground` = pre-commit's env and prompts; `Prefetch` (background attempts only, ADR-064) adds
    /// `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=` (empty: no askpass GUI), `GCM_INTERACTIVE=never` and, for ssh/scp URLs,
    /// `GIT_SSH_COMMAND='ssh -o BatchMode=yes'`; its stderr is never surfaced.
    ///
    /// `store.clone` (`store.py:132-210`): `init --template= <dest>` and `remote add origin <repo>` (`git.init_repo`,
    /// `<repo>` made absolute when it is a local directory), then `-c protocol.version=2 fetch origin <rev>
    /// --depth=1`, `checkout FETCH_HEAD`, `-c protocol.version=2 submodule update --init --recursive --depth=1`;
    /// any failure there falls back, in the same directory, to `fetch origin --tags`, `checkout <rev>`,
    /// `submodule update --init --recursive`, whose `CalledProcessError` is the error (X13-GIT-01). `dest` is
    /// owned by the caller (the store's scratch directory): it is never removed here, except that the D-SEC-06
    /// retry empties it before re-initialising.
    ///
    /// **Local repositories:** pre-commit resolves a relative `repo:` against its cwd, which `_adjust_args_and_chdir`
    /// has set to the project root. snp-git has no root here and never reads the process cwd, so the caller passes
    /// a local repository already joined to the project root (`ConfigSource::project_root`, the base
    /// `RepoId::from_config` uses) whenever `<root>/<repo>` is a directory; URLs are passed verbatim. A relative
    /// `repo` reaches `git remote add` unchanged.
    pub fn clone_rev(
        exe: Arc<GitExe>,
        base: &Env,
        dest: &Path,
        repo: &str,
        rev: &str,
        mode: CloneMode,
        b: Arc<dyn GitBackend>,
    ) -> Result<Self, Failure> {
        let mut env = owned_env(&exe, base);
        if mode == CloneMode::Prefetch {
            env = prefetch_env(&env, repo);
        }
        let owned = Self::new(exe, env, dest, b);
        let first = match owned.clone_sequence(repo, rev, None) {
            Ok(()) => return Ok(owned),
            Err(e) => e,
        };
        let Some(format) = server_object_format(&first.stderr) else {
            return Err(first.failure);
        };
        if !owned.exe.caps.init_object_format || owned.empty_dir().is_err() {
            return Err(first.failure);
        }
        owned
            .clone_sequence(repo, rev, Some(format))
            .map(|()| owned)
            .map_err(|e| e.failure)
    }

    /// Removes everything inside `dir` (the D-SEC-06 re-init needs an empty directory).
    fn empty_dir(&self) -> std::io::Result<()> {
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if std::fs::symlink_metadata(&path)?.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    fn argv<S: AsRef<OsStr>>(&self, args: &[S]) -> Vec<OsString> {
        let mut v = self.exe.argv0.clone();
        v.extend(args.iter().map(|a| a.as_ref().to_os_string()));
        v
    }

    fn capture_in<S: AsRef<OsStr>>(&self, cwd: &Path, args: &[S], env: &Env) -> Out {
        run::capture(
            self.backend.as_ref(),
            &self.argv(args),
            env,
            cwd,
            Group::Inherit,
        )
    }

    fn checked_in<S: AsRef<OsStr>>(
        &self,
        cwd: &Path,
        args: &[S],
        env: &Env,
    ) -> Result<Out, Failure> {
        run::checked(
            self.backend.as_ref(),
            &self.argv(args),
            env,
            cwd,
            Group::Inherit,
        )
    }

    /// A step that keeps git's stderr for the retry decision.
    fn step<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<(), StepError> {
        let argv = self.argv(args);
        let out = run::capture(
            self.backend.as_ref(),
            &argv,
            &self.env,
            &self.dir,
            Group::Inherit,
        );
        if out.status == 0 {
            return Ok(());
        }
        Err(StepError {
            failure: out.called_process_error(&argv),
            stderr: out.stderr.unwrap_or_default(),
        })
    }

    /// The process cwd of pre-commit's `git init <dir>` (an absolute `<dir>`): any existing directory works.
    fn init_cwd(&self) -> PathBuf {
        if self.dir.is_dir() {
            self.dir.to_path_buf()
        } else {
            self.dir
                .parent()
                .filter(|p| p.is_dir())
                .map_or_else(|| self.dir.to_path_buf(), Path::to_path_buf)
        }
    }

    /// `git.init_repo(path, remote)` (`git.py:185-193`) with an optional `--object-format` (D-SEC-06 retry only).
    fn init_repo(&self, repo: &str, object_format: Option<&str>) -> Result<(), Failure> {
        let mut init: Vec<OsString> = NO_FS_MONITOR.iter().map(|a| os(a)).collect();
        init.extend([os("init"), os("--template=")]);
        if let Some(f) = object_format {
            init.push(os(&format!("--object-format={f}")));
        }
        init.push(self.dir.as_os_str().to_os_string());
        self.checked_in(&self.init_cwd(), &init, &self.env)?;
        let mut remote: Vec<OsString> = NO_FS_MONITOR.iter().map(|a| os(a)).collect();
        remote.extend([os("remote"), os("add"), os("origin"), remote_arg(repo)]);
        self.checked_in(&self.dir, &remote, &self.env)?;
        Ok(())
    }

    fn clone_sequence(
        &self,
        repo: &str,
        rev: &str,
        object_format: Option<&str>,
    ) -> Result<(), StepError> {
        self.init_repo(repo, object_format)?;
        let shallow = self
            .step(&[
                "-c",
                "protocol.version=2",
                "fetch",
                "origin",
                rev,
                "--depth=1",
            ])
            .and_then(|()| self.step(&["checkout", "FETCH_HEAD"]))
            .and_then(|()| {
                self.step(&[
                    "-c",
                    "protocol.version=2",
                    "submodule",
                    "update",
                    "--init",
                    "--recursive",
                    "--depth=1",
                ])
            });
        if shallow.is_ok() {
            return Ok(());
        }
        self.step(&["fetch", "origin", "--tags"])?;
        self.step(&["checkout", rev])?;
        self.step(&["submodule", "update", "--init", "--recursive"])
    }

    /// `git clone --local --no-checkout <src> <dest>` + checkout `<rev>` (+ shallow submodules); falls back to
    /// `clone_rev`.
    ///
    /// The deps variant of a cloned repo (ADR-025): hard links from the base clone `src`, no network. The base's
    /// `origin` URL is copied so relative submodule URLs resolve as in the base; `<rev>` is checked out as the base's
    /// `HEAD` commit (the base is a detached checkout of `<rev>`, so a tag name need not exist in it). Any failure
    /// empties `dest` and runs [`OwnedRepo::clone_rev`] (foreground) from the base's `origin` URL, or from `src`
    /// itself when it has none.
    pub fn clone_local(
        exe: Arc<GitExe>,
        base: &Env,
        src: &Path,
        dest: &Path,
        rev: &str,
        b: Arc<dyn GitBackend>,
    ) -> Result<Self, Failure> {
        let env = owned_env(&exe, base);
        let owned = Self::new(Arc::clone(&exe), env, dest, Arc::clone(&b));
        let origin = owned.origin_url(src);
        if owned.local_variant(src, origin.as_deref()).is_ok() {
            return Ok(owned);
        }
        owned
            .empty_dir()
            .map_err(|e| Failure::from_io(&e, Some(dest.as_os_str()), None))?;
        let from = origin.unwrap_or_else(|| src.to_string_lossy().into_owned());
        Self::clone_rev(exe, base, dest, &from, rev, CloneMode::Foreground, b)
    }

    /// `remote.origin.url` of the base clone.
    fn origin_url(&self, src: &Path) -> Option<String> {
        let out = self.capture_in(src, &["config", "--get", "remote.origin.url"], &self.env);
        (out.status == 0).then(|| String::from_utf8_lossy(bytes_strip(&out.stdout)).into_owned())
    }

    fn local_variant(&self, src: &Path, origin: Option<&str>) -> Result<(), Failure> {
        let head = self.checked_in(src, &["rev-parse", "HEAD"], &self.env)?;
        let head = String::from_utf8_lossy(bytes_strip(&head.stdout)).into_owned();
        let clone = [
            os("clone"),
            os("--local"),
            os("--no-checkout"),
            src.as_os_str().to_os_string(),
            self.dir.as_os_str().to_os_string(),
        ];
        self.checked_in(&self.init_cwd(), &clone, &self.env)?;
        if let Some(url) = origin {
            self.checked_in(&self.dir, &["remote", "set-url", "origin", url], &self.env)?;
        }
        self.checked_in(&self.dir, &["checkout", head.as_str()], &self.env)?;
        self.checked_in(
            &self.dir,
            &[
                "-c",
                "protocol.version=2",
                "submodule",
                "update",
                "--init",
                "--recursive",
                "--depth=1",
            ],
            &self.env,
        )?;
        Ok(())
    }

    /// An existing owned checkout (or an empty directory for [`OwnedRepo::rev_info`]).
    pub fn open(exe: Arc<GitExe>, base: &Env, dir: &Path, b: Arc<dyn GitBackend>) -> Self {
        let env = owned_env(&exe, base);
        Self::new(exe, env, dir, b)
    }

    /// `git ls-tree -z <rev> -- <dir>/`: `(path, blob sha)` of the blobs directly in `dir` (Tier B module verification,
    /// 09 §5.4); `dir = ""` lists the root.
    pub fn ls_tree_blobs(&self, rev: &str, dir: &str) -> Result<Vec<(String, String)>, Failure> {
        let mut args = vec![os("ls-tree"), os("-z"), os(rev)];
        if !dir.is_empty() {
            args.push(os("--"));
            args.push(os(&format!("{}/", dir.trim_end_matches('/'))));
        }
        let out = self.checked_in(&self.dir, &args, &self.env)?;
        let records = crate::ZList::from_z(out.stdout)
            .map_err(|_| Failure::unexpected("MemoryError", "git output larger than 4 GiB"))?;
        let mut blobs = Vec::new();
        for rec in records.iter() {
            // <mode> SP <type> SP <object> TAB <path>
            let Some(tab) = memchr::memchr(b'\t', rec) else {
                continue;
            };
            let head = String::from_utf8_lossy(rec.get(..tab).unwrap_or_default()).into_owned();
            let path =
                String::from_utf8_lossy(rec.get(tab.saturating_add(1)..).unwrap_or_default());
            let mut fields = head.split(' ');
            let (_mode, kind, sha) = (fields.next(), fields.next(), fields.next());
            if let (Some("blob"), Some(sha)) = (kind, sha) {
                blobs.push((path.into_owned(), sha.to_owned()));
            }
        }
        Ok(blobs)
    }

    /// `git cat-file blob <rev>:<path>` (Tier B): the blob bytes.
    pub fn read_blob(&self, rev: &str, path: &str) -> Result<Vec<u8>, Failure> {
        let spec = format!("{rev}:{path}");
        Ok(self
            .checked_in(&self.dir, &["cat-file", "blob", spec.as_str()], &self.env)?
            .stdout)
    }

    /// autoupdate 03 §11.2.
    ///
    /// `RevInfo.update` (`autoupdate.py:38-82`) in `self`'s (empty, temporary) directory with pre-commit's argv
    /// (`git -c core.useBuiltinFSMonitor=false -C <dir> …`) and the sanitized env (hazard c, D-SEC-05): `init_repo`,
    /// `config extensions.partialClone true`, `fetch origin HEAD --quiet --filter=blob:none --tags`, `describe
    /// FETCH_HEAD --tags --abbrev=0` (`tags_only`) or `--exact-match` (02 G.7 row 7; was `--exact`) falling back to
    /// `rev-parse FETCH_HEAD`, the first tag of `tag --points-at <rev>` containing a `.`, `rev-parse <rev>` for
    /// `--freeze` (an annotated tag freezes to the tag object), then `show <rev>:.pre-commit-hooks.yaml` +
    /// `checkout <rev> -- .pre-commit-hooks.yaml` (failures ignored). `hook_ids` is left empty: the manifest at
    /// `<dir>/.pre-commit-hooks.yaml` is loaded by the caller (snp-config owns manifest parsing and its
    /// `InvalidManifestError` texts). A relative local `repo` must already be joined to the project root, as for
    /// [`OwnedRepo::clone_rev`]; `RevInfo::repo` is `repo` as given.
    pub fn rev_info(&self, repo: &str, tags_only: bool, freeze: bool) -> Result<RevInfo, Failure> {
        let dir = self.dir.as_os_str().to_os_string();
        let git_c = |args: &[&str]| -> Vec<OsString> {
            let mut v: Vec<OsString> = NO_FS_MONITOR.iter().map(|a| os(a)).collect();
            v.push(os("-C"));
            v.push(dir.clone());
            v.extend(args.iter().map(|a| os(a)));
            v
        };
        let text = |out: Out| String::from_utf8_lossy(&out.stdout).into_owned();
        self.init_repo(repo, None)?;
        self.checked_in(
            &self.dir,
            &git_c(&["config", "extensions.partialClone", "true"]),
            &self.env,
        )?;
        self.checked_in(
            &self.dir,
            &git_c(&[
                "fetch",
                "origin",
                "HEAD",
                "--quiet",
                "--filter=blob:none",
                "--tags",
            ]),
            &self.env,
        )?;
        let tag_opt = if tags_only {
            "--abbrev=0"
        } else {
            "--exact-match"
        };
        let described = self.checked_in(
            &self.dir,
            &git_c(&["describe", "FETCH_HEAD", "--tags", tag_opt]),
            &self.env,
        );
        let mut rev = match described {
            Ok(out) => {
                let tag = strip(&text(out)).to_owned();
                if tags_only {
                    self.best_candidate_tag(&tag)?
                } else {
                    tag
                }
            }
            Err(_) => strip(&text(self.checked_in(
                &self.dir,
                &git_c(&["rev-parse", "FETCH_HEAD"]),
                &self.env,
            )?))
            .to_owned(),
        };
        let mut frozen = None;
        if freeze {
            let exact = strip(&text(self.checked_in(
                &self.dir,
                &git_c(&["rev-parse", rev.as_str()]),
                &self.env,
            )?))
            .to_owned();
            if exact != rev {
                frozen = Some(std::mem::replace(&mut rev, exact));
            }
        }
        let show = format!("{rev}:{MANIFEST_FILE}");
        if self
            .checked_in(&self.dir, &git_c(&["show", show.as_str()]), &self.env)
            .is_ok()
        {
            let _ignored: Result<Out, Failure> = self.checked_in(
                &self.dir,
                &git_c(&["checkout", rev.as_str(), "--", MANIFEST_FILE]),
                &self.env,
            );
        }
        Ok(RevInfo {
            repo: repo.to_owned(),
            rev,
            frozen,
            hook_ids: Vec::new(),
        })
    }

    /// `git.get_best_candidate_tag` (`git.py:233-245`): `git -c core.useBuiltinFSMonitor=false tag --points-at <rev>`
    /// (no `-C`; cwd = the repo); the first tag containing a `.`, else `rev`.
    fn best_candidate_tag(&self, rev: &str) -> Result<String, Failure> {
        let mut args: Vec<&str> = NO_FS_MONITOR.to_vec();
        args.extend(["tag", "--points-at", rev]);
        let out = self.checked_in(&self.dir, &args, &self.env)?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(snp_pycompat::text::splitlines(&text)
            .find(|t| t.contains('.'))
            .unwrap_or(rev)
            .to_owned())
    }

    /// try-repo 03 §12: (shadow repo path, ref).
    ///
    /// `try_repo._repo_ref` (`try_repo.py:21-48`) for `self` = the repo being tried, as `args.repo` after
    /// `_adjust_args_and_chdir` (a URL, or a path relative to `user`'s root, pre-commit's cwd), with the sanitized env
    /// everywhere except the deliberate `GIT_INDEX_FILE`/`GIT_OBJECT_DIRECTORY` of the shadow `add`s (02 G.6): `ref =
    /// ls-remote --exit-code <repo> HEAD` with `<repo>` verbatim in the root; a repo that does not exist on disk
    /// relative to the root (`os.path.exists`: every URL) is `(repo, ref)` with no further call; when `git diff
    /// --quiet --no-ext-diff HEAD` in the repo returns 1, the repo is cloned
    /// into `<tmp>/shadow-repo`, checked out as `_pc_tmp`, the staged files and `add -u` are written into the shadow
    /// index, committed as `pre-commit <asottile+pre-commit@umich.edu>`, and `(shadow, its HEAD)` is returned.
    /// Otherwise `(repo, ref)` comes back unchanged: the caller prints `[WARNING] Creating temporary repo with
    /// uncommitted changes...` exactly when the returned path differs from the repo (nothing is printed in between in
    /// pre-commit, so the order is the same).
    pub fn shadow_try_repo(&self, user: &Git, tmp: &Path) -> Result<(PathBuf, String), Failure> {
        let root = user.root();
        let repo = if self.dir.is_absolute() {
            self.dir.to_path_buf()
        } else {
            root.join(&self.dir)
        };
        let reference = self.head_rev(root, self.dir.as_os_str())?;
        if !repo.exists() {
            return Ok((self.dir.to_path_buf(), reference));
        }
        let dirty = self.capture_in(
            &repo,
            &["diff", "--quiet", "--no-ext-diff", "HEAD"],
            &self.env,
        );
        if dirty.status != 1 {
            return Ok((self.dir.to_path_buf(), reference));
        }
        let shadow = tmp.join("shadow-repo");
        let clone = [
            os("clone"),
            self.dir.as_os_str().to_os_string(),
            shadow.as_os_str().to_os_string(),
        ];
        self.checked_in(root, &clone, &self.env)?;
        self.checked_in(
            &shadow,
            &["checkout", reference.as_str(), "-b", "_pc_tmp"],
            &self.env,
        )?;
        let git_path = |name: &str| -> Result<OsString, Failure> {
            let out = self.checked_in(&shadow, &["rev-parse", "--git-path", name], &self.env)?;
            Ok(snp_pycompat::ospath::join(
                shadow.as_os_str(),
                &[&os_from_bytes(bytes_strip(&out.stdout))],
            ))
        };
        let shadow_env = self.env.patched(
            &EnvPatch::default()
                .set("GIT_INDEX_FILE", git_path("index")?)
                .set("GIT_OBJECT_DIRECTORY", git_path("objects")?),
        );
        let staged = self.checked_in(
            &repo,
            &[
                "diff",
                "--staged",
                "--name-only",
                "--no-ext-diff",
                "-z",
                "--diff-filter=ACMRTUXB",
            ],
            &self.env,
        )?;
        let staged = crate::ZList::from_z(staged.stdout)
            .map_err(|_| Failure::unexpected("MemoryError", "git output larger than 4 GiB"))?;
        let files: Vec<OsString> = staged.iter().map(os_from_bytes).collect();
        let refs: Vec<&OsStr> = files.iter().map(OsString::as_os_str).collect();
        run::xargs_serial(
            self.backend.as_ref(),
            &self.argv(&["add", "--"]),
            &refs,
            &shadow_env,
            &repo,
            Group::Inherit,
        )?;
        self.checked_in(&repo, &["add", "-u"], &shadow_env)?;
        let commit_env = self.env.patched(
            &EnvPatch::default()
                .set("GIT_AUTHOR_NAME", COMMIT_NAME)
                .set("GIT_COMMITTER_NAME", COMMIT_NAME)
                .set("GIT_AUTHOR_EMAIL", COMMIT_EMAIL)
                .set("GIT_COMMITTER_EMAIL", COMMIT_EMAIL),
        );
        self.checked_in(
            &shadow,
            &["commit", "--no-edit", "--no-gpg-sign", "-n", "-minit"],
            &commit_env,
        )?;
        let head = self.head_rev(root, shadow.as_os_str())?;
        Ok((shadow, head))
    }

    /// `git.head_rev` (`git.py:170-172`): the first whitespace token of `git ls-remote --exit-code <remote> HEAD`.
    fn head_rev(&self, cwd: &Path, remote: &OsStr) -> Result<String, Failure> {
        let args = [
            os("ls-remote"),
            os("--exit-code"),
            remote.to_os_string(),
            os("HEAD"),
        ];
        let out = self.checked_in(cwd, &args, &self.env)?;
        let text = String::from_utf8_lossy(&out.stdout);
        split_whitespace(&text, None)
            .next()
            .map(str::to_owned)
            .ok_or_else(|| Failure::unexpected("IndexError", "list index out of range"))
    }
}

/// `<repo>` of `git remote add origin`: `os.path.abspath(repo)` when `os.path.isdir(repo)` (`git.py:186-187`).
///
/// pre-commit evaluates both after `_adjust_args_and_chdir` moved its cwd to the project root (`main.py:188-189`).
/// snp never changes its cwd and `clone_rev`/`rev_info` carry no root, so the caller resolves a relative local
/// repository against the project root first (see [`OwnedRepo::clone_rev`]); here an absolute directory is
/// normalised exactly like `abspath` (`.`/`..`/trailing `/`), and anything else, a relative path included, is passed
/// verbatim. The process cwd is never consulted (S6).
fn remote_arg(repo: &str) -> OsString {
    let p = Path::new(repo);
    if p.is_absolute() && p.is_dir() {
        snp_pycompat::ospath::abspath(p.as_os_str(), p).into_os_string()
    } else {
        os(repo)
    }
}

/// `no_git_env(base)` + the fsmonitor injection when git understands `GIT_CONFIG_COUNT` (≥ 2.31).
fn owned_env(exe: &GitExe, base: &Env) -> Env {
    let env = no_git_env(base);
    if exe.caps.config_env {
        inject_fsmonitor(&env)
    } else {
        env
    }
}

/// ADR-018 / 02 G.7 row 4: `core.fsmonitor=` (empty: off on every git version) and
/// `core.useBuiltinFSMonitor=false` appended through `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>`;
/// an existing count is extended (later entries win), a malformed one is left untouched so git still reports it.
pub(crate) fn inject_fsmonitor(env: &Env) -> Env {
    let start: u32 = match env.get("GIT_CONFIG_COUNT") {
        None => 0,
        Some(v) if v.is_empty() => 0,
        Some(v) => match parse_config_count(v.as_encoded_bytes()) {
            Some(n) => n,
            None => return env.clone(),
        },
    };
    let (Some(second), Some(count)) = (start.checked_add(1), start.checked_add(2)) else {
        return env.clone();
    };
    if i32::try_from(count).is_err() {
        return env.clone();
    }
    let key = |name: &str, n: u32| name_case(&format!("{name}{n}"));
    env.patched(
        &EnvPatch::default()
            .set(key("GIT_CONFIG_KEY_", start), "core.fsmonitor")
            .set(key("GIT_CONFIG_VALUE_", start), "")
            .set(key("GIT_CONFIG_KEY_", second), "core.useBuiltinFSMonitor")
            .set(key("GIT_CONFIG_VALUE_", second), "false")
            .set(name_case("GIT_CONFIG_COUNT"), count.to_string()),
    )
}

/// git's `GIT_CONFIG_COUNT` parse (`config.c` `git_config_from_env`): `strtoul(env, &end, 10)` with trailing
/// garbage and values above `INT_MAX` rejected. Like `strtoul`, leading C-locale whitespace and one `+`/`-` sign are
/// accepted; a negated non-zero value wraps above `INT_MAX` (rejected), `-0` is 0. `None` = git reports it as bogus.
fn parse_config_count(b: &[u8]) -> Option<u32> {
    let start = b
        .iter()
        .position(|c| !matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))?;
    let rest = b.get(start..)?;
    let (negative, digits) = match rest.split_first() {
        Some((b'+', d)) => (false, d),
        Some((b'-', d)) => (true, d),
        _ => (false, rest),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut n: u32 = 0;
    for d in digits {
        n = n
            .checked_mul(10)?
            .checked_add(u32::from(d.wrapping_sub(b'0')))?;
    }
    if negative && n != 0 {
        return None;
    }
    i32::try_from(n).is_ok().then_some(n)
}

/// ADR-064: the background prefetch environment.
fn prefetch_env(env: &Env, repo: &str) -> Env {
    let mut patch = EnvPatch::default()
        .set(name_case("GIT_TERMINAL_PROMPT"), "0")
        .set(name_case("GIT_ASKPASS"), "")
        .set(name_case("GCM_INTERACTIVE"), "never");
    if classify_url(repo) == UrlKind::Ssh
        && !env.is_present("GIT_SSH_COMMAND")
        && !env.is_present("GIT_SSH")
    {
        patch = patch.set(name_case("GIT_SSH_COMMAND"), "ssh -o BatchMode=yes");
    }
    env.patched(&patch)
}

/// Upper-cased on Windows, where pre-commit's `env=` dict copies of `os.environ` carry upper-case names (ADR-074).
fn name_case(name: &str) -> String {
    if cfg!(windows) {
        name.to_uppercase()
    } else {
        name.to_owned()
    }
}

/// How git would reach a repository URL.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum UrlKind {
    Https,
    File,
    Local,
    Ssh,
    Other,
}

/// git's URL forms (`connect.c` `url_is_local_not_ssh`, `transport.c`): `<transport>::<address>` helpers, `scheme://`
/// URLs, local paths (no `:`, a `/` before the first `:`, or a DOS drive), else scp-like `[user@]host:path` (ssh).
fn classify_url(url: &str) -> UrlKind {
    if let Some(i) = url.find("::")
        && url.find("://").is_none_or(|j| i < j)
    {
        return UrlKind::Other;
    }
    if let Some(i) = url.find("://") {
        let scheme = url.get(..i).unwrap_or_default().to_ascii_lowercase();
        return match scheme.as_str() {
            "https" => UrlKind::Https,
            "file" => UrlKind::File,
            "ssh" | "git+ssh" | "ssh+git" => UrlKind::Ssh,
            _ => UrlKind::Other,
        };
    }
    let b = url.as_bytes();
    let dos_drive =
        b.len() >= 2 && b.first().is_some_and(u8::is_ascii_alphabetic) && b.get(1) == Some(&b':');
    match (url.find(':'), url.find('/')) {
        (None, _) => UrlKind::Local,
        (Some(c), Some(s)) if s < c => UrlKind::Local,
        _ if dos_drive => UrlKind::Local,
        _ => UrlKind::Ssh,
    }
}

/// Background prefetch is allowed for https://, file:// and local paths; for ssh/scp URLs only when none of
/// `GIT_SSH_COMMAND`, `GIT_SSH` and `core.sshCommand` is set (`BatchMode` can then be injected); never otherwise
/// (ADR-064).
pub fn prefetch_eligible(url: &str, env: &Env, core_ssh_command_set: bool) -> bool {
    match classify_url(url) {
        UrlKind::Https | UrlKind::File | UrlKind::Local => true,
        UrlKind::Ssh => {
            !core_ssh_command_set
                && !env.is_present("GIT_SSH_COMMAND")
                && !env.is_present("GIT_SSH")
        }
        UrlKind::Other => false,
    }
}

/// The variables `no_git_env` keeps besides every non-`GIT_` one (`git.py:27-48`).
const NO_GIT_ENV_KEEP: [&str; 9] = [
    "GIT_EXEC_PATH",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_SSL_CAINFO",
    "GIT_SSL_NO_VERIFY",
    "GIT_CONFIG_COUNT",
    "GIT_HTTP_PROXY_AUTHMETHOD",
    "GIT_ALLOW_PROTOCOL",
    "GIT_ASKPASS",
];

/// git.py:27-48 allowlist (Windows: names upper-cased, §2.5).
///
/// Every `GIT_*` variable is dropped except `GIT_CONFIG_KEY_*`, `GIT_CONFIG_VALUE_*` and the nine names above, so an
/// exported `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE` (git sets them while running hooks) never leaks into an owned
/// repository or an installer (D-SEC-05). On Windows the names are upper-cased like CPython's `os.environ` copy
/// (ADR-074).
pub fn no_git_env(env: &Env) -> Env {
    Env::from_pairs(env.iter().filter_map(|(k, v)| {
        let name = k
            .to_str()
            .map_or_else(|| k.to_os_string(), |s| OsString::from(name_case(s)));
        // on bytes: CPython decodes environ keys with surrogateescape, so a non-UTF-8 `GIT_…` name is still dropped
        let b = name.as_encoded_bytes();
        let keep = !b.starts_with(b"GIT_")
            || b.starts_with(b"GIT_CONFIG_KEY_")
            || b.starts_with(b"GIT_CONFIG_VALUE_")
            || NO_GIT_ENV_KEEP.iter().any(|k| b == k.as_bytes());
        keep.then(|| (name, v.to_os_string()))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().copied())
    }

    fn get(e: &Env, k: &str) -> Option<String> {
        e.get(k).map(|v| v.to_string_lossy().into_owned())
    }

    #[test]
    fn injection_starts_extends_or_leaves_alone() {
        let e = inject_fsmonitor(&env(&[("PATH", "/bin")]));
        assert_eq!(get(&e, "GIT_CONFIG_COUNT").as_deref(), Some("2"));
        assert_eq!(
            get(&e, "GIT_CONFIG_KEY_0").as_deref(),
            Some("core.fsmonitor")
        );
        assert_eq!(get(&e, "GIT_CONFIG_VALUE_0").as_deref(), Some(""));
        assert_eq!(
            get(&e, "GIT_CONFIG_KEY_1").as_deref(),
            Some("core.useBuiltinFSMonitor")
        );
        assert_eq!(get(&e, "GIT_CONFIG_VALUE_1").as_deref(), Some("false"));

        let e = inject_fsmonitor(&env(&[
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "user.name"),
            ("GIT_CONFIG_VALUE_0", "x"),
        ]));
        assert_eq!(get(&e, "GIT_CONFIG_COUNT").as_deref(), Some("3"));
        assert_eq!(get(&e, "GIT_CONFIG_KEY_0").as_deref(), Some("user.name"));
        assert_eq!(
            get(&e, "GIT_CONFIG_KEY_1").as_deref(),
            Some("core.fsmonitor")
        );
        assert_eq!(
            get(&e, "GIT_CONFIG_KEY_2").as_deref(),
            Some("core.useBuiltinFSMonitor")
        );

        // strtoul: leading whitespace and a sign are accepted, the rest must be digits up to INT_MAX
        for (count, next) in [
            (" 2", "4"),
            ("+2", "4"),
            ("\t\n2", "4"),
            ("-0", "2"),
            ("007", "9"),
        ] {
            let e = inject_fsmonitor(&env(&[("GIT_CONFIG_COUNT", count)]));
            assert_eq!(
                get(&e, "GIT_CONFIG_COUNT").as_deref(),
                Some(next),
                "{count:?}"
            );
            let first = next.parse::<u32>().unwrap() - 2;
            assert_eq!(
                get(&e, &format!("GIT_CONFIG_KEY_{first}")).as_deref(),
                Some("core.fsmonitor"),
                "{count:?}"
            );
        }
        for bogus in [
            "x",
            "1 ",
            " ",
            "+",
            "-",
            "+-1",
            "-1",
            "1.0",
            "0x1",
            "2147483648",
            "99999999999",
        ] {
            let e = inject_fsmonitor(&env(&[("GIT_CONFIG_COUNT", bogus)]));
            assert_eq!(get(&e, "GIT_CONFIG_COUNT").as_deref(), Some(bogus));
            assert_eq!(get(&e, "GIT_CONFIG_KEY_0"), None);
        }
        let e = inject_fsmonitor(&env(&[("GIT_CONFIG_COUNT", "")]));
        assert_eq!(get(&e, "GIT_CONFIG_COUNT").as_deref(), Some("2"));
        let e = inject_fsmonitor(&env(&[("GIT_CONFIG_COUNT", "2147483646")]));
        assert_eq!(get(&e, "GIT_CONFIG_COUNT").as_deref(), Some("2147483646"));
    }

    #[test]
    #[cfg(unix)]
    fn no_git_env_decides_non_utf8_names_on_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let raw = |b: &[u8]| OsStr::from_bytes(b).to_os_string();
        let e = no_git_env(&Env::from_pairs([
            (raw(b"GIT_\xffDIR"), raw(b"x")),
            (raw(b"GIT_CONFIG_KEY_\xff"), raw(b"k")),
            (raw(b"X_\xff"), raw(b"y")),
            (raw(b"GIT_SSH"), raw(b"plink")),
            (raw(b"GIT_DIR"), raw(b"/r/.git")),
        ]));
        let mut names: Vec<Vec<u8>> = e.iter().map(|(k, _)| k.as_bytes().to_vec()).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                b"GIT_CONFIG_KEY_\xff".to_vec(),
                b"GIT_SSH".to_vec(),
                b"X_\xff".to_vec()
            ]
        );
    }

    #[test]
    fn remote_arg_never_reads_the_process_cwd() {
        // `src` is a directory relative to this crate's cwd under `cargo test`: still passed verbatim
        assert_eq!(remote_arg("src"), OsString::from("src"));
        assert_eq!(remote_arg("https://h/x"), OsString::from("https://h/x"));
        if !cfg!(unix) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("hooks");
        std::fs::create_dir_all(&sub).unwrap();
        let messy = format!("{}/./hooks/", dir.path().display());
        assert_eq!(remote_arg(&messy), sub.into_os_string());
        let missing = format!("{}/nope", dir.path().display());
        assert_eq!(remote_arg(&missing), OsString::from(&missing));
    }

    #[test]
    fn urls() {
        assert_eq!(classify_url("https://github.com/a/b"), UrlKind::Https);
        assert_eq!(classify_url("HTTPS://github.com/a/b"), UrlKind::Https);
        assert_eq!(classify_url("http://example.com/x"), UrlKind::Other);
        assert_eq!(classify_url("file:///tmp/x"), UrlKind::File);
        assert_eq!(classify_url("/tmp/x"), UrlKind::Local);
        assert_eq!(classify_url("../hooks"), UrlKind::Local);
        assert_eq!(classify_url("./a:b"), UrlKind::Local);
        assert_eq!(classify_url(r"C:\repos\x"), UrlKind::Local);
        assert_eq!(classify_url("C:/repos/x"), UrlKind::Local);
        assert_eq!(classify_url("git@github.com:a/b.git"), UrlKind::Ssh);
        assert_eq!(classify_url("host:path"), UrlKind::Ssh);
        assert_eq!(classify_url("ssh://git@host/x"), UrlKind::Ssh);
        assert_eq!(classify_url("git+ssh://host/x"), UrlKind::Ssh);
        assert_eq!(classify_url("git://host/x"), UrlKind::Other);
        assert_eq!(classify_url("ext::ssh -i key host %S"), UrlKind::Other);
        assert_eq!(
            classify_url("persistent-https::https://x/y"),
            UrlKind::Other
        );
    }

    #[test]
    fn object_format_from_mismatch_message() {
        assert_eq!(
            server_object_format(b"fatal: mismatched algorithms: client sha256; server sha1\n"),
            Some("sha1")
        );
        assert_eq!(
            server_object_format(b"fatal: mismatched algorithms: client sha1; server sha256\n"),
            Some("sha256")
        );
        assert_eq!(
            server_object_format(b"fatal: Algorithmen stimmen nicht: Client sha1; Server sha256\n"),
            Some("sha256")
        );
        assert_eq!(
            server_object_format(b"fatal: couldn't find remote ref deadbeef\n"),
            None
        );
        assert_eq!(server_object_format(b"error: sha1 only\n"), None);
    }

    #[test]
    fn prefetch_env_vars() {
        let e = prefetch_env(&env(&[("PATH", "/bin")]), "git@host:x.git");
        assert_eq!(get(&e, "GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
        assert_eq!(get(&e, "GIT_ASKPASS").as_deref(), Some(""));
        assert_eq!(get(&e, "GCM_INTERACTIVE").as_deref(), Some("never"));
        assert_eq!(
            get(&e, "GIT_SSH_COMMAND").as_deref(),
            Some("ssh -o BatchMode=yes")
        );
        let e = prefetch_env(&env(&[]), "https://h/x");
        assert_eq!(get(&e, "GIT_SSH_COMMAND"), None);
        let e = prefetch_env(&env(&[("GIT_SSH", "plink")]), "git@host:x.git");
        assert_eq!(get(&e, "GIT_SSH_COMMAND"), None);
    }
}
