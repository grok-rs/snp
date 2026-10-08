//! `SNP_LOG=trace` spawn logging with the allow-list of ARCHITECTURE §2.5 / §10.5 (ADR-077).
//!
//! Values are printed only for `PATH`, `PATHEXT`, `HOME`, `USERPROFILE`, `TERM`, `LANG`, `LC_*`, `TZ`, `PRE_COMMIT*`,
//! `SNP_*`, `GIT_*` (except `GIT_ASKPASS`, `GIT_CONFIG_VALUE_*` and `GIT_CONFIG_PARAMETERS`, which carry credentials)
//! and the language activation variables snp sets itself; every other variable is logged as
//! `NAME=<redacted, N bytes>`. URL userinfo (`scheme://user:secret@host`) is scrubbed from argv and from every
//! printed value.
//!
//! Owner: WP-15.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::Path;

use snp_core::env::Env;

use crate::spawn::{Group, Role};

/// Variables whose values are shown verbatim (exact names).
const SHOWN: [&str; 7] = [
    "PATH",
    "PATHEXT",
    "HOME",
    "USERPROFILE",
    "TERM",
    "LANG",
    "TZ",
];

/// Language activation variables snp sets (04a §10.2, 04b): shown verbatim.
const ACTIVATION: [&str; 22] = [
    "PIP_DISABLE_PIP_VERSION_CHECK",
    "VIRTUAL_ENV",
    "PYTHONHOME",
    "NODE_VIRTUAL_ENV",
    "NPM_CONFIG_PREFIX",
    "NODE_PATH",
    "GOROOT",
    "GOPATH",
    "GOTOOLCHAIN",
    "RUSTUP_TOOLCHAIN",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "GEM_HOME",
    "GEM_PATH",
    "BUNDLE_IGNORE_CONFIG",
    "RBENV_ROOT",
    "RBENV_VERSION",
    "CONDA_PREFIX",
    "DOTNET_ROOT",
    "R_PROFILE_USER",
    "LUA_PATH",
    "PERL5LIB",
];

/// Prefixes whose values are shown (`LC_*`, `PRE_COMMIT*`, `SNP_*`, `GIT_*`).
const SHOWN_PREFIXES: [&str; 4] = ["LC_", "PRE_COMMIT", "SNP_", "GIT_"];

/// `GIT_*` names that carry credentials.
const GIT_SECRETS: [&str; 2] = ["GIT_ASKPASS", "GIT_CONFIG_PARAMETERS"];

/// Whether the value of `name` may be logged (ASCII-case-insensitive, as names are on Windows).
pub(crate) fn value_is_shown(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let upper = name.to_ascii_uppercase();
    if upper.starts_with("GIT_CONFIG_VALUE_") || GIT_SECRETS.contains(&upper.as_str()) {
        return false;
    }
    if SHOWN.contains(&upper.as_str()) || ACTIVATION.contains(&upper.as_str()) {
        return true;
    }
    // npm's lower-case spelling of the prefix variable.
    if name == "npm_config_prefix" {
        return true;
    }
    SHOWN_PREFIXES.iter().any(|p| upper.starts_with(p))
}

/// `s` with the userinfo of every `scheme://userinfo@host` URL replaced by `***`.
pub(crate) fn scrub_urls(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest
            .split_at_checked(i.saturating_add(3))
            .unwrap_or((rest, ""));
        out.push_str(head);
        // The authority ends at the first '/', '?', '#' or whitespace.
        let end = tail
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .unwrap_or(tail.len());
        let (authority, after) = tail.split_at_checked(end).unwrap_or((tail, ""));
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(authority.get(at..).unwrap_or_default());
            }
            None => out.push_str(authority),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// argv as a list of escaped strings, URL userinfo scrubbed.
pub(crate) fn render_argv(argv: &[std::ffi::OsString]) -> String {
    let mut out = String::from("[");
    for (i, a) in argv.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ignored = write!(out, "{:?}", scrub_urls(&a.to_string_lossy()));
    }
    out.push(']');
    out
}

/// The environment with the allow-list applied: `NAME="value"` or `NAME=<redacted, N bytes>`.
pub(crate) fn render_env(env: &Env) -> String {
    let mut out = String::new();
    for (k, v) in env.iter() {
        if !out.is_empty() {
            out.push(' ');
        }
        let name = k.to_string_lossy();
        if value_is_shown(k) {
            let _ignored = write!(out, "{name}={:?}", scrub_urls(&v.to_string_lossy()));
        } else {
            let _ignored = write!(out, "{name}=<redacted, {} bytes>", v.len());
        }
    }
    out
}

/// Emits the `snp::spawn` trace event (nothing is rendered unless a subscriber enables TRACE for it).
pub(crate) fn log_spawn(
    argv: &[std::ffi::OsString],
    env: &Env,
    cwd: &Path,
    role: Role,
    group: Group,
) {
    if tracing::enabled!(target: "snp::spawn", tracing::Level::TRACE) {
        tracing::trace!(
            target: "snp::spawn",
            role = ?role,
            group = ?group,
            cwd = %scrub_urls(&cwd.to_string_lossy()),
            argv = %render_argv(argv),
            env = %render_env(env),
            "spawn"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list() {
        for shown in [
            "PATH",
            "Path",
            "HOME",
            "LC_ALL",
            "LANG",
            "PRE_COMMIT_FROM_REF",
            "SNP_JOBS",
            "GIT_INDEX_FILE",
            "GIT_DIR",
            "VIRTUAL_ENV",
            "npm_config_prefix",
        ] {
            assert!(value_is_shown(OsStr::new(shown)), "{shown}");
        }
        for hidden in [
            "NPM_AUTH",
            "MYSQL_PWD",
            "OPENAI_API_KEY",
            "GIT_ASKPASS",
            "GIT_CONFIG_VALUE_0",
            "GIT_CONFIG_PARAMETERS",
            "AWS_SECRET_ACCESS_KEY",
            "npm_config__authToken",
            "LANGUAGE_SECRET",
        ] {
            assert!(!value_is_shown(OsStr::new(hidden)), "{hidden}");
        }
    }

    #[test]
    fn url_userinfo_is_scrubbed() {
        assert_eq!(
            scrub_urls("https://user:s3cret@example.com/repo.git"),
            "https://***@example.com/repo.git"
        );
        assert_eq!(
            scrub_urls("x https://tok@h.io?q=1 and ssh://git@h:22/p"),
            "x https://***@h.io?q=1 and ssh://***@h:22/p"
        );
        assert_eq!(
            scrub_urls("https://example.com/a@b"),
            "https://example.com/a@b"
        );
        assert_eq!(scrub_urls("plain"), "plain");
        assert_eq!(scrub_urls("://@"), "://***@");
        assert_eq!(scrub_urls("é://ü:p@ö"), "é://***@ö");
    }

    proptest::proptest! {
        /// Scrubbing never panics, is idempotent and leaves no userinfo before an `@` of a URL authority.
        #[test]
        fn scrub_is_total_and_idempotent(s in "\\PC{0,60}", user in "[a-z0-9:]{1,12}") {
            let once = scrub_urls(&s);
            proptest::prop_assert_eq!(scrub_urls(&once), once);
            let url = format!("x https://{user}@host/p {s}");
            let scrubbed = scrub_urls(&url);
            proptest::prop_assert!(scrubbed.starts_with("x https://***@host/p "));
        }
    }

    #[test]
    fn rendering_hides_secrets() {
        let env = Env::from_pairs([
            ("PATH", "/bin"),
            ("NPM_AUTH", "npm-secret"),
            ("MYSQL_PWD", "mysql-secret"),
            ("STRIPE_API_KEY", "stripe-secret"),
            (
                "PRE_COMMIT_REMOTE_URL",
                "https://me:remote-secret@git.example/x",
            ),
        ]);
        let e = render_env(&env);
        assert!(e.contains("PATH=\"/bin\""), "{e}");
        assert!(e.contains("NPM_AUTH=<redacted, 10 bytes>"), "{e}");
        assert!(
            e.contains("PRE_COMMIT_REMOTE_URL=\"https://***@git.example/x\""),
            "{e}"
        );
        for secret in [
            "npm-secret",
            "mysql-secret",
            "stripe-secret",
            "remote-secret",
        ] {
            assert!(!e.contains(secret), "{e}");
        }
        let a = render_argv(&[
            "git".into(),
            "clone".into(),
            "https://u:argv-secret@h/r".into(),
        ]);
        assert!(!a.contains("argv-secret"), "{a}");
        assert_eq!(a, r#"["git", "clone", "https://***@h/r"]"#);
    }
}
