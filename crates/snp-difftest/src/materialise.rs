//! The materialiser: builds a scenario's hook repositories and work repository at a **fixed absolute root**, the
//! same for both tools (10 §7.4.4): identical paths, identical commit dates and therefore identical SHAs, so
//! absolute paths in output (`file://<root>/repos/hooks`, `Check the log at <root>/store/pre-commit.log`) need no
//! rewriting.
//!
//! Layout of a root `R`: `home/` (`HOME`, `.gitconfig`, `.cache/`), `store/` (`PRE_COMMIT_HOME`), `snp-store/`
//! (`SNP_HOME`), `git-template/` (empty), `tmp/` (`TMPDIR`), `bin/` (`pre-commit` / `snp` entry points of the side
//! under test, first on `PATH`), `repos/<name>/`, `work/`. A marker file `.snp-difftest-root` guards the recursive
//! deletion that starts every side's run.
//!
//! Owner: WP-35.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use snp_core::env::Env;

use crate::exec::{self, Captured, Invocation};
use crate::scenario::{Op, RepoSpec, Scenario, Work};
use crate::text::sh_quote;

/// The marker that makes a directory deletable by the harness.
const MARKER: &str = ".snp-difftest-root";

/// The global git configuration (10 §7.3.2; identical to snp-testkit's).
const GITCONFIG: &str = "[init]\n\tdefaultBranch = master\n[user]\n\tname = test\n\temail = test@example.com\n\
[commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n[core]\n\tautocrlf = false\n\tfsmonitor = false\n\
[protocol \"file\"]\n\tallow = always\n[advice]\n\tdetachedHead = false\n[safe]\n\tdirectory = *\n";

/// 64 zeros: `${z64}` (the null SHA-256 object name; `${z40}` is its SHA-1 prefix).
const ZEROS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// 2020-01-01T00:00:00Z: the first commit date; every object-creating git command adds one second.
pub(crate) const BASE_DATE: u64 = 1_577_836_800;

/// The directories of a root.
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    /// `R`.
    pub root: PathBuf,
    /// `R/home`.
    pub home: PathBuf,
    /// `R/store` (`PRE_COMMIT_HOME`).
    pub store: PathBuf,
    /// `R/snp-store` (`SNP_HOME`).
    pub snp_store: PathBuf,
    /// `R/tmp`.
    pub tmp: PathBuf,
    /// `R/bin`.
    pub bin: PathBuf,
    /// `R/repos`.
    pub repos: PathBuf,
    /// The work repository (`R/work` unless the scenario says otherwise).
    pub work: PathBuf,
}

impl Layout {
    /// The layout of `root` for `work`.
    pub(crate) fn new(root: &Path, work: &Work) -> Self {
        Self {
            root: root.to_path_buf(),
            home: root.join("home"),
            store: root.join("store"),
            snp_store: root.join("snp-store"),
            tmp: root.join("tmp"),
            bin: root.join("bin"),
            repos: root.join("repos"),
            work: root.join(work.dir.as_deref().unwrap_or("work")),
        }
    }
}

/// A materialised scenario.
#[derive(Debug, Clone)]
pub(crate) struct World {
    /// Its directories.
    pub layout: Layout,
    /// The hermetic child environment (without the per-step changes).
    pub env: Env,
    /// git, resolved once on the hermetic `PATH`.
    pub git: PathBuf,
    /// `sh`, when present (posix shells; Git for Windows).
    pub sh: Option<PathBuf>,
    /// The next date tick.
    pub tick: u64,
    /// `${…}` substitutions.
    pub vars: Vec<(String, String)>,
}

/// Process variables a child may inherit (10 §7.3.2).
const INHERITED: [&str; 5] = ["PATH", "SYSTEMROOT", "PATHEXT", "COMSPEC", "WINDIR"];

/// Removes `root` if it is a harness root (or empty), then recreates it with the marker.
pub(crate) fn reset_root(root: &Path) -> Result<(), String> {
    if root.exists() {
        let marked = root.join(MARKER).is_file();
        let empty = std::fs::read_dir(root)
            .map_err(|e| format!("{}: {e}", root.display()))?
            .next()
            .is_none();
        if !marked && !empty {
            return Err(format!(
                "refusing to delete {}: not a difftest root (no {MARKER})",
                root.display()
            ));
        }
        make_writable(root);
        std::fs::remove_dir_all(root).map_err(|e| format!("removing {}: {e}", root.display()))?;
    }
    std::fs::create_dir_all(root).map_err(|e| format!("creating {}: {e}", root.display()))?;
    std::fs::write(root.join(MARKER), b"").map_err(|e| format!("{}: {e}", root.display()))
}

/// Removes a harness root (one carrying the marker); anything else is left alone.
pub(crate) fn remove_root(root: &Path) -> Result<(), String> {
    if !root.join(MARKER).is_file() {
        return Err(format!(
            "refusing to delete {}: not a difftest root (no {MARKER})",
            root.display()
        ));
    }
    make_writable(root);
    std::fs::remove_dir_all(root).map_err(|e| format!("removing {}: {e}", root.display()))
}

/// chmod u+rwx every directory below `p` (a scenario may leave `chmod 000` directories), never following symlinks.
fn make_writable(p: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(md) = std::fs::symlink_metadata(p) else {
            return;
        };
        if md.is_dir() {
            let mode = md.permissions().mode() | 0o700;
            drop(std::fs::set_permissions(
                p,
                std::fs::Permissions::from_mode(mode),
            ));
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    make_writable(&e.path());
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let Ok(md) = std::fs::symlink_metadata(p) else {
            return;
        };
        let mut perm = md.permissions();
        #[expect(
            clippy::permissions_set_readonly_false,
            reason = "Windows: clear the read-only attribute before deletion"
        )]
        perm.set_readonly(false);
        let _ = std::fs::set_permissions(p, perm);
        if md.is_dir()
            && let Ok(rd) = std::fs::read_dir(p)
        {
            for e in rd.flatten() {
                make_writable(&e.path());
            }
        }
    }
}

/// `os.path.realpath` of `p` once it exists (macOS `/tmp` → `/private/tmp`), without Windows verbatim prefixes.
pub(crate) fn canonical(p: &Path) -> Result<PathBuf, String> {
    #[expect(
        clippy::disallowed_methods,
        reason = "dev-only harness; snp_os is not an edge of snp-difftest (ARCHITECTURE §1.2)"
    )]
    let c = std::fs::canonicalize(p).map_err(|e| format!("canonicalising {}: {e}", p.display()))?;
    let s = c.to_string_lossy();
    Ok(s.strip_prefix(r"\\?\")
        .map_or_else(|| c.clone(), PathBuf::from))
}

/// The hermetic environment of a root (10 §7.3.2), `R/bin` first on `PATH`.
pub(crate) fn hermetic_env(l: &Layout, parent: &Env) -> Env {
    let mut pairs: Vec<(OsString, OsString)> = Vec::new();
    for k in INHERITED {
        if k == "PATH" || (k == "WINDIR" && !cfg!(windows)) {
            continue;
        }
        if let Some(v) = parent.get(k) {
            pairs.push((k.into(), v.to_os_string()));
        }
    }
    let mut path = l.bin.as_os_str().to_os_string();
    if let Some(p) = parent.get("PATH") {
        path.push(if cfg!(windows) { ";" } else { ":" });
        path.push(p);
    }
    pairs.push(("PATH".into(), path));
    let gitconfig = l.home.join(".gitconfig");
    // git never searches a ceiling directory itself, only below it: the ceiling is the root's parent, so a step whose
    // cwd is the root (or a non-repository work dir) is "not a git repository" even when the harness root lies inside
    // a checkout (the default `target/difftest/roots` does), instead of running against the enclosing repository.
    let ceiling = l.root.parent().unwrap_or(&l.root);
    let vars: [(&str, &Path); 7] = [
        ("HOME", &l.home),
        ("PRE_COMMIT_HOME", &l.store),
        ("SNP_HOME", &l.snp_store),
        ("GIT_CONFIG_GLOBAL", &gitconfig),
        ("TMPDIR", &l.tmp),
        ("GIT_CEILING_DIRECTORIES", ceiling),
        ("USERPROFILE", &l.home),
    ];
    for (k, v) in vars {
        if k != "USERPROFILE" || cfg!(windows) {
            pairs.push((k.into(), v.as_os_str().to_os_string()));
        }
    }
    pairs.push((
        "XDG_CACHE_HOME".into(),
        l.home.join(".cache").into_os_string(),
    ));
    pairs.push((
        "GIT_TEMPLATE_DIR".into(),
        l.root.join("git-template").into_os_string(),
    ));
    if cfg!(windows) {
        pairs.push(("TEMP".into(), l.tmp.as_os_str().to_os_string()));
        pairs.push(("TMP".into(), l.tmp.as_os_str().to_os_string()));
    }
    let date = git_date(0);
    for (k, v) in [
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_AUTHOR_NAME", "test"),
        ("GIT_COMMITTER_NAME", "test"),
        ("GIT_AUTHOR_EMAIL", "test@example.com"),
        ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ("GIT_AUTHOR_DATE", date.as_str()),
        ("GIT_COMMITTER_DATE", date.as_str()),
        ("GIT_ALLOW_PROTOCOL", "file"),
        ("LC_ALL", "C.UTF-8"),
        ("LANG", "C.UTF-8"),
        ("TZ", "UTC"),
        ("VIRTUALENV_NO_DOWNLOAD", "1"),
        // Python's str hashes are randomised per process: set and dict-of-set orders (e.g. which pair of mixed-type
        // keys `sorted()` compares first) would make the oracle disagree with itself. snp ignores the variable.
        ("PYTHONHASHSEED", "0"),
    ] {
        pairs.push((k.into(), v.into()));
    }
    Env::from_pairs(pairs)
}

/// git's raw date of tick `n`.
pub(crate) fn git_date(n: u64) -> String {
    format!("{} +0000", BASE_DATE.saturating_add(n))
}

/// Builds the scenario at `root` (which must already be reset). `probe`: the snp-probe binary for `${probe}`.
pub(crate) fn materialise(
    sc: &Scenario,
    root: &Path,
    probe: Option<&Path>,
    parent: &Env,
) -> Result<World, String> {
    let layout = Layout::new(root, &sc.work);
    for d in [
        &layout.home,
        &layout.home.join(".cache"),
        &layout.store,
        &layout.snp_store,
        &layout.tmp,
        &layout.bin,
        &layout.repos,
        &root.join("git-template"),
    ] {
        std::fs::create_dir_all(d).map_err(|e| format!("creating {}: {e}", d.display()))?;
    }
    std::fs::write(layout.home.join(".gitconfig"), GITCONFIG)
        .map_err(|e| format!("writing .gitconfig: {e}"))?;
    let env = hermetic_env(&layout, parent);
    let git = exec::which("git", &env).ok_or("git is not on PATH")?;
    let sh = exec::which("sh", &env);
    let mut vars = vec![
        (String::from("root"), slash(&layout.root)),
        (String::from("work"), slash(&layout.work)),
        (String::from("home"), slash(&layout.home)),
        (String::from("store"), slash(&layout.store)),
        (String::from("snp_store"), slash(&layout.snp_store)),
    ];
    if let Some(p) = probe {
        vars.push((String::from("probe"), sh_quote(&slash(p))));
        vars.push((String::from("probe_path"), slash(p)));
    }
    vars.push((
        String::from("z40"),
        ZEROS.get(..40).unwrap_or(ZEROS).to_owned(),
    ));
    vars.push((String::from("z64"), ZEROS.to_owned()));
    if let Some(p) = env.get("PATH") {
        vars.push((String::from("PATH"), p.to_string_lossy().into_owned()));
    }
    let mut w = World {
        layout,
        env,
        git,
        sh,
        tick: 0,
        vars,
    };
    for r in &sc.repos {
        build_repo(&mut w, r)?;
    }
    if sc.projects.is_empty() {
        build_work(&mut w, &sc.work)?;
    } else {
        for p in &sc.projects {
            w.layout.work = root.join(p);
            build_work(&mut w, &sc.work)?;
        }
        if let Some(first) = sc.projects.first() {
            w.layout.work = root.join(first);
        }
    }
    Ok(w)
}

/// A path with forward slashes on Windows (as pre-commit prints most paths).
fn slash(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s
    }
}

impl World {
    /// Replaces every `${name}` / `${repo:x}` / `${rev:x}` / `${url:x}` in `s`; unknown names are an error (a typo
    /// would otherwise compare two identical wrong runs).
    pub(crate) fn subst(&self, s: &str) -> Result<String, String> {
        let mut out = String::new();
        let mut rest = s;
        while let Some(start) = rest.find("${") {
            let (head, tail) = rest.split_at_checked(start).ok_or("bad substitution")?;
            out.push_str(head);
            let tail = tail.get(2..).unwrap_or_default();
            let end = tail
                .find('}')
                .ok_or_else(|| format!("unterminated ${{ in {s:?}"))?;
            let (name, after) = tail.split_at_checked(end).ok_or("bad substitution")?;
            let value = self
                .vars
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| format!("unknown substitution ${{{name}}}"))?;
            out.push_str(&value);
            rest = after.get(1..).unwrap_or_default();
        }
        out.push_str(rest);
        Ok(out)
    }

    /// [`World::subst`] on bytes that are UTF-8; other bytes pass unchanged.
    pub(crate) fn subst_bytes(&self, b: &[u8]) -> Result<Vec<u8>, String> {
        match std::str::from_utf8(b) {
            Ok(s) if s.contains("${") => self.subst(s).map(String::into_bytes),
            _ => Ok(b.to_vec()),
        }
    }

    /// Runs git in `cwd` at the current tick (advanced first for object-creating commands).
    pub(crate) fn git<S: AsRef<OsStr>>(
        &mut self,
        cwd: &Path,
        args: &[S],
    ) -> Result<Captured, String> {
        let creates = args.first().is_some_and(|a| {
            [
                "commit",
                "merge",
                "tag",
                "cherry-pick",
                "rebase",
                "revert",
                "stash",
                "am",
            ]
            .iter()
            .any(|c| a.as_ref() == OsStr::new(c))
        });
        if creates {
            self.tick = self.tick.saturating_add(1);
        }
        let date = git_date(self.tick);
        let env = self.env.patched(
            &snp_core::env::EnvPatch::default()
                .set("GIT_AUTHOR_DATE", date.clone())
                .set("GIT_COMMITTER_DATE", date),
        );
        exec::run(&Invocation {
            program: self.git.clone(),
            args: args.iter().map(|a| a.as_ref().to_os_string()).collect(),
            env,
            cwd: cwd.to_path_buf(),
            stdin: Vec::new(),
            tty: false,
            timeout: Duration::from_secs(120),
            signal: None,
        })
    }

    /// [`World::git`] that must succeed; returns stdout.
    pub(crate) fn git_ok<S: AsRef<OsStr>>(
        &mut self,
        cwd: &Path,
        args: &[S],
    ) -> Result<Vec<u8>, String> {
        let out = self.git(cwd, args)?;
        if out.exit != Some(0) {
            let shown: Vec<String> = args
                .iter()
                .map(|a| a.as_ref().to_string_lossy().into_owned())
                .collect();
            return Err(format!(
                "git {shown:?} in {} failed ({:?}): {}",
                cwd.display(),
                out.exit,
                String::from_utf8_lossy(&out.stderr).trim_end()
            ));
        }
        Ok(out.stdout)
    }

    fn shell(&self, cwd: &Path, script: &str) -> Result<(), String> {
        let sh = self.sh.clone().ok_or("sh is not on PATH")?;
        let out = exec::run(&Invocation {
            program: sh,
            args: vec!["-c".into(), script.into()],
            env: self.env.clone(),
            cwd: cwd.to_path_buf(),
            stdin: Vec::new(),
            tty: false,
            timeout: Duration::from_secs(120),
            signal: None,
        })?;
        if out.exit == Some(0) {
            Ok(())
        } else {
            Err(format!(
                "setup shell {script:?} failed ({:?}): {}",
                out.exit,
                String::from_utf8_lossy(&out.stderr).trim_end()
            ))
        }
    }

    fn rev(&mut self, dir: &Path) -> Result<String, String> {
        let out = self.git_ok(dir, &["rev-parse", "HEAD"])?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    fn apply_ops(&mut self, dir: &Path, ops: &[Op]) -> Result<(), String> {
        for op in ops {
            match op {
                Op::Git(args) => {
                    let args: Vec<String> = args
                        .iter()
                        .map(|a| self.subst(a))
                        .collect::<Result<_, _>>()?;
                    self.git_ok(dir, &args)?;
                }
                Op::Write(p, b) => {
                    let b = self.subst_bytes(b)?;
                    write_file(&confined(&self.layout.root, dir, p)?, &b)?;
                }
                Op::Remove(p) => {
                    let full = confined(&self.layout.root, dir, p)?;
                    // The lexical check above cannot see symlinked parents: the real parent must be inside too.
                    let real_root =
                        canonical(&self.layout.root).map_err(|e| format!("remove {p}: {e}"))?;
                    let real_parent = full
                        .parent()
                        .map(canonical)
                        .transpose()
                        .map_err(|e| format!("remove {p}: {e}"))?;
                    if !real_parent.is_some_and(|rp| rp.starts_with(&real_root)) {
                        return Err(format!("remove {p}: leaves the scenario root"));
                    }
                    let md =
                        std::fs::symlink_metadata(&full).map_err(|e| format!("remove {p}: {e}"))?;
                    if md.is_dir() {
                        std::fs::remove_dir_all(&full)
                    } else {
                        std::fs::remove_file(&full)
                    }
                    .map_err(|e| format!("remove {p}: {e}"))?;
                }
                Op::Shell(script) => {
                    let script = self.subst(script)?;
                    self.shell(dir, &script)?;
                }
                Op::Commit(msg) => {
                    self.git_ok(dir, &["add", "-A"])?;
                    self.git_ok(dir, &["commit", "-q", "--allow-empty", "-m", msg])?;
                }
                Op::Tag(t) => {
                    self.git_ok(dir, &["tag", t])?;
                }
            }
        }
        Ok(())
    }
}

/// `base/rel`, resolved lexically, refused unless it stays below the scenario root `root`: absolute paths, drive
/// prefixes and `..` that climb out are errors (a typo such as `remove: ../` must never touch files outside the
/// harness root). `base` is a directory inside `root`.
pub(crate) fn confined<P: AsRef<Path>>(
    root: &Path,
    base: &Path,
    rel: P,
) -> Result<PathBuf, String> {
    use std::path::Component;
    let rel = rel.as_ref();
    let mut out = base.to_path_buf();
    for c in rel.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    break;
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "{}: scenario paths must be relative to the scenario root",
                    rel.display()
                ));
            }
        }
    }
    if out.starts_with(root) && out != root {
        Ok(out)
    } else {
        Err(format!(
            "{}: leaves the scenario root {}",
            rel.display(),
            root.display()
        ))
    }
}

/// Writes `bytes` to `p`, creating parents.
pub(crate) fn write_file(p: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    std::fs::write(p, bytes).map_err(|e| format!("writing {}: {e}", p.display()))
}

fn set_mode(p: &Path, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
            .map_err(|e| format!("chmod {}: {e}", p.display()))
    }
    #[cfg(not(unix))]
    {
        let _: (&Path, u32) = (p, mode);
        Ok(())
    }
}

fn make_exec(p: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let md = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
        set_mode(p, md.permissions().mode() | 0o111)
    }
    #[cfg(not(unix))]
    {
        let _: &Path = p;
        Ok(())
    }
}

/// A path from raw bytes (non-UTF-8 names are posix-only).
#[cfg_attr(
    unix,
    expect(
        clippy::unnecessary_wraps,
        reason = "fallible on Windows (non-UTF-8 names)"
    )
)]
fn path_from_bytes(b: &[u8]) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(OsStr::from_bytes(b)))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(b)
            .map(PathBuf::from)
            .map_err(|_| String::from("non-UTF-8 file names need a posix host"))
    }
}

pub(crate) fn os_from_bytes(b: &[u8]) -> Result<OsString, String> {
    path_from_bytes(b).map(PathBuf::into_os_string)
}

/// The testkit's recreation of pre-commit's `testing/resources/<name>` (10 §3).
pub(crate) fn fixture_dir(name: &str) -> PathBuf {
    crate::workspace_root()
        .join("crates/snp-testkit/resources")
        .join(name)
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    let entries =
        std::fs::read_dir(src).map_err(|e| format!("reading fixture {}: {e}", src.display()))?;
    for e in entries {
        let e = e.map_err(|e| format!("{}: {e}", src.display()))?;
        let from = e.path();
        let to = dst.join(e.file_name());
        let ft = e
            .file_type()
            .map_err(|e| format!("{}: {e}", from.display()))?;
        if ft.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| format!("copying {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

fn build_repo(w: &mut World, r: &RepoSpec) -> Result<(), String> {
    if let Some((url, rev)) = &r.external {
        w.vars.push((format!("repo:{}", r.name), url.clone()));
        w.vars.push((format!("url:{}", r.name), url.clone()));
        w.vars.push((format!("rev:{}", r.name), rev.clone()));
        return Ok(());
    }
    let dir = match &r.path {
        Some(p) => confined(&w.layout.root, &w.layout.root, p)?,
        None => confined(&w.layout.root, &w.layout.repos, &r.name)?,
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let mut init: Vec<String> = vec![String::from("init"), String::from("-q")];
    if r.bare {
        init.push(String::from("--bare"));
    }
    if let Some(f) = &r.object_format {
        init.push(format!("--object-format={f}"));
    }
    init.push(String::from("."));
    w.git_ok(&dir, &init)?;
    let path_s = slash(&dir);
    w.vars.push((format!("repo:{}", r.name), path_s.clone()));
    w.vars
        .push((format!("url:{}", r.name), format!("file://{path_s}")));
    if r.bare {
        return Ok(());
    }
    if let Some(f) = &r.fixture {
        let src = fixture_dir(f);
        if !src.is_dir() {
            return Err(format!("unknown fixture {f:?} (no {})", src.display()));
        }
        copy_tree(&src, &dir)?;
    }
    for (p, b) in &r.files {
        let b = w.subst_bytes(b)?;
        write_file(&confined(&w.layout.root, &dir, p)?, &b)?;
    }
    for p in &r.exec {
        make_exec(&confined(&w.layout.root, &dir, p)?)?;
    }
    w.git_ok(&dir, &["add", "-A"])?;
    for p in &r.exec {
        w.git_ok(&dir, &["update-index", "--chmod=+x", "--", p])?;
    }
    w.git_ok(&dir, &["commit", "-q", "--allow-empty", "-m", "Add hooks"])?;
    for t in &r.tags {
        w.git_ok(&dir, &["tag", t])?;
    }
    w.apply_ops(&dir, &r.then)?;
    let rev = w.rev(&dir)?;
    w.vars.push((format!("rev:{}", r.name), rev));
    Ok(())
}

fn build_work(w: &mut World, work: &Work) -> Result<(), String> {
    let dir = w.layout.work.clone();
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    if work.git {
        let mut init: Vec<String> = vec![String::from("init"), String::from("-q")];
        if let Some(f) = &work.object_format {
            init.push(format!("--object-format={f}"));
        }
        init.push(String::from("."));
        w.git_ok(&dir, &init)?;
        for (k, v) in &work.git_config {
            let v = w.subst(v)?;
            w.git_ok(&dir, &["config", k.as_str(), v.as_str()])?;
        }
    }
    for (p, b) in &work.files {
        let b = w.subst_bytes(b)?;
        write_file(&confined(&w.layout.root, &dir, path_from_bytes(p)?)?, &b)?;
    }
    if let Some(c) = &work.config {
        let c = w.subst(c)?;
        write_file(&dir.join(".pre-commit-config.yaml"), c.as_bytes())?;
    }
    for p in &work.exec {
        make_exec(&confined(&w.layout.root, &dir, p)?)?;
    }
    for (link, target) in &work.symlinks {
        symlink(&w.subst(target)?, &confined(&w.layout.root, &dir, link)?)?;
    }
    if work.git && !work.commit.is_empty() {
        let mut add: Vec<OsString> = vec!["add".into(), "--".into()];
        for p in &work.commit {
            add.push(os_from_bytes(p)?);
        }
        w.git_ok(&dir, &add)?;
        for p in &work.exec {
            if work.commit.iter().any(|c| c == p.as_bytes()) {
                w.git_ok(&dir, &["update-index", "--chmod=+x", "--", p])?;
            }
        }
        w.git_ok(&dir, &["commit", "-q", "-m", "init"])?;
    }
    // `modes` apply after the commit (a mode-000 file is committed first, then made unreadable).
    for (p, m) in &work.modes {
        set_mode(&confined(&w.layout.root, &dir, p)?, *m)?;
    }
    if work.git && !work.stage.is_empty() {
        let mut add: Vec<OsString> = vec!["add".into(), "--".into()];
        for p in &work.stage {
            add.push(os_from_bytes(p)?);
        }
        w.git_ok(&dir, &add)?;
    }
    for (p, b) in &work.unstaged {
        let full = confined(&w.layout.root, &dir, p)?;
        match b {
            Some(b) => {
                let b = w.subst_bytes(b)?;
                write_file(&full, &b)?;
            }
            None => std::fs::remove_file(&full).map_err(|e| format!("removing {p}: {e}"))?,
        }
    }
    for f in &work.fifos {
        mkfifo(w, &dir, f)?;
    }
    w.apply_ops(&dir, &work.setup)?;
    for (p, m) in &work.chmod {
        set_mode(&confined(&w.layout.root, &dir, p)?, *m)?;
    }
    if work.git
        && let Ok(out) = w.git(&dir, &["rev-parse", "-q", "--verify", "HEAD"])
        && out.exit == Some(0)
    {
        w.vars.push((
            String::from("head"),
            String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        ));
    }
    Ok(())
}

fn symlink(target: &str, link: &Path) -> Result<(), String> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
            .map_err(|e| format!("symlink {}: {e}", link.display()))
    }
    #[cfg(not(unix))]
    {
        let _: &str = target;
        Err(format!(
            "symlink {}: symlinks are posix-only in the harness",
            link.display()
        ))
    }
}

fn mkfifo(w: &World, dir: &Path, name: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            confined(&w.layout.root, dir, name)?,
            rustix::fs::Mode::from_raw_mode(0o644),
        )
        .map_err(|e| format!("mkfifo {name}: {e}"))
    }
    #[cfg(not(unix))]
    {
        let _: (&World, &Path) = (w, dir);
        Err(format!("mkfifo {name}: FIFOs are posix-only"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::Scenario;

    #[test]
    #[expect(
        clippy::literal_string_with_formatting_args,
        reason = "`${…}` is the materialiser's substitution syntax"
    )]
    fn substitutions() {
        let w = World {
            layout: Layout::new(Path::new("/r"), &Work::default()),
            env: Env::from_pairs::<&str, &str>([]),
            git: PathBuf::from("git"),
            sh: None,
            tick: 0,
            vars: vec![
                (String::from("root"), String::from("/r")),
                (String::from("repo:h"), String::from("/r/repos/h")),
            ],
        };
        assert_eq!(
            w.subst("a ${root}/x ${repo:h}").unwrap(),
            "a /r/x /r/repos/h"
        );
        assert!(w.subst("${nope}").is_err());
        assert!(w.subst("${root").is_err());
        assert_eq!(w.subst_bytes(b"\xff${root}").unwrap(), b"\xff${root}");
    }

    #[test]
    fn reset_refuses_foreign_directories() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("precious"), b"x").unwrap();
        assert!(reset_root(d.path()).is_err());
        assert!(d.path().join("precious").exists());
        let r = d.path().join("root");
        reset_root(&r).unwrap();
        std::fs::write(r.join("junk"), b"x").unwrap();
        reset_root(&r).unwrap();
        assert!(!r.join("junk").exists());
    }

    #[test]
    fn builds_identical_shas_twice() {
        let text = "name: t\nrepos:\n  hooks: {fixture: script_hooks_repo}\nwork:\n  files: {a.txt: \"a\\n\"}\n  config: \"repos:\\n-   repo: ${repo:hooks}\\n    rev: ${rev:hooks}\\n    hooks: [{id: bash_hook}]\\n\"\n  commit: [.pre-commit-config.yaml]\n  stage: [a.txt]\nsteps: [{tool: [run]}]\n";
        let sc = Scenario::parse(text, Path::new("t.yaml")).unwrap();
        let d = tempfile::tempdir().unwrap();
        let root = canonical(d.path()).unwrap().join("r");
        let parent = Env::capture_process();
        let mut revs = Vec::new();
        for _ in 0..2 {
            reset_root(&root).unwrap();
            let mut w = materialise(&sc, &root, None, &parent).unwrap();
            let work = w.layout.work.clone();
            revs.push((
                w.subst("${rev:hooks}").unwrap(),
                String::from_utf8(w.git_ok(&work, &["rev-parse", "HEAD"]).unwrap()).unwrap(),
            ));
            let cfg = std::fs::read_to_string(work.join(".pre-commit-config.yaml")).unwrap();
            assert!(cfg.contains(&format!("repo: {}", slash(&root.join("repos/hooks")))));
        }
        assert_eq!(revs[0], revs[1]);
        assert_eq!(revs[0].0.len(), 40);
    }

    #[test]
    fn confined_refuses_paths_outside_the_root() {
        let root = Path::new("/r");
        let work = Path::new("/r/work");
        assert_eq!(
            confined(root, work, "a/b").unwrap(),
            Path::new("/r/work/a/b")
        );
        assert_eq!(
            confined(root, work, "../tools/x").unwrap(),
            Path::new("/r/tools/x")
        );
        assert_eq!(confined(root, work, "./a").unwrap(), Path::new("/r/work/a"));
        for bad in ["../", "../..", "../../etc", "/home/x", "a/../../.."] {
            assert!(confined(root, work, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn scenario_ops_cannot_touch_files_outside_the_root() {
        let d = tempfile::tempdir().unwrap();
        let base = canonical(d.path()).unwrap();
        std::fs::write(base.join("precious"), b"x").unwrap();
        let root = base.join("r");
        let parent = Env::capture_process();
        for setup in [
            "[{remove: ../..}]",
            "[{remove: ../../precious}]",
            "[{write: {../../precious: y}}]",
        ] {
            let text = format!(
                "name: t\nwork:\n  files: {{a.txt: a}}\n  setup: {setup}\nsteps: [{{tool: [run]}}]\n"
            );
            let sc = Scenario::parse(&text, Path::new("t.yaml")).unwrap();
            reset_root(&root).unwrap();
            assert!(materialise(&sc, &root, None, &parent).is_err(), "{setup}");
            assert_eq!(
                std::fs::read(base.join("precious")).unwrap(),
                b"x",
                "{setup}"
            );
        }
        let text = "name: t\nwork:\n  files: {../../precious: y}\nsteps: [{tool: [run]}]\n";
        let sc = Scenario::parse(text, Path::new("t.yaml")).unwrap();
        reset_root(&root).unwrap();
        assert!(materialise(&sc, &root, None, &parent).is_err());
        assert_eq!(std::fs::read(base.join("precious")).unwrap(), b"x");
    }
}
