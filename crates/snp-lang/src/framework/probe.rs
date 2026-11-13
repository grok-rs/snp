//! Default-version and executable-lookup memoisation (§6.1, ADR-070, ADR-079).
//!
//! Owner: WP-16.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use snp_core::env::Env;
use snp_core::lang::LanguageId;
use snp_core::profile::SNP_VERSION;
use snp_core::sync::lock;

use crate::{Fingerprint, LangError};

/// Default-version and exe-lookup memoisation for one process, plus persistent facts in `cache-v1/probes` only where
/// sound (§6.1, ADR-079): a fact is persisted only when the resolved executable is a native binary (not a `#!` script)
/// and not a known proxy (pyenv/asdf/rbenv/nodenv shims, mise and rustup proxies, macOS `/usr/bin/*` xcrun shims); its
/// key is the realpath + (len, `mtime_ns`, ino) of that binary plus a digest of the env that steers it (PATH,
/// `PYENV_VERSION`, `ASDF_*`, `MISE_*`, `RUSTUP_TOOLCHAIN`, `DEVELOPER_DIR`, `VIRTUAL_ENV`, `CONDA_PREFIX`). Everything
/// else is memoised per process.
///
/// pre-commit memoises every `get_default_version` with `functools.lru_cache(maxsize=1)` for the process lifetime
/// (`python.py:108`, `node.py:27`, `rust.py:35`, …) and never persists anything. The executable whose facts steer a
/// language's default is fixed per language: python's reference interpreter (`python3`, then `python`; Windows `py`,
/// then `python`) and rust's `cargo` (`rust.py:36-45`); the other languages' defaults spawn nothing (`exe_exists`,
/// constants) and are only memoised.
#[derive(Debug, Default)]
pub struct ToolProbe {
    /// `cache-v1/probes` (None: memoise per process only).
    persist_dir: Option<PathBuf>,
    /// `default_version` results: one slot per (language, steering env), locked while its probe runs.
    defaults: Mutex<Vec<DefaultSlot>>,
    /// `find_executable` results per (name, PATH, PATHEXT, cwd).
    exes: Mutex<HashMap<ExeKey, Option<PathBuf>>>,
    /// docker facts shared by `docker` and `docker_image` (`docker.py` caches `_is_rootless`).
    pub(crate) docker: DockerFacts,
}

/// The per-process docker facts (04a §14.14: rootless result, container mounts).
#[derive(Debug, Default)]
pub(crate) struct DockerFacts {
    /// `_is_rootless()`.
    pub(crate) rootless: OnceLock<bool>,
    /// The bind mounts of the container snp runs in (`None`: not in a container, or unknown).
    pub(crate) mounts: OnceLock<Option<Vec<(OsString, OsString)>>>,
    /// Test seam: the file read instead of `/proc/1/mountinfo`.
    pub(crate) mountinfo: Option<PathBuf>,
}

/// One memoised default version.
#[derive(Debug)]
struct DefaultSlot {
    /// The language.
    lang: LanguageId,
    /// Digest of the steering environment (and the cwd when PATH has relative entries).
    steer: [u8; 32],
    /// The value once probed.
    value: Arc<Mutex<Option<Arc<str>>>>,
}

/// Key of a memoised executable lookup.
type ExeKey = (OsString, Option<OsString>, Option<OsString>, PathBuf);

/// The variables that steer which interpreter a name resolves to and what it reports (ADR-079), besides every
/// `ASDF_*` and `MISE_*` variable.
const STEERING: [&str; 7] = [
    "PATH",
    "PATHEXT",
    "PYENV_VERSION",
    "RUSTUP_TOOLCHAIN",
    "DEVELOPER_DIR",
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
];

/// Version of the persisted fact format (part of every key).
const FACT_FORMAT: &str = "snp-probe-v1";
/// Cap on a fact file.
const MAX_FACT: u64 = 4096;

impl ToolProbe {
    /// A probe cache persisting native facts under `persist_dir` (None: memoise per process only).
    pub fn new(persist_dir: Option<PathBuf>) -> Self {
        Self {
            persist_dir,
            ..Self::default()
        }
    }

    /// Test seam (feature `testing`): read `path` instead of `/proc/1/mountinfo` when docker looks for the container
    /// it runs in (`docker.py` `_get_container_id`).
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn with_mountinfo(mut self, path: PathBuf) -> Self {
        self.docker.mountinfo = Some(path);
        self
    }

    /// Memoised `Language::default_version` result for `lang` under `env`; `probe` runs at most once per process.
    ///
    /// The memo is keyed by `lang` and a digest of the steering variables (and of `cwd` when `PATH` has relative or
    /// empty entries); concurrent callers with the same key wait for the one running probe. A failing probe is not
    /// memoised (pre-commit's `lru_cache` does not cache exceptions either). With a persistence directory, a fact for
    /// a native, non-proxy steering executable is read before probing and written after it.
    pub fn default_version(
        &self,
        lang: LanguageId,
        env: &Env,
        cwd: &Path,
        probe: &mut dyn FnMut() -> Result<Arc<str>, LangError>,
    ) -> Result<Arc<str>, LangError> {
        let steer = steering_digest(env, cwd);
        let slot = {
            let mut slots = lock(&self.defaults);
            if let Some(s) = slots.iter().find(|s| s.lang == lang && s.steer == steer) {
                Arc::clone(&s.value)
            } else {
                let value = Arc::new(Mutex::new(None));
                slots.push(DefaultSlot {
                    lang,
                    steer,
                    value: Arc::clone(&value),
                });
                value
            }
        };
        let mut guard = lock(&slot);
        if let Some(v) = guard.as_ref() {
            return Ok(Arc::clone(v));
        }
        let fact = self.fact_file(lang, env, cwd, &steer);
        if let Some(v) = fact.as_ref().and_then(Fact::read) {
            *guard = Some(Arc::clone(&v));
            return Ok(v);
        }
        let v = probe()?;
        if let Some(f) = &fact {
            f.write(&v);
        }
        *guard = Some(Arc::clone(&v));
        Ok(v)
    }

    /// `find_executable` memoised per (PATH, PATHEXT, name).
    ///
    /// `parse_shebang.find_executable` (`parse_shebang.py:22-43`) through `snp_proc::resolve::find_executable`; the
    /// memo key also holds `cwd`, because relative `PATH` entries resolve against it.
    pub fn find_exe(&self, name: &OsStr, env: &Env, cwd: &Path) -> Option<PathBuf> {
        let key: ExeKey = (
            name.to_os_string(),
            env.get("PATH").map(OsStr::to_os_string),
            env.get("PATHEXT").map(OsStr::to_os_string),
            cwd.to_path_buf(),
        );
        if let Some(hit) = lock(&self.exes).get(&key) {
            return hit.clone();
        }
        let found = snp_proc::resolve::find_executable(name, env, cwd);
        lock(&self.exes).insert(key, found.clone());
        found
    }

    /// (len, mtime, ino/index) of an executable for health fast paths; None when it cannot be stat'ed.
    ///
    /// Symlinks are followed (a venv's `python` links to the base interpreter, whose replacement must invalidate the
    /// env); `detail` is empty for the caller to fill.
    pub fn fingerprint(&self, exe: &Path) -> Option<Fingerprint> {
        let md = std::fs::metadata(exe).ok()?;
        Some(Fingerprint {
            exe: exe.to_path_buf(),
            len: md.len(),
            mtime_ns: mtime_ns(&md)?,
            ino_or_index: file_id(exe, &md)?,
            detail: Arc::from(""),
        })
    }

    /// The persisted fact for `lang`'s steering executable, when sound.
    fn fact_file(&self, lang: LanguageId, env: &Env, cwd: &Path, steer: &[u8; 32]) -> Option<Fact> {
        let dir = self.persist_dir.as_ref()?;
        let found = steering_exes(lang)
            .iter()
            .find_map(|n| self.find_exe(OsStr::new(n), env, cwd))?;
        let found = if found.is_absolute() {
            found
        } else {
            cwd.join(found)
        };
        let real = snp_pycompat::ospath::realpath(found.as_os_str(), cwd).ok()?;
        if is_proxy(&found, &real) || !is_native(&real) {
            return None;
        }
        let md = std::fs::metadata(&real).ok()?;
        let mut h = blake3::Hasher::new();
        h.update(FACT_FORMAT.as_bytes());
        h.update(b"\0");
        h.update(SNP_VERSION.as_bytes());
        h.update(b"\0");
        h.update(lang.as_str().as_bytes());
        h.update(b"\0");
        h.update(real.as_os_str().as_encoded_bytes());
        h.update(b"\0");
        h.update(&md.len().to_le_bytes());
        h.update(&mtime_ns(&md)?.to_le_bytes());
        h.update(&file_id(&real, &md)?.to_le_bytes());
        h.update(steer);
        Some(Fact {
            path: dir.join(format!("{}.json", h.finalize().to_hex())),
            lang,
            exe: real,
        })
    }
}

/// The executables whose identity steers `lang`'s default version (see [`ToolProbe`]).
const fn steering_exes(lang: LanguageId) -> &'static [&'static str] {
    match lang {
        LanguageId::Python if cfg!(windows) => &["py", "python"],
        LanguageId::Python => &["python3", "python"],
        LanguageId::Rust => &["cargo"],
        _ => &[],
    }
}

/// blake3 over the steering variables (sorted; unset ≠ empty) and, when `PATH` has relative or empty entries,
/// `cwd`.
fn steering_digest(env: &Env, cwd: &Path) -> [u8; 32] {
    let upper = |k: &OsStr| k.to_string_lossy().to_ascii_uppercase();
    let mut pairs: Vec<(String, Option<OsString>)> = STEERING
        .iter()
        .map(|k| ((*k).to_owned(), env.get(k).map(OsStr::to_os_string)))
        .collect();
    for (k, v) in env.iter() {
        let u = upper(k);
        if u.starts_with("ASDF_") || u.starts_with("MISE_") {
            pairs.push((u, Some(v.to_os_string())));
        }
    }
    pairs.sort();
    let mut h = blake3::Hasher::new();
    for (k, v) in &pairs {
        h.update(k.as_bytes());
        match v {
            Some(v) => {
                h.update(b"=");
                h.update(v.as_encoded_bytes());
            }
            None => {
                h.update(b"\x01");
            }
        }
        h.update(b"\0");
    }
    let relative_path = env.get("PATH").is_none_or(|p| {
        std::env::split_paths(p).any(|d| d.as_os_str().is_empty() || d.is_relative())
    });
    if relative_path {
        h.update(b"cwd=");
        h.update(cwd.as_os_str().as_encoded_bytes());
    }
    *h.finalize().as_bytes()
}

/// A known proxy: a `shims` directory (pyenv, rbenv, nodenv, asdf, mise, scoop), a version-manager binary (rustup's
/// proxies are links or copies of rustup; mise, volta, proto), a directory holding `rustup` (`~/.cargo/bin`),
/// Chocolatey's shim directory, or macOS's `/usr/bin` (xcrun shims steered by `DEVELOPER_DIR`).
fn is_proxy(found: &Path, real: &Path) -> bool {
    const MANAGERS: [&str; 11] = [
        "rustup",
        "rustup-init",
        "mise",
        "rtx",
        "volta",
        "volta-shim",
        "proto-shim",
        "asdf",
        "pyenv",
        "rbenv",
        "nodenv",
    ];
    let has_component = |p: &Path, name: &str| {
        p.components()
            .any(|c| matches!(c, Component::Normal(n) if n.eq_ignore_ascii_case(name)))
    };
    if [found, real]
        .iter()
        .any(|p| has_component(p, "shims") || has_component(p, "chocolatey"))
    {
        return true;
    }
    let stem = real
        .file_stem()
        .map_or_default(|s| s.to_string_lossy().to_ascii_lowercase());
    if MANAGERS.contains(&stem.as_str()) {
        return true;
    }
    let rustup_sibling = |p: &Path| {
        p.parent()
            .is_some_and(|d| d.join("rustup").exists() || d.join("rustup.exe").exists())
    };
    if rustup_sibling(found) || rustup_sibling(real) {
        return true;
    }
    cfg!(target_os = "macos") && real.starts_with("/usr/bin")
}

/// A native executable: not a `#!` script (Windows: `.exe`/`.com`).
fn is_native(real: &Path) -> bool {
    if cfg!(windows) {
        return real
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("com"));
    }
    let mut head = [0_u8; 2];
    File::open(real)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| head != *b"#!")
}

/// mtime in nanoseconds since the epoch (negative before it).
fn mtime_ns(md: &std::fs::Metadata) -> Option<i128> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        i128::from(md.mtime())
            .checked_mul(1_000_000_000)?
            .checked_add(i128::from(md.mtime_nsec()))
    }
    #[cfg(not(unix))]
    {
        let t = md.modified().ok()?;
        match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => i128::try_from(d.as_nanos()).ok(),
            Err(e) => i128::try_from(e.duration().as_nanos()).ok()?.checked_neg(),
        }
    }
}

/// inode (Unix) / file index (Windows).
#[cfg_attr(
    unix,
    expect(clippy::unnecessary_wraps, reason = "fallible on Windows only")
)]
fn file_id(path: &Path, md: &std::fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _: &Path = path;
        Some(md.ino())
    }
    #[cfg(windows)]
    {
        let _: &std::fs::Metadata = md;
        let f = File::open(path).ok()?;
        winapi_util::file::information(&f)
            .ok()
            .map(|i| i.file_index())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _: (&Path, &std::fs::Metadata) = (path, md);
        Some(0)
    }
}

/// A persisted default-version fact.
#[derive(Debug)]
struct Fact {
    /// `cache-v1/probes/<blake3>.json`.
    path: PathBuf,
    /// The language (re-checked on read).
    lang: LanguageId,
    /// The realpath of the steering executable (re-checked on read).
    exe: PathBuf,
}

impl Fact {
    /// The value, when the file exists and is well-formed for this language and executable.
    fn read(&self) -> Option<Arc<str>> {
        let mut bytes = Vec::new();
        File::open(&self.path)
            .and_then(|f| f.take(MAX_FACT).read_to_end(&mut bytes))
            .ok()?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let ok = v.get("v").and_then(serde_json::Value::as_u64) == Some(1)
            && v.get("lang").and_then(serde_json::Value::as_str) == Some(self.lang.as_str())
            && v.get("exe").and_then(serde_json::Value::as_str)
                == Some(self.exe.to_string_lossy().as_ref());
        let value = v.get("value").and_then(serde_json::Value::as_str)?;
        (ok && !value.is_empty() && value.len() <= 256).then(|| Arc::from(value))
    }

    /// Best effort: a fact that cannot be written is simply probed again next time.
    fn write(&self, value: &str) {
        let body = serde_json::json!({
            "v": 1,
            "lang": self.lang.as_str(),
            "exe": self.exe.to_string_lossy(),
            "value": value,
        });
        let Ok(bytes) = serde_json::to_vec(&body) else {
            return;
        };
        if let Some(dir) = self.path.parent()
            && std::fs::create_dir_all(dir).is_ok()
        {
            let _ignored = snp_os::fs::atomic_replace(&self.path, &bytes, Some(0o600));
        }
    }
}
