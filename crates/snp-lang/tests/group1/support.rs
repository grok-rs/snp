//! Fixtures of the group-1 tests: a temporary repository with a hook prefix, a [`TestHost`] recording through the
//! shim harness, a fake [`Fetch`], scripted replies with file effects, and the contexts of the `Language` calls.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use snp_core::cancel::CancelToken;
use snp_core::env::Env;
use snp_lang::harness::{Recorded, Reply, Substitutions, TestHost};
use snp_lang::{EnvCtx, Host, InstallCtx, PlanCtx};
use snp_net::{Checksum, Fetch, NetError};

/// The files of the hook repository (pre-commit's clone and snp's prefix hold the same).
pub const HOOK_FILES: [(&str, &str); 8] = [
    (
        ".pre-commit-hooks.yaml",
        "-   id: py-hook\n    name: py-hook\n    entry: py-hello --flag\n    language: python\n    files: ^a\\.txt$\n    require_serial: true\n\
         -   id: node-hook\n    name: node-hook\n    entry: node-hello\n    language: node\n    files: ^a\\.txt$\n    require_serial: true\n\
         -   id: go-hook\n    name: go-hook\n    entry: go-hello\n    language: golang\n    files: ^a\\.txt$\n    require_serial: true\n\
         -   id: rust-hook\n    name: rust-hook\n    entry: rust-hello\n    language: rust\n    files: ^a\\.txt$\n    require_serial: true\n\
         -   id: ruby-hook\n    name: ruby-hook\n    entry: ruby-hello\n    language: ruby\n    files: ^a\\.txt$\n    require_serial: true\n",
    ),
    (
        "setup.py",
        "from setuptools import setup\nsetup(name='hooks', version='0.0.0', py_modules=[])\n",
    ),
    (
        "package.json",
        "{\"name\": \"hooks\", \"version\": \"0.0.0\", \"bin\": {\"node-hello\": \"hello.js\"}}\n",
    ),
    ("go.mod", "module example.com/hooks\n\ngo 1.21\n"),
    ("main.go", "package main\n\nfunc main() {}\n"),
    (
        "Cargo.toml",
        "[package]\nname = \"hooks\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"rust-hello\"\npath = \"main.rs\"\n",
    ),
    ("main.rs", "fn main() {}\n"),
    (
        "hooks.gemspec",
        "Gem::Specification.new do |s|\n  s.name = 'hooks'\n  s.version = '0.0.0'\n  s.summary = 'x'\n  s.authors = ['x']\nend\n",
    ),
];

/// A temporary world: `<root>/repo` (the user's repository, the run's cwd), `<root>/prefix` (the hook clone),
/// `<root>/shims` (fake executables on `PATH`), `<root>/home`.
#[derive(Debug)]
pub struct World {
    /// Keeps the directory alive.
    _dir: tempfile::TempDir,
    /// The canonical root.
    pub root: PathBuf,
    /// The user's repository.
    pub repo: PathBuf,
    /// The hook prefix.
    pub prefix: PathBuf,
    /// Fake executables.
    pub shims: PathBuf,
    /// `$HOME`.
    pub home: PathBuf,
}

impl World {
    /// A new world with the hook files in the prefix (and a `.git` directory, as in a clone).
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = snp_pycompat::ospath::realpath(dir.path().as_os_str(), dir.path()).unwrap();
        let w = Self {
            repo: root.join("repo"),
            prefix: root.join("prefix"),
            shims: root.join("shims"),
            home: root.join("home"),
            root,
            _dir: dir,
        };
        for d in [&w.repo, &w.prefix, &w.shims, &w.home] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::create_dir_all(w.prefix.join(".git")).unwrap();
        for (name, body) in HOOK_FILES {
            std::fs::write(w.prefix.join(name), body).unwrap();
        }
        w
    }

    /// The `PATH` of both sides: the shims, then the system directories.
    pub fn path(&self) -> OsString {
        let mut p = self.shims.clone().into_os_string();
        p.push(if cfg!(windows) { ";" } else { ":" });
        p.push(if cfg!(windows) { r"C:\Windows\System32" } else { "/usr/bin:/bin" });
        p
    }

    /// The base environment (the run's, with `PRE_COMMIT=1`) plus `extra`.
    pub fn env(&self, extra: &[(&str, &str)]) -> Env {
        let mut pairs: Vec<(OsString, OsString)> = vec![
            ("PATH".into(), self.path()),
            ("HOME".into(), self.home.clone().into_os_string()),
            ("LANG".into(), "C.UTF-8".into()),
            ("PRE_COMMIT".into(), "1".into()),
        ];
        if cfg!(windows) {
            pairs.push(("USERPROFILE".into(), self.home.clone().into_os_string()));
        }
        for (k, v) in extra {
            pairs.push(((*k).into(), (*v).into()));
        }
        Env::from_pairs(pairs)
    }

    /// A host recording into a [`snp_lang::harness::ShimRunner`] whose delta base is `env`, cwd = the repository.
    pub fn host(&self, env: Env) -> TestHost {
        TestHost::new(&self.repo, env)
    }

    /// An empty executable `name` in the shims directory.
    pub fn shim(&self, name: &str) -> PathBuf {
        let p = self.shims.join(name);
        exe(&p);
        p
    }

    /// The path placeholders of this world.
    pub fn subst(&self) -> Substitutions {
        Substitutions::new()
            .with(self.repo.join(".snp-tools").as_os_str(), "{TOOLS}")
            .with(self.repo.as_os_str(), "{REPO}")
            .with(self.prefix.as_os_str(), "{PREFIX}")
            .with(self.shims.as_os_str(), "{SHIMS}")
            .with(self.home.as_os_str(), "{HOME}")
    }
}

/// An empty executable file (parents created).
pub fn exe(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// `deps` as the trait wants them.
pub fn deps(v: &[&str]) -> Vec<Arc<str>> {
    v.iter().map(|s| Arc::from(*s)).collect()
}

/// An environment context.
pub fn env_ctx<'a>(host: &'a Host<'a>, prefix: &'a Path, version: &'a str, env_dir: &'a Path) -> EnvCtx<'a> {
    EnvCtx {
        host,
        prefix,
        version,
        env_dir,
    }
}

/// An install context.
pub fn install_ctx<'a>(
    host: &'a Host<'a>,
    prefix: &'a Path,
    version: &'a str,
    env_dir: &'a Path,
    deps: &'a [Arc<str>],
    is_local: bool,
) -> InstallCtx<'a> {
    InstallCtx {
        cx: env_ctx(host, prefix, version, env_dir),
        deps,
        is_local,
    }
}

/// A plan context.
pub fn plan_ctx<'a>(
    host: &'a Host<'a>,
    prefix: &'a Path,
    version: &'a str,
    env_dir: &'a Path,
    env: &'a Env,
    entry: &'a str,
) -> PlanCtx<'a> {
    PlanCtx {
        host,
        prefix,
        version,
        env_dir: Some(env_dir),
        env,
        entry,
        args: &[],
        is_local: false,
        color: false,
        docker_tag: None,
    }
}

/// A file effect of a scripted call; templates expand `{cwd}`, `{argN}` and `{env:NAME}`.
#[derive(Debug, Clone, Copy)]
pub enum Eff {
    /// `mkdir -p`.
    Mkdir(&'static str),
    /// An empty executable file.
    Exe(&'static str),
    /// A text file.
    Write(&'static str, &'static str),
}

/// One scripted reply, shared by the snp side (a [`Reply`]) and the oracle side (`record.py`'s `replies`).
#[derive(Debug, Clone)]
pub struct R {
    /// The tool.
    pub tool: &'static str,
    /// Argument prefix.
    pub when: &'static [&'static str],
    /// Exit status.
    pub status: i64,
    /// stdout.
    pub stdout: &'static str,
    /// Effects.
    pub effects: Vec<Eff>,
}

/// A reply without effects.
pub fn r(tool: &'static str, when: &'static [&'static str], status: i64, stdout: &'static str) -> R {
    R {
        tool,
        when,
        status,
        stdout,
        effects: Vec::new(),
    }
}

/// Expands a template against a recorded call.
fn expand(t: &str, rec: &Recorded, base: &Env) -> PathBuf {
    let mut out = t.replace("{cwd}", &rec.cwd.to_string_lossy());
    for (i, a) in rec.argv.iter().enumerate().rev() {
        out = out.replace(&format!("{{arg{i}}}"), &a.to_string_lossy());
    }
    while let Some(start) = out.find("{env:") {
        let end = start + out[start..].find('}').unwrap();
        let name = &out[start + 5..end];
        let value = rec
            .env_delta
            .iter()
            .find(|(k, _)| k == OsStr::new(name))
            .and_then(|(_, v)| v.clone())
            .or_else(|| base.get(name).map(OsStr::to_os_string))
            .unwrap_or_default();
        out = format!("{}{}{}", &out[..start], value.to_string_lossy(), &out[end + 1..]);
    }
    PathBuf::from(out)
}

impl R {
    /// The snp-side reply (sticky).
    pub fn reply(&self, base: &Env) -> Reply {
        let effects = self.effects.clone();
        let base = base.clone();
        Reply {
            status: self.status,
            stdout: self.stdout.as_bytes().to_vec(),
            ..Reply::ok(Vec::new())
        }
        .when(self.when.iter().copied())
        .sticky()
        .with_effect(move |rec| {
            for e in &effects {
                match e {
                    Eff::Mkdir(t) => std::fs::create_dir_all(expand(t, rec, &base))?,
                    Eff::Exe(t) => {
                        let p = expand(t, rec, &base);
                        std::fs::create_dir_all(p.parent().unwrap())?;
                        std::fs::write(&p, b"")?;
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))?;
                        }
                    }
                    Eff::Write(t, body) => {
                        let p = expand(t, rec, &base);
                        std::fs::create_dir_all(p.parent().unwrap())?;
                        std::fs::write(&p, body)?;
                    }
                }
            }
            Ok(())
        })
    }

    /// The oracle-side reply (`record.py` JSON).
    pub fn json(&self) -> serde_json::Value {
        let effects: Vec<serde_json::Value> = self
            .effects
            .iter()
            .map(|e| match e {
                Eff::Mkdir(t) => serde_json::json!({"mkdir": t}),
                Eff::Exe(t) => serde_json::json!({"exe": t}),
                Eff::Write(t, b) => serde_json::json!({"write": [t, b]}),
            })
            .collect();
        serde_json::json!({
            "tool": self.tool,
            "when": self.when,
            "status": self.status,
            "stdout": self.stdout,
            "stderr": "",
            "effects": effects,
        })
    }
}

/// Scripts `replies` on the host's runner.
pub fn script(t: &TestHost, replies: &[R]) {
    for r in replies {
        t.runner.reply(r.tool, r.reply(&t.env));
    }
}

/// A [`Fetch`] serving fixed texts and files (verified like the real one).
#[derive(Debug, Default)]
pub struct FakeFetch {
    /// URL → text.
    pub texts: Vec<(String, String)>,
    /// URL → bytes.
    pub files: Vec<(String, Vec<u8>)>,
    /// Every URL asked for, in order.
    pub asked: Mutex<Vec<String>>,
}

impl Fetch for FakeFetch {
    fn get_text(&self, url: &str, _max: u64) -> Result<String, NetError> {
        self.asked.lock().unwrap().push(url.to_owned());
        self.texts
            .iter()
            .find(|(u, _)| u == url)
            .map(|(_, t)| t.clone())
            .ok_or_else(|| NetError::Http {
                url: url.to_owned(),
                status: 404,
            })
    }

    fn get_verified(
        &self,
        url: &str,
        sum: &Checksum,
        dest_dir: &Path,
        file_name: &str,
        _cancel: &CancelToken,
    ) -> Result<PathBuf, NetError> {
        self.asked.lock().unwrap().push(url.to_owned());
        let bytes = self
            .files
            .iter()
            .find(|(u, _)| u == url)
            .map(|(_, b)| b.clone())
            .or_else(|| snp_net::tools::embedded(url).map(<[u8]>::to_vec))
            .ok_or_else(|| NetError::Http {
                url: url.to_owned(),
                status: 404,
            })?;
        let Checksum::Sha256(want) = sum else {
            panic!("unexpected checksum")
        };
        let got: [u8; 32] = Sha256::digest(&bytes).into();
        if &got != want {
            return Err(NetError::Checksum {
                url: url.to_owned(),
                expected: sum.hex(),
                actual: String::new(),
            });
        }
        std::fs::create_dir_all(dest_dir)?;
        let p = dest_dir.join(file_name);
        std::fs::write(&p, bytes)?;
        Ok(p)
    }
}

/// sha256 hex of `b`.
pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// A `.tar.gz` holding `files` (`(path, body, mode)`, directories implied).
pub fn tar_gz(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut b = tar::Builder::new(enc);
    for (path, body, mode) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(*mode);
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, path, *body).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}
