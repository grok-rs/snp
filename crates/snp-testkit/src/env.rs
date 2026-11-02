//! The hermetic test environment (10 §7.3.2).
//!
//! Owner: WP-18.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, OnceLock};

use snp_core::env::{Env, EnvPatch};

use crate::harness::{OrFail, canonical, mkdirs, write};
use crate::{Bins, Cmd, Fixture, RepoBuilder, fixture, snapshot};

/// The global git configuration of every test (10 §7.3.2): pinned identity and branch, no signing, no CRLF
/// conversion, no fsmonitor, local submodules allowed, no detached-HEAD advice, every directory safe (CI checkouts
/// owned by another uid). Read through `GIT_CONFIG_GLOBAL` and, for snp-owned repositories whose env drops
/// `GIT_CONFIG_*` (`no_git_env`), through `$HOME/.gitconfig`, which is the same file.
const GITCONFIG: &str = "[init]\n\tdefaultBranch = master\n[user]\n\tname = test\n\temail = test@example.com\n\
[commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n[core]\n\tautocrlf = false\n\tfsmonitor = false\n\
[protocol \"file\"]\n\tallow = always\n[advice]\n\tdetachedHead = false\n[safe]\n\tdirectory = *\n";

/// Process variables a child may inherit (10 §7.3.2): everything else is cleared.
const INHERITED: [&str; 4] = ["PATH", "SYSTEMROOT", "PATHEXT", "COMSPEC"];
const INHERITED_WINDOWS: [&str; 1] = ["WINDIR"];

/// Canonicalised temp root, hermetic env (10 §7.3.2), fixed `GIT_*_DATE` counter.
#[derive(Debug)]
pub struct TestEnv {
    tmp: Arc<tempfile::TempDir>,
    root: PathBuf,
    env: Env,
    bins: Bins,
    pub(crate) dates: Arc<AtomicU64>,
    alias: Arc<OnceLock<PathBuf>>,
}

/// The pre-commit 4.6.2 oracle venvs built by `cargo xtask oracle install`.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Oracle {
    /// `target/oracle/pc462-py312` (CPython 3.12.3).
    Pc462Py312,
    /// `target/oracle/pc462-py313` (CPython ≥ 3.13.14).
    Pc462Py313,
}

impl Oracle {
    const fn dir_name(self) -> &'static str {
        match self {
            Self::Pc462Py312 => "pc462-py312",
            Self::Pc462Py313 => "pc462-py313",
        }
    }
}

/// The parent process's value of `key` (the test harness reads its own environment; children never see it).
fn process_var(key: &str) -> Option<OsString> {
    Env::capture_process().get(key).map(OsStr::to_os_string)
}

/// `<workspace>/target/tmp`, located like `target/oracle` (`CARGO_TARGET_TMPDIR` is a compile-time variable of
/// integration tests only, so library code derives it).
fn default_tmp_base() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp")
}

impl TestEnv {
    /// A fresh hermetic environment: a canonicalised temp root holding `home/` (+ `.gitconfig`, `.cache/`), `store/`
    /// (`PRE_COMMIT_HOME`), `snp-store/` (`SNP_HOME`), an empty `git-template/` and `tmp/`; the child env of 10 §7.3.2.
    ///
    /// The root lives under `$SNP_TEST_TMPDIR`, else `$CARGO_TARGET_TMPDIR`, else the workspace's `target/tmp` (the
    /// directory cargo gives integration tests as `CARGO_TARGET_TMPDIR`): a known prefix that `cargo clean` removes
    /// and that no periodic `/tmp` cleaner touches. Because that prefix sits inside the snp checkout,
    /// `GIT_CEILING_DIRECTORIES` stops git's discovery at the root's parent: a test directory that is not a
    /// repository never finds the snp repository above it.
    pub fn new(bins: Bins) -> Self {
        let base = process_var("SNP_TEST_TMPDIR")
            .or_else(|| process_var("CARGO_TARGET_TMPDIR"))
            .map_or_else(default_tmp_base, PathBuf::from);
        mkdirs(&base);
        let tmp = tempfile::Builder::new()
            .prefix("snp-test-")
            .tempdir_in(&base)
            .or_fail("creating the test root");
        let root = canonical(tmp.path());
        let home = root.join("home");
        let template = root.join("git-template");
        let tmpdir = root.join("tmp");
        for d in [
            &home,
            &home.join(".cache"),
            &root.join("store"),
            &root.join("snp-store"),
            &template,
            &tmpdir,
        ] {
            mkdirs(d);
        }
        let gitconfig = home.join(".gitconfig");
        write(&gitconfig, GITCONFIG.as_bytes());

        let mut pairs: Vec<(OsString, OsString)> = Vec::new();
        let inherited = INHERITED
            .iter()
            .chain(INHERITED_WINDOWS.iter().filter(|_| cfg!(windows)));
        for k in inherited {
            if let Some(v) = process_var(k) {
                pairs.push(((*k).into(), v));
            }
        }
        let paths: [(&str, &Path); 8] = [
            ("HOME", &home),
            ("XDG_CACHE_HOME", &home.join(".cache")),
            ("PRE_COMMIT_HOME", &root.join("store")),
            ("SNP_HOME", &root.join("snp-store")),
            ("GIT_CONFIG_GLOBAL", &gitconfig),
            ("GIT_TEMPLATE_DIR", &template),
            ("TMPDIR", &tmpdir),
            ("USERPROFILE", &home),
        ];
        for (k, v) in paths {
            if k != "USERPROFILE" || cfg!(windows) {
                pairs.push((k.into(), v.as_os_str().to_os_string()));
            }
        }
        if cfg!(windows) {
            pairs.push(("TEMP".into(), tmpdir.as_os_str().to_os_string()));
            pairs.push(("TMP".into(), tmpdir.as_os_str().to_os_string()));
        }
        if let Some(parent) = root.parent() {
            pairs.push((
                "GIT_CEILING_DIRECTORIES".into(),
                parent.as_os_str().to_os_string(),
            ));
        }
        let fixed_date = crate::harness::date(0);
        for (k, v) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "test"),
            ("GIT_COMMITTER_NAME", "test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
            ("GIT_AUTHOR_DATE", fixed_date.as_str()),
            ("GIT_COMMITTER_DATE", fixed_date.as_str()),
            ("GIT_ALLOW_PROTOCOL", "file"),
            ("LC_ALL", "C.UTF-8"),
            ("LANG", "C.UTF-8"),
            ("TZ", "UTC"),
            ("VIRTUALENV_NO_DOWNLOAD", "1"),
            // The suite also runs against older git (platform.yml's git axis): the once-per-process "older than
            // 2.31.0" warning (D-DIAG-01) would prefix every expected output. snp-git's own tests cover it.
            ("SNP_ALLOW_OLD_GIT", "1"),
        ] {
            pairs.push((k.into(), v.into()));
        }
        Self {
            tmp: Arc::new(tmp),
            root,
            env: Env::from_pairs(pairs),
            bins,
            dates: Arc::new(AtomicU64::new(0)),
            alias: Arc::new(OnceLock::new()),
        }
    }

    /// The canonicalised temp root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Child env only; never the process env.
    pub fn env(&self) -> &Env {
        &self.env
    }

    /// Derived env (same root, same date counter): `self`'s env plus `k=v`.
    #[must_use]
    pub fn with_env(&self, k: &str, v: impl Into<OsString>) -> Self {
        Self {
            tmp: Arc::clone(&self.tmp),
            root: self.root.clone(),
            env: self
                .env
                .patched(&EnvPatch::default().set(k.to_owned(), v.into())),
            bins: self.bins.clone(),
            dates: Arc::clone(&self.dates),
            alias: Arc::clone(&self.alias),
        }
    }

    /// A deterministic repository builder for `<root>/<name>` (an existing repository is extended).
    pub fn git_repo(&self, name: &str) -> RepoBuilder {
        RepoBuilder::new(
            self.root.join(name),
            self.env.clone(),
            Arc::clone(&self.dates),
        )
    }

    /// Recreation of a pre-commit testing/resources repo (10 §3) or a conftest scenario (10 §7.3).
    pub fn fixture_repo(&self, fixture: Fixture) -> PathBuf {
        fixture::build(self, fixture)
    }

    /// `bins.snp` (snp brand).
    pub fn snp(&self, cwd: &Path) -> Cmd {
        Cmd::new(self.bins.snp.clone(), self.env.clone(), cwd)
    }

    /// `/bin/sh -c script` with the hermetic env.
    pub fn sh(&self, cwd: &Path, script: &str) -> Cmd {
        Cmd::new(PathBuf::from("/bin/sh"), self.env.clone(), cwd).args(["-c", script])
    }

    /// git (resolved on the hermetic `PATH`) with the hermetic env, for runs that need a [`Cmd`] (a pty session).
    pub fn git(&self, cwd: &Path) -> Cmd {
        Cmd::new(
            crate::harness::git_exe(&self.env, cwd),
            self.env.clone(),
            cwd,
        )
    }

    /// Through a `pre-commit` symlink/copy (compat brand): `<root>/bin/pre-commit` (`pre-commit.exe` copy on
    /// Windows), so `basename(argv[0])` selects the compat brand (COMPATIBILITY §3.1).
    pub fn snp_as_pre_commit(&self, cwd: &Path) -> Cmd {
        let alias = self.alias.get_or_init(|| {
            let bin = self.root.join("bin");
            mkdirs(&bin);
            let alias = bin.join(format!("pre-commit{}", std::env::consts::EXE_SUFFIX));
            #[cfg(unix)]
            std::os::unix::fs::symlink(&self.bins.snp, &alias)
                .or_fail("creating the pre-commit alias");
            #[cfg(not(unix))]
            {
                std::fs::copy(&self.bins.snp, &alias).or_fail("copying the pre-commit alias");
            }
            alias
        });
        Cmd::new(alias.clone(), self.env.clone(), cwd)
    }

    /// `SNP_ORACLE_DIR`/target/oracle; never PATH: the `pre-commit` entry point of the oracle venv, `None` when it is
    /// not installed (`cargo xtask oracle install`).
    pub fn oracle(&self, which: Oracle, cwd: &Path) -> Option<Cmd> {
        let dir = process_var("SNP_ORACLE_DIR").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/oracle"),
            PathBuf::from,
        );
        let venv = dir.join(which.dir_name());
        let exe = if cfg!(windows) {
            venv.join("Scripts").join("pre-commit.exe")
        } else {
            venv.join("bin").join("pre-commit")
        };
        exe.is_file().then(|| Cmd::new(exe, self.env.clone(), cwd))
    }

    /// insta filters (10 §7.3.4), in application order: `(regex, replacement)`.
    pub fn filters(&self) -> Vec<(String, String)> {
        snapshot::filters(&self.root)
    }

    /// `bins.probe`.
    pub fn probe(&self) -> &Path {
        &self.bins.probe
    }

    /// A fresh, unique directory name under the root (`<base>`, then `<base>-1`, …): pre-commit's
    /// `tempdir_factory`.
    pub(crate) fn fresh_dir(&self, base: &str) -> PathBuf {
        for n in 0_u64.. {
            let p = if n == 0 {
                self.root.join(base)
            } else {
                self.root.join(format!("{base}-{n}"))
            };
            if !p.exists() {
                mkdirs(&p);
                return p;
            }
        }
        crate::harness::fail("no free directory name")
    }
}
