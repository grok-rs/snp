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

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use snp_core::env::Env;

use super::shared::read_ini;

/// The pip configuration in effect: `(option name, value, origin)` with option names in pip's long-option form
/// (`index-url`), later sources already applied over earlier ones.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PipConfig {
    /// `(name, value, origin)`; origin is a file path or `PIP_<NAME>`.
    pub(crate) options: Vec<(String, String, String)>,
    /// name → position in `options` (lookups only; the vector keeps the order).
    index: HashMap<String, usize>,
}

impl PipConfig {
    /// Sets `name`, replacing an earlier source's value at its first position (pip assigns option defaults in
    /// order, so a later value wins).
    fn set(&mut self, name: &str, value: &str, origin: &str) {
        if let Some(slot) = self.index.get(name).and_then(|&i| self.options.get_mut(i)) {
            value.clone_into(&mut slot.1);
            origin.clone_into(&mut slot.2);
            return;
        }
        self.index.insert(name.to_owned(), self.options.len());
        self.options
            .push((name.to_owned(), value.to_owned(), origin.to_owned()));
    }
}

/// pip's `_normalize_name`: lower-cased, `_` → `-`, one leading `--` removed (`--Index_URL` → `index-url`).
fn normalize_name(key: &str) -> String {
    let n = key.to_lowercase().replace('_', "-");
    n.strip_prefix("--")
        .map_or_else(|| n.clone(), str::to_owned)
}

/// `os.devnull`.
const DEVNULL: &str = if cfg!(windows) { "nul" } else { "/dev/null" };

/// An environment value that `platformdirs` treats as set (`os.environ.get(name, "").strip()` non-empty).
fn env_dir(env: &Env, name: &str) -> Option<OsString> {
    env.get(name)
        .filter(|v| !snp_pycompat::text::strip(&v.to_string_lossy()).is_empty())
        .map(std::ffi::OsStr::to_os_string)
}

/// pip's configuration files in override order (pip 26 `configuration.py` `iter_config_files` /
/// `get_configuration_files`): the global files (`$XDG_CONFIG_DIRS/pip/pip.conf`, default `/etc/xdg`, then
/// `/etc/pip.conf`; macOS `/Library/Application Support/pip/pip.conf`; Windows `C:\ProgramData\pip\pip.ini`), the
/// user files (legacy `~/.pip/pip.conf` — Windows `~\pip\pip.ini` —, then `$XDG_CONFIG_HOME/pip/pip.conf` or
/// `~/.config/pip/pip.conf`; macOS `~/Library/Application Support/pip/pip.conf` when that directory exists, else
/// `~/.config/pip/pip.conf`; Windows `%APPDATA%\pip\pip.ini`) — skipped when `$PIP_CONFIG_FILE` names an existing
/// file —, then `$PIP_CONFIG_FILE`. The site file (`sys.prefix/pip.conf`) belongs to the environment being built and
/// does not exist yet. `PIP_CONFIG_FILE=os.devnull` (exactly) disables every file: `None`. A path listed twice
/// keeps its first place (pip keys the loaded files by name).
pub(crate) fn config_files(env: &Env) -> Option<Vec<PathBuf>> {
    let explicit = env.get("PIP_CONFIG_FILE").map(PathBuf::from);
    if explicit
        .as_deref()
        .is_some_and(|p| p.as_os_str() == std::ffi::OsStr::new(DEVNULL))
    {
        return None;
    }
    let home = PathBuf::from(crate::common::home_dir(env));
    let basename = if cfg!(windows) { "pip.ini" } else { "pip.conf" };
    let mut files: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        files.push(PathBuf::from(r"C:\ProgramData\pip").join(basename));
    } else if cfg!(target_os = "macos") {
        files.push(PathBuf::from("/Library/Application Support/pip").join(basename));
    } else {
        let xdg_dirs =
            env_dir(env, "XDG_CONFIG_DIRS").unwrap_or_else(|| OsString::from("/etc/xdg"));
        for dir in std::env::split_paths(&xdg_dirs) {
            let dir = super::interp::expanduser(&dir.to_string_lossy(), env);
            files.push(dir.join("pip").join(basename));
        }
        files.push(PathBuf::from("/etc").join(basename));
    }
    let load_user = !explicit
        .as_deref()
        .is_some_and(|p| !p.as_os_str().is_empty() && p.exists());
    if load_user {
        files.push(
            home.join(if cfg!(windows) { "pip" } else { ".pip" })
                .join(basename),
        );
        let new = if cfg!(windows) {
            env.get_nonempty("APPDATA")
                .map(|a| Path::new(a).join("pip"))
        } else if cfg!(target_os = "macos") {
            let data = home.join("Library").join("Application Support").join("pip");
            Some(if data.is_dir() {
                data
            } else {
                home.join(".config").join("pip")
            })
        } else {
            Some(
                env_dir(env, "XDG_CONFIG_HOME")
                    .map_or_else(|| home.join(".config"), PathBuf::from)
                    .join("pip"),
            )
        };
        if let Some(dir) = new {
            files.push(dir.join(basename));
        }
    }
    if let Some(p) = explicit {
        files.push(p);
    }
    let mut seen: HashSet<PathBuf> = HashSet::new();
    files.retain(|f| seen.insert(f.clone()));
    Some(files)
}

/// The command whose section applies besides `[global]`.
const COMMAND: &str = "install";

/// `PIP_*` names pip does not read (`ENV_NAMES_IGNORED`), plus `PIP_CONFIG_FILE` itself.
const ENV_IGNORED: [&str; 3] = ["version", "help", "config-file"];

/// pip's effective `install` configuration under `env`, as `ConfigOptionParser._update_defaults` applies it: every
/// file is loaded first (later files override a key, which keeps its first position; keys normalised by
/// [`normalize_name`]), then the merged `[global]` items, then the merged `[install]` items, then the `PIP_<NAME>`
/// variables are applied in that order (so a site file's `[install]` beats a user file's `[global]`), empty values
/// skipped. Only variables whose name starts with exactly `PIP_` count (Windows: names are upper-cased first, like
/// `os.environ`), except `PIP_VERSION`, `PIP_HELP` and `PIP_CONFIG_FILE`.
pub(crate) fn collect(env: &Env) -> PipConfig {
    // `section_items_dict` of `_get_ordered_configuration_items`: `global` then `install`.
    let mut sections: [PipConfig; 2] = [PipConfig::default(), PipConfig::default()];
    for file in config_files(env).unwrap_or_default() {
        let Some(ini) = read_ini(&file) else {
            continue;
        };
        let origin = file.display().to_string();
        // Only `global`/`install` (and `global.*`/`install.*`, see below) survive the split.
        let relevant = |s: &&str| {
            matches!(
                s.split_once('.').map_or(*s, |(head, _)| head),
                "global" | COMMAND
            )
        };
        for section in ini.sections().filter(relevant) {
            // `section + "." + _normalize_name(name)`, later split at the first `.`: a section `global.x` with
            // option `y` lands in `global` as key `x.y`.
            let Some(items) = ini.items(section) else {
                continue;
            };
            for (k, v) in items {
                let full = format!("{section}.{}", normalize_name(k));
                let Some((sect, key)) = full.split_once('.') else {
                    continue;
                };
                let slot = match sect {
                    "global" => 0,
                    COMMAND => 1,
                    _ => continue,
                };
                if let Some(cfg) = sections.get_mut(slot) {
                    cfg.set(key, v, &origin);
                }
            }
        }
    }
    let mut cfg = PipConfig::default();
    for section in &sections {
        for (k, v, origin) in &section.options {
            if !v.is_empty() {
                cfg.set(k, v, origin);
            }
        }
    }
    let mut vars = PipConfig::default();
    for (k, v) in env.iter() {
        let Some(k) = k.to_str() else {
            continue;
        };
        let k = if cfg!(windows) {
            k.to_ascii_uppercase()
        } else {
            k.to_owned()
        };
        let Some(name) = k.strip_prefix("PIP_") else {
            continue;
        };
        let lower = name.to_lowercase();
        let key = normalize_name(&lower);
        if ENV_IGNORED.contains(&lower.as_str()) || ENV_IGNORED.contains(&key.as_str()) {
            continue;
        }
        vars.set(&key, &v.to_string_lossy(), &format!("PIP_{name}"));
    }
    for (k, v, origin) in &vars.options {
        if !v.is_empty() && !k.is_empty() {
            cfg.set(k, v, origin);
        }
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
/// | `timeout` / `default-timeout` (a float) | `UV_HTTP_TIMEOUT`, rounded up to whole seconds |
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
            "index-url" => flag(&mut t, "--default-index", value.trim()),
            "extra-index-url" => {
                for u in words(value) {
                    flag(&mut t, "--index", &u);
                    extra_index = true;
                }
            }
            "find-links" => {
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
            "cert" => set_env(&mut t, "SSL_CERT_FILE", value.trim()),
            "client-cert" => set_env(&mut t, "SSL_CLIENT_CERT", value.trim()),
            "proxy" => {
                for var in ["HTTPS_PROXY", "HTTP_PROXY"] {
                    let lower = var.to_ascii_lowercase();
                    if env.get_nonempty(var).is_none() && env.get_nonempty(&lower).is_none() {
                        set_env(&mut t, var, value.trim());
                    }
                }
            }
            "pre" => {
                if truthy()? {
                    flag(&mut t, "--prerelease", "allow");
                }
            }
            "constraint" => {
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
            // pip's timeout is a float (`60.0`); uv's `UV_HTTP_TIMEOUT` is whole seconds: rounded up.
            "timeout" | "default-timeout" => {
                let secs = value
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|s| s.is_finite() && *s >= 0.0 && *s < 1e15)
                    .ok_or_else(fail)?;
                set_env(&mut t, "UV_HTTP_TIMEOUT", format!("{}", secs.ceil()));
            }
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

/// Sets a variable of the translation (a later option with the same destination wins).
fn set_env(t: &mut Translation, key: &str, value: impl Into<OsString>) {
    let value = value.into();
    if let Some(slot) = t.env.iter_mut().find(|(k, _)| k == key) {
        slot.1 = value;
    } else {
        t.env.push((key.to_owned(), value));
    }
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
            assert_eq!(config_files(&devnull), None);
        }
    }

    /// A home with a user file, a site directory with a global file, and `extra` variables (the layering tests
    /// below are XDG-only: not Windows or macOS, whose site directories differ).
    #[cfg(not(any(windows, target_os = "macos")))]
    fn layered(user: &str, site: &str, extra: &[(&str, &str)]) -> (tempfile::TempDir, PipConfig) {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        std::fs::create_dir_all(home.join(".config/pip")).unwrap();
        std::fs::create_dir_all(d.path().join("xdg/pip")).unwrap();
        std::fs::write(home.join(".config/pip/pip.conf"), user).unwrap();
        std::fs::write(d.path().join("xdg/pip/pip.conf"), site).unwrap();
        let mut pairs: Vec<(String, String)> = vec![
            ("HOME".into(), home.display().to_string()),
            (
                "XDG_CONFIG_DIRS".into(),
                d.path().join("xdg").display().to_string(),
            ),
        ];
        for (k, v) in extra {
            let v = v.replace("{D}", &d.path().display().to_string());
            pairs.push(((*k).to_owned(), v));
        }
        let c = collect(&Env::from_pairs(pairs));
        (d, c)
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    fn value(c: &PipConfig, n: &str) -> Option<String> {
        c.options
            .iter()
            .find(|(k, _, _)| k == n)
            .map(|(_, v, _)| v.clone())
    }

    /// The host's own global file would take part in pip's layering too.
    #[cfg(not(any(windows, target_os = "macos")))]
    fn host_has_global_conf() -> bool {
        Path::new("/etc/pip.conf").exists()
    }

    /// pip merges every file first and then applies `[global]`, `[install]`, the environment: a site file's
    /// `[install]` beats a user file's `[global]` (pip 26 `_get_ordered_configuration_items`, checked against pip's
    /// `InstallCommand.parse_args`).
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn site_install_section_beats_user_global() {
        if host_has_global_conf() {
            return;
        }
        let (_d, c) = layered(
            "[global]\nindex-url = https://user-global/simple\n",
            "[install]\nindex-url = https://site-install/simple\n",
            &[],
        );
        assert_eq!(
            value(&c, "index-url").as_deref(),
            Some("https://site-install/simple")
        );
        // Within one section, the user file overrides the site file.
        let (_d, c) = layered(
            "[global]\ntimeout = 9\n",
            "[global]\ntimeout = 5\nretries = 2\n",
            &[],
        );
        assert_eq!(value(&c, "timeout").as_deref(), Some("9"));
        assert_eq!(value(&c, "retries").as_deref(), Some("2"));
    }

    /// (b) An existing `$PIP_CONFIG_FILE` skips the user files; a missing one does not.
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn existing_pip_config_file_skips_user_files() {
        if host_has_global_conf() {
            return;
        }
        std::fs::create_dir_all(std::env::temp_dir()).unwrap();
        let (_d, c) = layered(
            "[global]\nindex-url = https://user/simple\n",
            "",
            &[("PIP_CONFIG_FILE", "{D}/xdg/pip/pip.conf")],
        );
        assert_eq!(value(&c, "index-url"), None);
        let (_d, c) = layered(
            "[global]\nindex-url = https://user/simple\n",
            "",
            &[("PIP_CONFIG_FILE", "{D}/missing.conf")],
        );
        assert_eq!(
            value(&c, "index-url").as_deref(),
            Some("https://user/simple")
        );
    }

    /// (c) Empty values are skipped: `PIP_INDEX_URL=` keeps the configured index, an empty file value keeps the
    /// earlier one.
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn empty_values_are_ignored() {
        if host_has_global_conf() {
            return;
        }
        let (_d, c) = layered(
            "[global]\nindex-url = https://user/simple\n",
            "",
            &[("PIP_INDEX_URL", "")],
        );
        assert_eq!(
            value(&c, "index-url").as_deref(),
            Some("https://user/simple")
        );
        let (_d, c) = layered(
            "[install]\nindex-url =\n",
            "[global]\nindex-url = https://g/simple\n",
            &[],
        );
        assert_eq!(value(&c, "index-url").as_deref(), Some("https://g/simple"));
    }

    /// (d) Only names starting with exactly `PIP_` count (on POSIX, where names are case-sensitive), and
    /// `PIP_VERSION`/`PIP_HELP` are not options.
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn only_upper_case_pip_variables() {
        if host_has_global_conf() {
            return;
        }
        let (_d, c) = layered(
            "",
            "",
            &[
                ("pip_index_url", "https://lower/simple"),
                ("Pip_Pre", "1"),
                ("PIP_VERSION", "1"),
                ("PIP_HELP", "1"),
                ("PIP_TIMEOUT", "7"),
            ],
        );
        assert_eq!(
            c.options,
            vec![(
                "timeout".to_owned(),
                "7".to_owned(),
                "PIP_TIMEOUT".to_owned()
            )]
        );
    }

    /// (e) A leading `--` of a file key or variable name is dropped (`_normalize_name`).
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn leading_dashes_are_dropped() {
        if host_has_global_conf() {
            return;
        }
        let (_d, c) = layered("[global]\n--Index_URL = https://dash/simple\n", "", &[]);
        assert_eq!(
            value(&c, "index-url").as_deref(),
            Some("https://dash/simple")
        );
        assert_eq!(normalize_name("__index_url"), "index-url");
        assert_eq!(normalize_name("----x"), "--x");
    }

    /// pip's timeout is a float; uv wants whole seconds.
    #[test]
    fn float_timeouts_round_up() {
        let none = Env::from_pairs::<&str, &str>([]);
        for (v, want) in [("60.0", "60"), ("2.5", "3"), ("15", "15"), ("0", "0")] {
            let t = translate(&cfg(&[("timeout", v)]), &none).unwrap();
            assert_eq!(
                t.env,
                vec![("UV_HTTP_TIMEOUT".to_owned(), OsString::from(want))],
                "{v}"
            );
        }
        for bad in ["abc", "-1", "inf", "nan"] {
            assert!(
                translate(&cfg(&[("timeout", bad)]), &none).is_err(),
                "{bad}"
            );
        }
        // `--timeout` and `--default-timeout` share one destination: the later one wins.
        let t = translate(&cfg(&[("timeout", "5"), ("default-timeout", "8")]), &none).unwrap();
        assert_eq!(
            t.env,
            vec![("UV_HTTP_TIMEOUT".to_owned(), OsString::from("8"))]
        );
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
