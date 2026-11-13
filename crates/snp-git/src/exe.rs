//! The resolved git executable, its version and capabilities (02 G.2, G.4, G.10; ADR-051, ADR-063, ADR-079).
//!
//! Owner: WP-18.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::output::Printer;
use snp_proc::spawn::{self, Group, Role, Spec, Stdin};

use crate::messages;

/// `git --version` triple (02 G.4 "Parsing `git --version`").
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitVersion {
    /// Major.
    pub major: u16,
    /// Minor.
    pub minor: u16,
    /// Patch.
    pub patch: u16,
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl GitVersion {
    pub(crate) const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// `git --version` stdout: must start with `git version `, then `^(\d+)\.(\d+)(?:\.(\d+))?` (a missing patch is
    /// 0, any suffix is ignored: `2.39.5 (Apple Git-154)`, `2.47.1.windows.2`, `2.56.0.rc0`). Out-of-range numbers
    /// make the output unparseable (`None`, "treat as newest", 02 G.4).
    pub(crate) fn parse(stdout: &[u8]) -> Option<Self> {
        let rest = stdout.strip_prefix(b"git version ")?;
        let (major, rest) = number(rest)?;
        let rest = rest.strip_prefix(b".")?;
        let (minor, rest) = number(rest)?;
        let patch = rest
            .strip_prefix(b".")
            .and_then(number)
            .map_or(0, |(p, _)| p);
        Some(Self::new(major, minor, patch))
    }
}

/// A leading run of ASCII digits as a `u16` (Python `\d` also matches non-ASCII digits, which git never prints).
fn number(b: &[u8]) -> Option<(u16, &[u8])> {
    let n = b.iter().take_while(|c| c.is_ascii_digit()).count();
    let (digits, rest) = b.split_at_checked(n)?;
    if digits.is_empty() {
        return None;
    }
    let v = digits.iter().try_fold(0_u16, |acc, &d| {
        acc.checked_mul(10)?
            .checked_add(u16::from(d.wrapping_sub(b'0')))
    })?;
    Some((v, rest))
}

/// The first version without capability fallbacks: `ls-files --deduplicate`, `rev-parse --path-format`,
/// `GIT_CONFIG_COUNT` (02 G.2); below it the D-DIAG-01 warning is printed.
pub(crate) const FLOOR: GitVersion = GitVersion::new(2, 31, 0);
/// `git init --object-format=` (02 G.2).
const OBJECT_FORMAT: GitVersion = GitVersion::new(2, 29, 0);
/// `checkout --pathspec-from-file` / `--pathspec-file-nul` (git 2.25.0).
const PATHSPEC_FROM_FILE: GitVersion = GitVersion::new(2, 25, 0);
/// `diff --no-relative` and `diff.relative` (git 2.28.0).
const DIFF_NO_RELATIVE: GitVersion = GitVersion::new(2, 28, 0);
/// Config-based hooks `hook.<name>.{command,event}` (02 G.5).
const CONFIG_HOOKS: GitVersion = GitVersion::new(2, 54, 0);

/// Capability fallbacks by version band (02 G.2, G.10).
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per git capability (02 G.2)"
)]
pub struct GitCaps {
    /// The parsed version (`None`: unparseable, treated as the newest git).
    pub version: Option<GitVersion>,
    /// `ls-files --deduplicate` (≥ 2.31; else `ls-files -z` + adjacent dedup, D-CRASH-26).
    pub ls_files_dedup: bool,
    /// `rev-parse --path-format=absolute` (≥ 2.31; else separate `rev-parse` calls).
    pub path_format: bool,
    /// `GIT_CONFIG_COUNT` (≥ 2.31; else the owned-repo fsmonitor injection is skipped).
    pub config_env: bool,
    /// `init --object-format` (≥ 2.29; the D-SEC-06 retry).
    pub init_object_format: bool,
    /// Config-based hooks (≥ 2.54).
    pub config_hooks: bool,
    /// `checkout --pathspec-from-file --pathspec-file-nul` (≥ 2.25; else D-DATA-19's checkout passes the literal
    /// pathspecs as arguments, in chunks).
    pub pathspec_from_file: bool,
    /// `diff --no-relative` (≥ 2.28, with the `diff.relative` setting it overrides; older git needs neither).
    pub diff_no_relative: bool,
}

impl GitCaps {
    /// The capabilities of `version` (`None` = newest).
    pub(crate) fn for_version(version: Option<GitVersion>) -> Self {
        let at_least = |min: GitVersion| version.is_none_or(|v| v >= min);
        Self {
            version,
            ls_files_dedup: at_least(FLOOR),
            path_format: at_least(FLOOR),
            config_env: at_least(FLOOR),
            init_object_format: at_least(OBJECT_FORMAT),
            config_hooks: at_least(CONFIG_HOOKS),
            pathspec_from_file: at_least(PATHSPEC_FROM_FILE),
            diff_no_relative: at_least(DIFF_NO_RELATIVE),
        }
    }
}

/// `git` resolved once via `normalize_cmd(["git"])` so `CalledProcessError` argv matches pre-commit (02 G.10).
#[derive(Debug)]
pub struct GitExe {
    /// The resolved argv prefix (`parse_shebang.normalize_cmd(('git',))`: the absolute git path, or interpreter +
    /// script for a script wrapper).
    pub argv0: Vec<OsString>,
    /// Capabilities.
    pub caps: GitCaps,
}

/// One warning per process (D-DIAG-01).
static OLD_GIT_WARNED: AtomicBool = AtomicBool::new(false);

impl GitExe {
    /// Version from `cache` when (exe realpath, size, mtime, `GIT_EXEC_PATH`) matches a passing entry, else
    /// `git --version`. Never persisted for a known proxy (macOS `/usr/bin/git` xcrun shim, scoop/asdf shims; ADR-079).
    /// `Git::discover` runs after this call (it needs `caps` for the 2.19–2.30 fallback); a cache hit costs one stat.
    ///
    /// `git` not found (or a directory, or not executable) is pre-commit's first observable failure of every git
    /// command: `get_root`'s `FatalError: git failed. Is it installed, and are you in a Git repository directory?`
    /// (`git.py:51-73`, `util.py:93-97`). An unparseable or failing `git --version` gives `version: None`.
    pub fn resolve(env: &Env, cwd: &Path, cache: Option<&Path>) -> Result<Self, Failure> {
        let argv0 = snp_proc::resolve::normalize_cmd(&[OsString::from("git")], env, cwd)
            .map_err(|_| Failure::fatal("FatalError", messages::GIT_FAILED))?;
        let key = cache_key(&argv0, env, cwd);
        if let (Some(path), Some(key)) = (cache, key.as_ref())
            && let Some(v) = cache_lookup(path, key)
        {
            return Ok(Self {
                argv0,
                caps: GitCaps::for_version(Some(v)),
            });
        }
        let version = spawn_version(&argv0, env, cwd);
        if let (Some(path), Some(key), Some(v)) = (cache, key.as_ref(), version)
            && v >= FLOOR
        {
            cache_store(path, key, v);
        }
        Ok(Self {
            argv0,
            caps: GitCaps::for_version(version),
        })
    }

    /// Never refuses (ADR-063): < 2.31 → one stderr warning per process unless `SNP_ALLOW_OLD_GIT` (D-DIAG-01) and the
    /// capability fallbacks; the < 2.19 stash refusal lives in snp-stash (D-DATA-17); config hooks need ≥ 2.54.
    ///
    /// The only error is a Cygwin/MSYS2 git on Windows (D-PLAT-08): pre-commit only warns there (`git.py:210-230`,
    /// dead code on native Windows Python) and then misbehaves; snp stops with a `FatalError` (exit 1).
    pub fn version_gate(&self, printer: &Printer, env: &Env) -> Result<(), Failure> {
        self.gate(printer, env, &OLD_GIT_WARNED)
    }

    /// [`Self::version_gate`] with an explicit once-latch (tests).
    pub(crate) fn gate(
        &self,
        printer: &Printer,
        env: &Env,
        latch: &AtomicBool,
    ) -> Result<(), Failure> {
        if cfg!(windows)
            && let Some(exe) = self.argv0.first()
            && is_cygwin_like(Path::new(exe))
        {
            return Err(Failure::fatal(
                "FatalError",
                messages::cygwin_git(printer.brand(), exe),
            ));
        }
        if let Some(v) = self.caps.version
            && v < FLOOR
            && env.get_nonempty("SNP_ALLOW_OLD_GIT").is_none()
            && !latch.swap(true, Ordering::AcqRel)
        {
            printer.diag(&messages::old_git_warning(v, printer));
        }
        Ok(())
    }

    /// `git <V>` for messages: the parsed version, or `(unknown version)`.
    pub(crate) fn version_text(&self) -> String {
        self.caps
            .version
            .map_or_else(|| "(unknown version)".to_owned(), |v| v.to_string())
    }
}

/// A Cygwin or MSYS2 build of git: its directory holds the POSIX emulation DLL (`cygwin1.dll`, `msys-2.0.dll`).
/// Git for Windows' `cmd\`, `bin\` and `mingw64\bin\` launchers never sit next to them (D-PLAT-08).
pub(crate) fn is_cygwin_like(exe: &Path) -> bool {
    exe.parent().is_some_and(|dir| {
        ["cygwin1.dll", "msys-2.0.dll"]
            .iter()
            .any(|dll| dir.join(dll).is_file())
    })
}

/// `git --version` through the W0 spawn baseline (Tool role, never cancelled: it is the first spawn of a run).
fn spawn_version(argv0: &[OsString], env: &Env, cwd: &Path) -> Option<GitVersion> {
    let mut argv = argv0.to_vec();
    argv.push(OsString::from("--version"));
    let cancel = CancelToken::new();
    let spec = Spec {
        argv: &argv,
        env,
        cwd,
        stdin: Stdin::Null,
        group: Group::Inherit,
        role: Role::Tool,
        deadline: None,
        cancel: &cancel,
    };
    let out = spawn::capture(&spec).ok()?;
    if out.status.0 != 0 {
        return None;
    }
    GitVersion::parse(&out.stdout)
}

/// The cache key of a resolved git (02 G.4 "Cost"): realpath, size, mtime (ns) and `GIT_EXEC_PATH`. `None` for a
/// known proxy, whose version can change without its own stat changing (ADR-079).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CacheKey {
    realpath: String,
    size: u64,
    mtime_ns: u128,
    exec_path: String,
}

fn hex(b: &[u8]) -> String {
    use fmt::Write as _;
    let mut s = String::new();
    for x in b {
        let _infallible: fmt::Result = write!(s, "{x:02x}");
    }
    s
}

fn cache_key(argv0: &[OsString], env: &Env, cwd: &Path) -> Option<CacheKey> {
    let [exe] = argv0 else {
        return None; // a script wrapper (shebang expanded by normalize_cmd): a proxy by definition
    };
    let real = snp_pycompat::ospath::realpath(exe, cwd).ok()?;
    if is_known_proxy(Path::new(exe)) || is_known_proxy(&real) {
        return None;
    }
    let md = std::fs::metadata(&real).ok()?;
    let mtime_ns = md
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(CacheKey {
        realpath: hex(real.as_os_str().as_encoded_bytes()),
        size: md.len(),
        mtime_ns,
        exec_path: env
            .get("GIT_EXEC_PATH")
            .map_or_else(|| "-".to_owned(), |p| hex(p.as_encoded_bytes())),
    })
}

/// Known proxies (ADR-079): the macOS `/usr/bin/git` xcrun shim, any `shims` directory (asdf, mise, pyenv-style,
/// scoop) and Chocolatey's `chocolatey\bin` shimgen launchers.
pub(crate) fn is_known_proxy(path: &Path) -> bool {
    if cfg!(target_os = "macos") && path == Path::new("/usr/bin/git") {
        return true;
    }
    let names: Vec<&OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n),
            _ => None,
        })
        .collect();
    let is = |n: &OsStr, want: &str| n.as_encoded_bytes().eq_ignore_ascii_case(want.as_bytes());
    names.iter().any(|n| is(n, "shims"))
        || names
            .iter()
            .zip(names.iter().skip(1))
            .any(|(a, b)| is(a, "chocolatey") && is(b, "bin"))
}

const CACHE_HEADER: &str = "snp-git-version-cache v1";
/// Entries kept (one per git binary seen; newest first).
const CACHE_ENTRIES: usize = 16;
/// The cache file is tiny; anything bigger is not ours.
const CACHE_CAP: u64 = 64 * 1024;

fn cache_line(key: &CacheKey, v: GitVersion) -> String {
    format!(
        "{v}\t{}\t{}\t{}\t{}",
        key.size, key.mtime_ns, key.exec_path, key.realpath
    )
}

/// Parses one entry: `(key, version)`.
fn parse_cache_line(line: &str) -> Option<(CacheKey, GitVersion)> {
    let mut f = line.split('\t');
    let version = GitVersion::parse(format!("git version {}", f.next()?).as_bytes())?;
    let size = f.next()?.parse().ok()?;
    let mtime_ns = f.next()?.parse().ok()?;
    let exec_path = f.next()?.to_owned();
    let realpath = f.next()?.to_owned();
    if f.next().is_some() {
        return None;
    }
    Some((
        CacheKey {
            realpath,
            size,
            mtime_ns,
            exec_path,
        },
        version,
    ))
}

fn cache_read(path: &Path) -> Vec<(CacheKey, GitVersion)> {
    let mut text = String::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(CACHE_CAP).read_to_string(&mut text));
    if read.is_err() {
        return Vec::new();
    }
    let mut lines = text.lines();
    if lines.next() != Some(CACHE_HEADER) {
        return Vec::new();
    }
    lines.filter_map(parse_cache_line).collect()
}

/// Only passing entries are trusted (02 G.4): a below-floor entry never short-cuts `git --version`.
pub(crate) fn cache_lookup(path: &Path, key: &CacheKey) -> Option<GitVersion> {
    cache_read(path)
        .into_iter()
        .find(|(k, v)| k == key && *v >= FLOOR)
        .map(|(_, v)| v)
}

/// Best effort (a read-only store is fine): temp file + rename; write errors are ignored.
pub(crate) fn cache_store(path: &Path, key: &CacheKey, v: GitVersion) {
    let mut out = format!("{CACHE_HEADER}\n{}\n", cache_line(key, v));
    for (k, old) in cache_read(path)
        .into_iter()
        .filter(|(k, _)| k.realpath != key.realpath || k.exec_path != key.exec_path)
        .take(CACHE_ENTRIES.saturating_sub(1))
    {
        out.push_str(&cache_line(&k, old));
        out.push('\n');
    }
    let write = || snp_os::fs::atomic_replace(path, out.as_bytes(), Some(0o600));
    // An existing store that lacks the cache directory (created by hand, or never through `ensure_created`) gets it
    // here (0700, one level): otherwise no run would ever persist the version and every run would spawn
    // `git --version`. A missing store is never created for a cache (lazy store, 03 §21 Q12).
    if let Err(e) = write()
        && e.kind() == std::io::ErrorKind::NotFound
        && let Some(dir) = path.parent()
        && dir.parent().is_some_and(Path::is_dir)
        && snp_os::fs::ensure_private_dir(dir).is_ok()
    {
        let _ignored: std::io::Result<()> = write();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Option<GitVersion> {
        GitVersion::parse(s.as_bytes())
    }

    #[test]
    fn version_parser_table() {
        // 02 G.4 "Parsing `git --version`".
        assert_eq!(v("git version 2.43.0\n"), Some(GitVersion::new(2, 43, 0)));
        assert_eq!(
            v("git version 2.39.5 (Apple Git-154)\n"),
            Some(GitVersion::new(2, 39, 5))
        );
        assert_eq!(
            v("git version 2.47.1.windows.2\n"),
            Some(GitVersion::new(2, 47, 1))
        );
        assert_eq!(
            v("git version 2.56.0.rc0\n"),
            Some(GitVersion::new(2, 56, 0))
        );
        assert_eq!(
            v("git version 2.56.0.123.gabcdef0.dirty\n"),
            Some(GitVersion::new(2, 56, 0))
        );
        assert_eq!(v("git version 2.31"), Some(GitVersion::new(2, 31, 0)));
        assert_eq!(v("git version 2.31.x"), Some(GitVersion::new(2, 31, 0)));
        assert_eq!(v("git version 2"), None);
        assert_eq!(v("git version x.1.2"), None);
        assert_eq!(v("git  version 2.43.0"), None);
        assert_eq!(v("hub version 2.14.2"), None);
        assert_eq!(v("git version 99999.1.1"), None);
        assert_eq!(v(""), None);
        assert_eq!(GitVersion::new(2, 30, 9).to_string(), "2.30.9");
    }

    #[test]
    fn caps_by_band() {
        let c = GitCaps::for_version(Some(GitVersion::new(2, 30, 9)));
        assert!(!c.ls_files_dedup && !c.path_format && !c.config_env);
        assert!(c.init_object_format && !c.config_hooks);
        let c = GitCaps::for_version(Some(GitVersion::new(2, 28, 0)));
        assert!(!c.init_object_format);
        let c = GitCaps::for_version(Some(GitVersion::new(2, 31, 0)));
        assert!(c.ls_files_dedup && c.path_format && c.config_env && !c.config_hooks);
        let c = GitCaps::for_version(Some(GitVersion::new(2, 54, 0)));
        assert!(c.config_hooks);
        let c = GitCaps::for_version(None);
        assert!(c.ls_files_dedup && c.path_format && c.config_env && c.init_object_format);
        assert!(c.config_hooks);
    }

    #[test]
    fn proxies() {
        assert!(is_known_proxy(Path::new("/home/u/.asdf/shims/git")));
        assert!(is_known_proxy(Path::new(
            "/home/u/.local/share/mise/shims/git"
        )));
        assert!(is_known_proxy(Path::new("/opt/Chocolatey/bin/git.exe")));
        assert!(!is_known_proxy(Path::new("/usr/local/bin/git")));
        assert!(!is_known_proxy(Path::new("/opt/bin/chocolatey")));
        assert_eq!(
            is_known_proxy(Path::new("/usr/bin/git")),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn cygwin_detection() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("git.exe");
        std::fs::write(&exe, b"").unwrap();
        assert!(!is_cygwin_like(&exe));
        std::fs::write(d.path().join("msys-2.0.dll"), b"").unwrap();
        assert!(is_cygwin_like(&exe));
        let d2 = tempfile::tempdir().unwrap();
        std::fs::write(d2.path().join("cygwin1.dll"), b"").unwrap();
        assert!(is_cygwin_like(&d2.path().join("git.exe")));
        assert!(!is_cygwin_like(Path::new("git.exe")));
    }

    fn key(n: u64) -> CacheKey {
        CacheKey {
            realpath: hex(b"/usr/bin/git"),
            size: n,
            mtime_ns: 7,
            exec_path: "-".to_owned(),
        }
    }

    #[test]
    fn cache_trusts_only_passing_entries() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("git-version");
        assert_eq!(cache_lookup(&p, &key(1)), None);
        cache_store(&p, &key(1), GitVersion::new(2, 43, 0));
        assert_eq!(cache_lookup(&p, &key(1)), Some(GitVersion::new(2, 43, 0)));
        // a different size (upgrade in place) misses
        assert_eq!(cache_lookup(&p, &key(2)), None);
        // a hand-written below-floor entry is never trusted
        let text = format!(
            "{CACHE_HEADER}\n{}\n",
            cache_line(&key(3), GitVersion::new(2, 30, 2))
        );
        std::fs::write(&p, text).unwrap();
        assert_eq!(cache_lookup(&p, &key(3)), None);
        // garbage and foreign headers are ignored
        std::fs::write(&p, "nonsense\n").unwrap();
        assert_eq!(cache_lookup(&p, &key(1)), None);
        std::fs::write(&p, format!("{CACHE_HEADER}\nbad\tline\n")).unwrap();
        assert_eq!(cache_lookup(&p, &key(1)), None);
        // replacing keeps one entry per binary
        cache_store(&p, &key(1), GitVersion::new(2, 43, 0));
        cache_store(&p, &key(4), GitVersion::new(2, 44, 0));
        assert_eq!(cache_read(&p).len(), 1);
        assert_eq!(cache_lookup(&p, &key(4)), Some(GitVersion::new(2, 44, 0)));
        // an unwritable location is silently skipped
        cache_store(
            &d.path().join("missing/dir/file"),
            &key(1),
            GitVersion::new(2, 43, 0),
        );
    }

    #[test]
    fn cache_line_round_trip() {
        let k = key(9);
        let line = cache_line(&k, GitVersion::new(2, 45, 1));
        assert_eq!(
            parse_cache_line(&line),
            Some((k, GitVersion::new(2, 45, 1)))
        );
        assert_eq!(parse_cache_line(&format!("{line}\textra")), None);
    }
}
