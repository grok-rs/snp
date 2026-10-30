//! pip's configuration (`pip.conf` files and `PIP_*` variables) translated to `uv pip install` (04a §11.3 G2–G4,
//! §11.5; ADR-033).
//!
//! uv reads neither `pip.conf` nor `PIP_*`, so a corporate index configured for pip would silently be bypassed. snp
//! collects pip's effective install configuration exactly as pip layers it (site files, user files,
//! `$PIP_CONFIG_FILE`, then the environment; `[global]` then `[install]` within a file) and translates every
//! setting it understands into `uv pip install` flags or variables. A setting that changes resolution or installation
//! and has no translation — and every `additional_dependencies` item that is a pip option uv does not accept — is a
//! **fallback trigger**: the environment is built with pip instead (`SNP_PYTHON_INSTALLER=auto`), or the install
//! fails (`SNP_PYTHON_INSTALLER=uv`).
//!
//! Owner: WP-24.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use snp_core::env::Env;

use super::shared::{ini_section, read_ini};

/// The pip configuration in effect: `(option name, value, origin)` with option names in pip's long-option form
/// (`index-url`), later sources already applied over earlier ones.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PipConfig {
    /// `(name, value, origin)`; origin is a file path or `PIP_<NAME>`.
    pub(crate) options: Vec<(String, String, String)>,
}

impl PipConfig {
    /// Sets `name` (replacing an earlier source).
    fn set(&mut self, name: &str, value: &str, origin: &str) {
        self.options.retain(|(n, _, _)| n != name);
        self.options
            .push((name.to_owned(), value.to_owned(), origin.to_owned()));
    }
}

/// `option-name` from a file key (`index_url`, `Index-URL` → `index-url`).
fn canonical(key: &str) -> String {
    key.trim().to_lowercase().replace('_', "-")
}

/// pip's configuration files in load order (pip `configuration.py` `get_configuration_files`): the site files
/// (`$XDG_CONFIG_DIRS/pip/pip.conf` + `/etc/pip.conf`; Windows `C:\ProgramData\pip\pip.ini`), the user files (legacy
/// `~/.pip/pip.conf`, then `$XDG_CONFIG_HOME/pip/pip.conf` or `~/.config/pip/pip.conf`; macOS also
/// `~/Library/Application Support/pip/pip.conf`; Windows `%APPDATA%\pip\pip.ini`), then `$PIP_CONFIG_FILE`.
/// `PIP_CONFIG_FILE=os.devnull` disables every file.
pub(crate) fn config_files(env: &Env) -> Vec<PathBuf> {
    let devnull = if cfg!(windows) { "nul" } else { "/dev/null" };
    let explicit = env.get("PIP_CONFIG_FILE").map(PathBuf::from);
    if explicit
        .as_deref()
        .is_some_and(|p| p.as_os_str().eq_ignore_ascii_case(devnull))
    {
        return Vec::new();
    }
    let home = crate::common::home_dir(env);
    let home = PathBuf::from(home);
    let mut files: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        files.push(PathBuf::from(r"C:\ProgramData\pip\pip.ini"));
        if let Some(appdata) = env.get_nonempty("APPDATA") {
            files.push(Path::new(appdata).join("pip").join("pip.ini"));
        }
    } else {
        let xdg_dirs = env
            .get_nonempty("XDG_CONFIG_DIRS")
            .map_or_else(|| OsString::from("/etc/xdg"), std::ffi::OsStr::to_os_string);
        for dir in std::env::split_paths(&xdg_dirs) {
            files.push(dir.join("pip").join("pip.conf"));
        }
        files.push(PathBuf::from("/etc/pip.conf"));
        files.push(home.join(".pip").join("pip.conf"));
        if cfg!(target_os = "macos") {
            files.push(
                home.join("Library")
                    .join("Application Support")
                    .join("pip")
                    .join("pip.conf"),
            );
        }
        let xdg_home = env
            .get_nonempty("XDG_CONFIG_HOME")
            .map_or_else(|| home.join(".config"), PathBuf::from);
        files.push(xdg_home.join("pip").join("pip.conf"));
    }
    if let Some(p) = explicit {
        files.push(p);
    }
    files
}

/// pip's effective `install` configuration under `env`: each file's `[global]` then `[install]` options, files in
/// load order, then the `PIP_<NAME>` variables (pip's `_update_defaults`: environment over files). `PIP_CONFIG_FILE`
/// itself is not an option.
pub(crate) fn collect(env: &Env) -> PipConfig {
    let mut cfg = PipConfig::default();
    for file in config_files(env) {
        let Some(ini) = read_ini(&file) else {
            continue;
        };
        let origin = file.display().to_string();
        for section in ["global", "install"] {
            if let Some(opts) = ini_section(&ini, section) {
                for (k, v) in opts {
                    cfg.set(&canonical(k), v, &origin);
                }
            }
        }
    }
    for (k, v) in env.iter() {
        let Some(k) = k.to_str() else {
            continue;
        };
        let upper = k.to_ascii_uppercase();
        let Some(name) = upper.strip_prefix("PIP_") else {
            continue;
        };
        if name.is_empty() || name == "CONFIG_FILE" {
            continue;
        }
        cfg.set(
            &canonical(name),
            &v.to_string_lossy(),
            &format!("PIP_{name}"),
        );
    }
    cfg
}

/// pip's `strtobool`: `y yes t true on 1` → true, `n no f false off 0` → false (ASCII-case-insensitive), else an
/// error.
fn strtobool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" | "t" | "true" | "on" | "1" => Some(true),
        "n" | "no" | "f" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// What `uv pip install` gets for a pip configuration.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Translation {
    /// Flags inserted before the packages.
    pub(crate) flags: Vec<OsString>,
    /// Variables set on the install command.
    pub(crate) env: Vec<(String, OsString)>,
}

/// Options that never change what gets resolved or installed (output, caching, prompts): accepted without a
/// translation.
const NEUTRAL: &[&str] = &[
    "disable-pip-version-check",
    "no-cache-dir",
    "cache-dir",
    "progress-bar",
    "quiet",
    "verbose",
    "no-color",
    "no-input",
    "root-user-action",
    "retries",
    "log",
    "log-file",
    "no-python-version-warning",
    "no-warn-script-location",
    "no-warn-conflicts",
    "exists-action",
    "isolated",
    "require-virtualenv",
    "break-system-packages",
];

/// Translates `cfg` (04a §11.5); `Err(origin)` names the first setting without a translation (a fallback trigger).
///
/// | pip | uv |
/// |---|---|
/// | `index-url` | `--default-index` |
/// | `extra-index-url` (whitespace-separated) | `--index` each, and `--index-strategy unsafe-best-match` (pip merges candidates across indexes, G3) |
/// | `find-links` | `--find-links` each |
/// | `no-index` (true) | `--no-index` |
/// | `trusted-host` | `--allow-insecure-host` each |
/// | `cert` / `client-cert` | `SSL_CERT_FILE` / `SSL_CLIENT_CERT` |
/// | `proxy` | `HTTPS_PROXY` and `HTTP_PROXY` unless set |
/// | `pre` (true) | `--prerelease allow` |
/// | `constraint` | `--constraint` each |
/// | `no-build-isolation` (pip's inverted boolean: a false value disables isolation) | `--no-build-isolation` |
/// | `only-binary` / `no-binary` | `--only-binary` / `--no-binary` per comma-separated item |
/// | `timeout` / `default-timeout` | `UV_HTTP_TIMEOUT` |
/// | `keyring-provider` | `--keyring-provider` |
/// | `require-hashes` (true) | `--require-hashes` |
///
/// The output and cache options of [`NEUTRAL`] need nothing; everything else (`use-feature`, `ignore-installed`,
/// `target`, `prefix`, `user`, `src`, `upgrade-strategy`, …) falls back to pip.
pub(crate) fn translate(cfg: &PipConfig, env: &Env) -> Result<Translation, String> {
    let mut t = Translation::default();
    let flag = |t: &mut Translation, f: &str, v: &str| {
        t.flags.push(OsString::from(f));
        t.flags.push(OsString::from(v));
    };
    let words = |v: &str| -> Vec<String> { v.split_whitespace().map(str::to_owned).collect() };
    let mut extra_index = false;
    for (name, value, origin) in &cfg.options {
        let fail = || format!("{origin}: {name}");
        let truthy = || strtobool(value).ok_or_else(fail);
        match name.as_str() {
            "index-url" | "i" => flag(&mut t, "--default-index", value.trim()),
            "extra-index-url" => {
                for u in words(value) {
                    flag(&mut t, "--index", &u);
                    extra_index = true;
                }
            }
            "find-links" | "f" => {
                for u in words(value) {
                    flag(&mut t, "--find-links", &u);
                }
            }
            "no-index" => {
                if truthy()? {
                    t.flags.push(OsString::from("--no-index"));
                }
            }
            "trusted-host" => {
                for h in words(value) {
                    flag(&mut t, "--allow-insecure-host", &h);
                }
            }
            "cert" => t
                .env
                .push(("SSL_CERT_FILE".to_owned(), OsString::from(value.trim()))),
            "client-cert" => t
                .env
                .push(("SSL_CLIENT_CERT".to_owned(), OsString::from(value.trim()))),
            "proxy" => {
                for var in ["HTTPS_PROXY", "HTTP_PROXY"] {
                    let lower = var.to_ascii_lowercase();
                    if env.get_nonempty(var).is_none() && env.get_nonempty(&lower).is_none() {
                        t.env.push((var.to_owned(), OsString::from(value.trim())));
                    }
                }
            }
            "pre" => {
                if truthy()? {
                    flag(&mut t, "--prerelease", "allow");
                }
            }
            "constraint" | "c" => {
                for c in words(value) {
                    flag(&mut t, "--constraint", &c);
                }
            }
            // pip's `store_false` options take `strtobool(value)` as the destination's value: a FALSE value
            // disables build isolation.
            "no-build-isolation" => {
                if !truthy()? {
                    t.flags.push(OsString::from("--no-build-isolation"));
                }
            }
            "only-binary" | "no-binary" => {
                for item in value
                    .split([',', ' ', '\t', '\n'])
                    .filter(|s| !s.is_empty())
                {
                    flag(&mut t, &format!("--{name}"), item);
                }
            }
            "timeout" | "default-timeout" => t
                .env
                .push(("UV_HTTP_TIMEOUT".to_owned(), OsString::from(value.trim()))),
            "keyring-provider" => flag(&mut t, "--keyring-provider", value.trim()),
            "require-hashes" => {
                if truthy()? {
                    t.flags.push(OsString::from("--require-hashes"));
                }
            }
            n if NEUTRAL.contains(&n) => {}
            _ => return Err(fail()),
        }
    }
    if extra_index {
        flag(&mut t, "--index-strategy", "unsafe-best-match");
    }
    Ok(t)
}

/// The `-`-prefixed `additional_dependencies` items uv accepts with pip's meaning (G4, verified with uv 0.8): exact
/// flags, and options that take their value in the same item (`--index-url=…`) or in the next item (`-r`, `-e`,
/// `-c`, …).
const DASH_FLAGS: [&str; 7] = [
    "--pre",
    "--no-deps",
    "--no-build-isolation",
    "--no-cache-dir",
    "--isolated",
    "-q",
    "--quiet",
];
/// Options with a value (`--x=v` or `--x v`).
const DASH_VALUED: [&str; 14] = [
    "-r",
    "--requirement",
    "-e",
    "--editable",
    "-c",
    "--constraint",
    "-i",
    "--index-url",
    "--extra-index-url",
    "--trusted-host",
    "--only-binary",
    "--no-binary",
    "-f",
    "--find-links",
];

/// The first `additional_dependencies` item that is a pip option uv does not accept (a fallback trigger), if any.
pub(crate) fn unsupported_dep(deps: &[std::sync::Arc<str>]) -> Option<String> {
    for d in deps {
        if !d.starts_with('-') {
            continue;
        }
        let name = d.split_once('=').map_or(&**d, |(n, _)| n);
        let ok = DASH_FLAGS.contains(&&**d) || DASH_VALUED.contains(&name);
        if !ok {
            return Some(d.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> PipConfig {
        let mut c = PipConfig::default();
        for (k, v) in pairs {
            c.set(k, v, "test");
        }
        c
    }

    fn flags(t: &Translation) -> Vec<String> {
        t.flags
            .iter()
            .map(|f| f.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn translation_table() {
        let env = Env::from_pairs([("HTTP_PROXY", "http://set")]);
        let t = translate(
            &cfg(&[
                ("index-url", "https://idx/simple"),
                ("extra-index-url", "https://a/simple\nhttps://b/simple"),
                ("find-links", "/wheels"),
                ("no-index", "false"),
                ("trusted-host", "a b"),
                ("cert", "/ca.pem"),
                ("client-cert", "/client.pem"),
                ("proxy", "http://proxy:3128"),
                ("pre", "yes"),
                ("constraint", "/c.txt"),
                ("no-build-isolation", "0"),
                ("only-binary", ":all:"),
                ("no-binary", "a,b"),
                ("timeout", "30"),
                ("keyring-provider", "subprocess"),
                ("require-hashes", "on"),
                ("disable-pip-version-check", "1"),
            ]),
            &env,
        )
        .unwrap();
        assert_eq!(
            flags(&t),
            [
                "--default-index",
                "https://idx/simple",
                "--index",
                "https://a/simple",
                "--index",
                "https://b/simple",
                "--find-links",
                "/wheels",
                "--allow-insecure-host",
                "a",
                "--allow-insecure-host",
                "b",
                "--prerelease",
                "allow",
                "--constraint",
                "/c.txt",
                "--no-build-isolation",
                "--only-binary",
                ":all:",
                "--no-binary",
                "a",
                "--no-binary",
                "b",
                "--keyring-provider",
                "subprocess",
                "--require-hashes",
                "--index-strategy",
                "unsafe-best-match",
            ]
        );
        assert_eq!(
            t.env,
            vec![
                ("SSL_CERT_FILE".to_owned(), OsString::from("/ca.pem")),
                ("SSL_CLIENT_CERT".to_owned(), OsString::from("/client.pem")),
                (
                    "HTTPS_PROXY".to_owned(),
                    OsString::from("http://proxy:3128")
                ),
                ("UV_HTTP_TIMEOUT".to_owned(), OsString::from("30")),
            ]
        );
    }

    #[test]
    fn inverted_build_isolation_and_bad_booleans() {
        let t = translate(
            &cfg(&[("no-build-isolation", "1")]),
            &Env::from_pairs::<&str, &str>([]),
        )
        .unwrap();
        assert_eq!(t.flags.len(), 0);
        assert!(
            translate(
                &cfg(&[("pre", "maybe")]),
                &Env::from_pairs::<&str, &str>([])
            )
            .is_err()
        );
    }

    #[test]
    fn untranslatable_settings_trigger_the_fallback() {
        for k in [
            "use-feature",
            "ignore-installed",
            "target",
            "prefix",
            "user",
            "upgrade-strategy",
        ] {
            let e = translate(&cfg(&[(k, "x")]), &Env::from_pairs::<&str, &str>([])).unwrap_err();
            assert_eq!(e, format!("test: {k}"));
        }
    }

    #[test]
    fn layering_env_over_files() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("pip.conf");
        std::fs::write(
            &f,
            "[global]\nindex_url = https://file/simple\ntimeout = 5\n[install]\ntimeout = 9\n[list]\nformat = json\n",
        )
        .unwrap();
        let env = Env::from_pairs([
            ("PIP_CONFIG_FILE", f.as_os_str()),
            ("HOME", d.path().as_os_str()),
            ("XDG_CONFIG_DIRS", d.path().join("none").as_os_str()),
            ("PIP_INDEX_URL", std::ffi::OsStr::new("https://env/simple")),
            ("PIP_DISABLE_PIP_VERSION_CHECK", std::ffi::OsStr::new("1")),
        ]);
        let c = collect(&env);
        let get = |n: &str| {
            c.options
                .iter()
                .find(|(k, _, _)| k == n)
                .map(|(_, v, _)| v.clone())
        };
        assert_eq!(get("index-url").as_deref(), Some("https://env/simple"));
        assert_eq!(get("timeout").as_deref(), Some("9"));
        assert_eq!(get("format"), None);
        assert_eq!(get("disable-pip-version-check").as_deref(), Some("1"));
        let devnull = Env::from_pairs([("PIP_CONFIG_FILE", "/dev/null")]);
        if cfg!(unix) {
            assert_eq!(config_files(&devnull).len(), 0);
        }
    }

    #[test]
    fn dash_dependencies() {
        let deps = |v: &[&str]| -> Vec<Arc<str>> { v.iter().map(|s| Arc::from(*s)).collect() };
        assert_eq!(
            unsupported_dep(&deps(&["six", "--pre", "-r", "req.txt"])),
            None
        );
        assert_eq!(
            unsupported_dep(&deps(&["--index-url=https://x", "-e", "."])),
            None
        );
        assert_eq!(
            unsupported_dep(&deps(&["six", "--user"])).as_deref(),
            Some("--user")
        );
        assert_eq!(
            unsupported_dep(&deps(&["--progress-bar=off"])).as_deref(),
            Some("--progress-bar=off")
        );
        assert_eq!(
            unsupported_dep(&deps(&["--use-pep517"])).as_deref(),
            Some("--use-pep517")
        );
    }
}
