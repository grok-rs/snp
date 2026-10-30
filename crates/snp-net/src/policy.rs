//! The environment-derived download policy of [`crate::HttpFetch`]: offline mode, the insecure-mirror switch, mirror
//! and checksum-manifest overrides, proxies and trust roots (ARCHITECTURE §6.3, ADR-034, ADR-077).
//!
//! Everything is read once from the captured [`Env`] snapshot (S6): ureq's own `Proxy::try_from_env` default is
//! always overridden, so the process environment is never consulted behind snp's back.
//!
//! Owner: WP-16.

use std::path::PathBuf;

use snp_core::env::Env;

use crate::error::scrub_url;

/// pre-commit's (nodeenv's) canonical origin of official node builds.
pub(crate) const NODE_DIST: &str = "https://nodejs.org/dist";
/// The origin of the unofficial node builds (musl, riscv64, …).
pub(crate) const NODE_UNOFFICIAL: &str = "https://unofficial-builds.nodejs.org/download/release";
/// The go.dev release list with every file and its sha256 (the checksum manifest of go archives).
pub(crate) const GO_JSON_ALL: &str = "https://go.dev/dl/?mode=json&include=all";
/// The go.dev list of the current stable releases (`golang.py` `_infer_go_version`).
pub(crate) const GO_JSON_LATEST: &str = "https://go.dev/dl/?mode=json";

/// The download policy.
#[derive(Clone, Debug, Default)]
pub(crate) struct Policy {
    /// `SNP_OFFLINE` (truthy flag): every network access fails with [`crate::NetError::Offline`].
    pub(crate) offline: bool,
    /// `SNP_ALLOW_INSECURE_MIRROR` (truthy flag): `http://` archive URLs are allowed (archives only; their checksum
    /// still comes from a TLS manifest or an embedded table).
    pub(crate) allow_insecure: bool,
    /// `SNP_NODE_MIRROR`: replaces [`NODE_DIST`] in node **archive** URLs (may be `http://` with the switch above).
    pub(crate) node_mirror: Option<String>,
    /// `SNP_NODE_SHASUMS_URL`: replaces [`NODE_DIST`] in node **manifest** URLs (`SHASUMS256.txt`, `index.json`);
    /// must be `https://`.
    pub(crate) node_shasums: Option<String>,
    /// `SNP_GO_JSON_URL`: replaces both go.dev JSON URLs; must be `https://`.
    pub(crate) go_json: Option<String>,
    /// Proxy for `https://` URLs: the `https` entry of [`proxies_environment`], else its `all` entry.
    pub(crate) https_proxy: Option<String>,
    /// Proxy for `http://` URLs: the `http` entry of [`proxies_environment`] (whose `HTTP_PROXY` is ignored under
    /// CGI), else its `all` entry.
    pub(crate) http_proxy: Option<String>,
    /// The `no` entry of [`proxies_environment`] (`no_proxy` / `NO_PROXY`): comma-separated host patterns that
    /// bypass the proxy.
    pub(crate) no_proxy: Option<String>,
    /// `SSL_CERT_FILE`: a PEM bundle that replaces the platform trust store (OpenSSL semantics).
    pub(crate) cert_file: Option<PathBuf>,
    /// `SSL_CERT_DIR`: directories of PEM certificates (`os.pathsep`-separated) that replace the platform store.
    pub(crate) cert_dirs: Vec<PathBuf>,
    /// A variable that cannot be used (non-UTF-8 URL, …): reported by the first network access.
    pub(crate) config_error: Option<String>,
}

impl Policy {
    /// Reads the policy from `env`.
    pub(crate) fn from_env(env: &Env) -> Self {
        let mut p = Self {
            offline: env.is_truthy_flag("SNP_OFFLINE"),
            allow_insecure: env.is_truthy_flag("SNP_ALLOW_INSECURE_MIRROR"),
            ..Self::default()
        };
        p.node_mirror = p.url_var(env, "SNP_NODE_MIRROR");
        p.node_shasums = p.url_var(env, "SNP_NODE_SHASUMS_URL");
        p.go_json = p.url_var(env, "SNP_GO_JSON_URL");
        let proxies = proxies_environment(env);
        let mut entry = |key: &str| {
            let (name, value) = proxies
                .iter()
                .find(|(k, _, _)| k == key)
                .map(|(_, n, v)| (n, v))?;
            if let Some(s) = value.to_str() {
                Some(s.to_owned())
            } else {
                p.error(format!("{} is not valid UTF-8", name.to_string_lossy()));
                None
            }
        };
        let (https, http, all, no) = (entry("https"), entry("http"), entry("all"), entry("no"));
        p.https_proxy = https.or_else(|| all.clone());
        p.http_proxy = http.or(all);
        p.no_proxy = no;
        p.cert_file = env.get_nonempty("SSL_CERT_FILE").map(PathBuf::from);
        if let Some(dirs) = env.get_nonempty("SSL_CERT_DIR") {
            p.cert_dirs = std::env::split_paths(dirs)
                .filter(|d| !d.as_os_str().is_empty())
                .collect();
        }
        p
    }

    /// A non-empty variable as UTF-8 (a non-UTF-8 value is recorded as a configuration error).
    fn str_var(&mut self, env: &Env, name: &str) -> Option<String> {
        let v = env.get_nonempty(name)?;
        if let Some(s) = v.to_str() {
            Some(s.to_owned())
        } else {
            self.error(format!("{name} is not valid UTF-8"));
            None
        }
    }

    /// A URL variable: trailing `/` removed (bases are joined with `/…`).
    fn url_var(&mut self, env: &Env, name: &str) -> Option<String> {
        let v = self.str_var(env, name)?;
        Some(v.trim_end_matches('/').to_owned())
    }

    /// Records the first configuration error.
    fn error(&mut self, msg: String) {
        if self.config_error.is_none() {
            self.config_error = Some(msg);
        }
    }

    /// Where a checksum manifest or version list at the canonical `url` is really fetched from (always https; checked
    /// by the caller).
    pub(crate) fn manifest_url(&self, url: &str) -> String {
        if let Some(base) = &self.node_shasums
            && let Some(rest) = url.strip_prefix(NODE_DIST)
            && rest.starts_with('/')
        {
            return format!("{base}{rest}");
        }
        if let Some(go) = &self.go_json
            && (url == GO_JSON_ALL || url == GO_JSON_LATEST)
        {
            return go.clone();
        }
        url.to_owned()
    }

    /// Where an archive at the canonical `url` is really fetched from (a configured mirror).
    pub(crate) fn archive_url(&self, url: &str) -> String {
        if let Some(base) = &self.node_mirror
            && let Some(rest) = url.strip_prefix(NODE_DIST)
            && rest.starts_with('/')
        {
            return format!("{base}{rest}");
        }
        url.to_owned()
    }
}

/// CPython's `urllib.request.getproxies_environment` (3.12/3.13) over `env`: `(scheme, variable, value)` entries.
///
/// Pass 1 takes every non-empty variable whose lowercased name ends in `_proxy` (any case: `HTTPS_PROXY`,
/// `Https_Proxy`; on a repeat the later variable wins); under CGI (`REQUEST_METHOD` set) the `http` entry is dropped
/// (httpoxy, CVE-2016-1000110: the `Proxy:` request header arrives as `HTTP_PROXY`). Pass 2 lets every variable whose
/// name ends in a lowercase `_proxy` override, and a set-but-empty one removes its scheme (`https_proxy=` disables an
/// `HTTPS_PROXY`). On Windows `os.environ` uppercases every name, so pass 2 never matches there. The `all` entry
/// (`ALL_PROXY`) is not used by urllib; snp adds it as the fallback of ureq and curl. Non-UTF-8 names are skipped.
fn proxies_environment(env: &Env) -> Vec<(String, std::ffi::OsString, std::ffi::OsString)> {
    let mut proxies: Vec<(String, std::ffi::OsString, std::ffi::OsString)> = Vec::new();
    let mut put = |key: String, name: &std::ffi::OsStr, value: &std::ffi::OsStr| {
        proxies.retain(|(k, _, _)| *k != key);
        if !value.is_empty() {
            proxies.push((key, name.to_os_string(), value.to_os_string()));
        }
    };
    for (name, value) in env.iter() {
        let Some(lower) = name.to_str().map(str::to_lowercase) else {
            continue;
        };
        if !value.is_empty()
            && let Some(key) = lower.strip_suffix("_proxy")
        {
            put(key.to_owned(), name, value);
        }
    }
    if env.is_present("REQUEST_METHOD") {
        put(
            "http".to_owned(),
            std::ffi::OsStr::new(""),
            std::ffi::OsStr::new(""),
        );
    }
    if !cfg!(windows) {
        for (name, value) in env.iter() {
            if let Some(s) = name.to_str()
                && s.ends_with("_proxy")
                && let Some(key) = s.to_lowercase().strip_suffix("_proxy")
            {
                put(key.to_owned(), name, value);
            }
        }
    }
    drop(put);
    proxies
}

/// `scheme://` of `url`, lowercased (`None` without `://`).
pub(crate) fn scheme(url: &str) -> Option<String> {
    url.split_once("://").map(|(s, _)| s.to_ascii_lowercase())
}

/// The text naming `url` in errors.
pub(crate) fn display(url: &str) -> String {
    scrub_url(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().copied())
    }

    #[test]
    fn flags_and_overrides() {
        let p = Policy::from_env(&env(&[
            ("SNP_OFFLINE", "1"),
            ("SNP_ALLOW_INSECURE_MIRROR", "0"),
            ("SNP_NODE_MIRROR", "http://mirror.local/node/"),
            ("SNP_NODE_SHASUMS_URL", "https://sums.local/dist"),
            ("SNP_GO_JSON_URL", "https://go.local/dl.json"),
        ]));
        assert!(p.offline);
        assert!(!p.allow_insecure);
        assert_eq!(
            p.archive_url("https://nodejs.org/dist/v1.2.3/node.tar.gz"),
            "http://mirror.local/node/v1.2.3/node.tar.gz"
        );
        assert_eq!(
            p.manifest_url("https://nodejs.org/dist/v1.2.3/SHASUMS256.txt"),
            "https://sums.local/dist/v1.2.3/SHASUMS256.txt"
        );
        // The archive mirror never serves manifests and vice versa.
        assert_eq!(
            p.manifest_url("https://nodejs.org/distx/a"),
            "https://nodejs.org/distx/a"
        );
        assert_eq!(p.manifest_url(GO_JSON_ALL), "https://go.local/dl.json");
        assert_eq!(p.manifest_url(GO_JSON_LATEST), "https://go.local/dl.json");
        assert_eq!(
            p.archive_url("https://dl.google.com/go/x.tar.gz"),
            "https://dl.google.com/go/x.tar.gz"
        );
    }

    #[test]
    fn proxy_precedence() {
        let p = Policy::from_env(&env(&[
            ("HTTPS_PROXY", "http://upper:1"),
            ("https_proxy", "http://lower:2"),
            ("HTTP_PROXY", "http://h:3"),
            ("ALL_PROXY", "http://all:4"),
            ("NO_PROXY", "a,b"),
        ]));
        assert_eq!(p.https_proxy.as_deref(), Some("http://lower:2"));
        assert_eq!(p.http_proxy.as_deref(), Some("http://h:3"));
        assert_eq!(p.no_proxy.as_deref(), Some("a,b"));
        let cgi = Policy::from_env(&env(&[
            ("HTTP_PROXY", "http://h:3"),
            ("REQUEST_METHOD", "GET"),
        ]));
        assert_eq!(cgi.http_proxy, None);
        // `all` is snp's fallback where urllib's result has no entry for the scheme.
        let all = Policy::from_env(&env(&[("ALL_PROXY", "http://all:4"), ("https_proxy", "")]));
        assert_eq!(all.https_proxy.as_deref(), Some("http://all:4"));
    }

    /// CPython 3.12.3: `env -i HTTPS_PROXY=http://upper:1 https_proxy= python3.12 -I -c 'import urllib.request as u;
    /// print(u.getproxies_environment())'` prints `{}`; `no_proxy= NO_PROXY=a` has no `no` entry either.
    #[test]
    fn proxies_match_urllib_getproxies_environment() {
        let p = Policy::from_env(&env(&[
            ("HTTPS_PROXY", "http://upper:1"),
            ("https_proxy", ""),
            ("NO_PROXY", "a"),
            ("no_proxy", ""),
            ("Http_Proxy", "http://mixed:2"),
        ]));
        // (On Windows the names are one variable each, whose last value is empty.)
        assert_eq!(p.https_proxy, None);
        assert_eq!(p.no_proxy, None);
        // Pass 1 matches any case.
        assert_eq!(p.http_proxy.as_deref(), Some("http://mixed:2"));
        // A lowercase-suffixed name of any prefix case overrides in pass 2 (`HTTPS_proxy`).
        let p = Policy::from_env(&env(&[
            ("HTTPS_proxy", "http://second:1"),
            ("HTTPS_PROXY", "http://first:2"),
        ]));
        let want = if cfg!(windows) {
            "http://first:2"
        } else {
            "http://second:1"
        };
        assert_eq!(p.https_proxy.as_deref(), Some(want));
        // CGI drops pass 1's `http` (any case), but a lowercase `http_proxy` comes back in pass 2.
        let p = Policy::from_env(&env(&[
            ("REQUEST_METHOD", "GET"),
            ("HTTP_PROXY", "http://header:1"),
        ]));
        assert_eq!(p.http_proxy, None);
        if !cfg!(windows) {
            let p = Policy::from_env(&env(&[
                ("REQUEST_METHOD", "GET"),
                ("HTTP_PROXY", "http://header:1"),
                ("http_proxy", "http://lower:2"),
            ]));
            assert_eq!(p.http_proxy.as_deref(), Some("http://lower:2"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_is_a_config_error() {
        use std::os::unix::ffi::OsStringExt;
        let env = Env::from_pairs([(
            std::ffi::OsString::from("SNP_NODE_MIRROR"),
            std::ffi::OsString::from_vec(vec![0xff]),
        )]);
        let p = Policy::from_env(&env);
        assert_eq!(p.node_mirror, None);
        assert_eq!(
            p.config_error.as_deref(),
            Some("SNP_NODE_MIRROR is not valid UTF-8")
        );
    }

    #[test]
    fn schemes() {
        assert_eq!(scheme("HTTPS://x").as_deref(), Some("https"));
        assert_eq!(scheme("x").as_deref(), None);
    }
}
