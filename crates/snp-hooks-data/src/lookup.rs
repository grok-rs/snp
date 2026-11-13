//! Lookups over the generated tables (09 §5.1–§5.4, §6.2–§6.3); no other logic.
//!
//! Owner: WP-17.

use crate::generated::blobs::VERIFIED_BLOBS;
use crate::generated::closures::IMPORT_CLOSURES;
use crate::generated::parity_gates::PARITY_GATES;
use crate::generated::scripts::CONSOLE_SCRIPTS;
use crate::generated::tags::TAG_PROFILES;
use crate::{Epoch, Gate, TagProfile};

/// Whether a config's `repo:` string names `github.com/pre-commit/pre-commit-hooks` (09 §6.2; the clone URL itself
/// stays verbatim, this is for matching only).
///
/// Accepted forms: `scheme://[userinfo@]host[:port]/path` with scheme `https`, `http`, `ssh`, `git` or `git+ssh`
/// (ASCII case-insensitive), and the scp-like `[user@]host:path` (no `/` before the `:`). The host is lowercased and a
/// leading `www.` stripped; it must be `github.com`. A port is allowed only as `443` with `https` and `22` with
/// `ssh`/`git+ssh`. The path loses one leading `/`, then one trailing `/`, then one trailing `.git` (ASCII
/// case-insensitive) and must be exactly `pre-commit/pre-commit-hooks`, compared ASCII case-insensitively (GitHub
/// routes case-insensitively). A query (`?`) or fragment (`#`) never matches; `local`, `meta`, `builtin`, `file://`
/// URLs, local paths, forks and mirrors never match (they reach Tier B through a clone). The userinfo (which may
/// hold a token) is parsed past, never stored or logged.
pub fn is_pre_commit_hooks_url(url: &str) -> bool {
    if matches!(url, "local" | "meta" | "builtin") || url.contains(['?', '#']) {
        return false;
    }
    let (scheme, authority, path) = if let Some((scheme, rest)) = url.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(
            scheme.as_str(),
            "https" | "http" | "ssh" | "git" | "git+ssh"
        ) {
            return false;
        }
        // The path keeps its leading `/` (stripped once below, like the scp form's optional one).
        let cut = rest.find('/').unwrap_or(rest.len());
        let (authority, path) = (
            rest.get(..cut).unwrap_or(rest),
            rest.get(cut..).unwrap_or(""),
        );
        (Some(scheme), authority, path)
    } else {
        // scp-like: `[user@]host:path`, the part before the first `:` has no `/`.
        let Some((authority, path)) = url.split_once(':') else {
            return false;
        };
        if authority.contains('/') || authority.is_empty() {
            return false;
        }
        (None, authority, path)
    };
    let hostport = authority
        .rsplit_once('@')
        .map_or(authority, |(_userinfo, h)| h);
    let (host, port) = match (&scheme, hostport.rsplit_once(':')) {
        (Some(_), Some((h, p))) => (h, Some(p)),
        (None, Some(_)) => return false,
        (_, None) => (hostport, None),
    };
    let host = host.to_ascii_lowercase();
    if host.trim_prefix("www.") != "github.com" {
        return false;
    }
    match (scheme.as_deref(), port) {
        (_, None) | (Some("https"), Some("443")) | (Some("ssh" | "git+ssh"), Some("22")) => {}
        _ => return false,
    }
    let path = path.trim_prefix('/');
    let path = path.trim_suffix('/');
    let path = strip_suffix_ascii_ci(path, ".git");
    let mut segments = path.split('/');
    matches!(
        (segments.next(), segments.next(), segments.next()),
        (Some(owner), Some(repo), None)
            if owner.eq_ignore_ascii_case("pre-commit") && repo.eq_ignore_ascii_case("pre-commit-hooks")
    )
}

/// `s` without one trailing `suffix` compared ASCII case-insensitively.
fn strip_suffix_ascii_ci<'a>(s: &'a str, suffix: &str) -> &'a str {
    let cut = s.len().checked_sub(suffix.len());
    match cut.and_then(|i| s.get(i..).map(|tail| (i, tail))) {
        Some((i, tail)) if tail.eq_ignore_ascii_case(suffix) => s.get(..i).unwrap_or(s),
        _ => s,
    }
}

/// Tier A (09 §6.3): the profile of a known tag (`rev` equal to the tag name, e.g. `v6.0.0`) or of the full commit of
/// a known tag (`rev` equal to its 40 lowercase hex digits, the form `pre-commit autoupdate --freeze` writes). Any
/// other rev (abbreviated SHA, branch, unknown tag) is `None` and goes to Tier B.
pub fn profile_for_rev(rev: &str) -> Option<&'static TagProfile> {
    TAG_PROFILES.iter().find(|p| p.tag == rev).or_else(|| {
        (rev.len() == 40 && rev.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .then(|| TAG_PROFILES.iter().find(|p| p.commit == rev))
            .flatten()
    })
}

/// Tier B (09 §6.3 step 3): the module and epoch of a verified blob of `pre_commit_hooks/<module>.py` (40 lowercase
/// hex digits, as `git ls-tree` prints them). `Epoch(0)` = a DELEGATE epoch; an unknown blob is `None` (delegate:
/// "implementation differs from verified upstream blobs").
pub fn verified_blob(blob_hex: &str) -> Option<(&'static str, Epoch)> {
    VERIFIED_BLOBS
        .binary_search_by(|(blob, ..)| (*blob).cmp(blob_hex))
        .ok()
        .and_then(|i| VERIFIED_BLOBS.get(i))
        .map(|&(_, module, epoch)| (module, epoch))
}

/// Console-script table (`setup.cfg` `[options.entry_points]`, union of the supported tags): the module in
/// `pre_commit_hooks/` (without `.py`) whose `main` a console script runs, e.g. `trailing-whitespace-fixer` →
/// `trailing_whitespace_fixer`. Entries whose target is not `main` (v4.0.0's broken `main_reverse`) are absent.
pub fn module_for_entry(entry: &str) -> Option<&'static str> {
    CONSOLE_SCRIPTS
        .binary_search_by(|(e, _)| (*e).cmp(entry))
        .ok()
        .and_then(|i| CONSOLE_SCRIPTS.get(i))
        .map(|&(_, module)| module)
}

/// The modules `module` imports from the package at any supported tag, transitively, plus the package `__init__`
/// (executed before any module); the module itself is not included. Tier B verifies the blob of every one of
/// them (09 §5.4). Unknown modules have an empty closure.
pub fn import_closure(module: &str) -> &'static [&'static str] {
    IMPORT_CLOSURES
        .binary_search_by(|(m, _)| (*m).cmp(module))
        .ok()
        .and_then(|i| IMPORT_CLOSURES.get(i))
        .map_or(&[], |&(_, closure)| closure)
}

/// Lookup in the generated table `src/generated/parity_gates.rs` (initial version by WP-17: Exact for NATIVE ids,
/// Delegate for caveat ids; regenerated by WP-27's `xtask pch-gates`). No TOML at run time, no build script.
/// Ids absent from the table delegate.
pub fn parity_gate(hook_id: &str) -> Gate {
    PARITY_GATES
        .binary_search_by(|(id, _)| (*id).cmp(hook_id))
        .ok()
        .and_then(|i| PARITY_GATES.get(i))
        .map_or(Gate::Delegate, |&(_, gate)| gate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HookClass;

    fn strictly_sorted<T, K: Ord>(xs: &[T], key: impl Fn(&T) -> K) -> bool {
        xs.iter()
            .zip(xs.iter().skip(1))
            .all(|(a, b)| key(a) < key(b))
    }

    /// Binary searches need strictly sorted keys; ids and blob ids are well-formed.
    #[test]
    fn tables_are_sorted_and_well_formed() {
        assert!(strictly_sorted(VERIFIED_BLOBS, |b| b.0));
        assert!(strictly_sorted(CONSOLE_SCRIPTS, |s| s.0));
        assert!(strictly_sorted(IMPORT_CLOSURES, |c| c.0));
        assert!(strictly_sorted(PARITY_GATES, |g| g.0));
        assert!(strictly_sorted(TAG_PROFILES, |p| p
            .tag
            .trim_start_matches('v')
            .split('.')
            .map(|n| n.parse::<u32>().unwrap())
            .collect::<Vec<_>>()));
        let hex40 =
            |s: &str| s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        assert!(VERIFIED_BLOBS.iter().all(|b| hex40(b.0)));
        assert!(TAG_PROFILES.iter().all(|p| hex40(p.commit)));
        assert!(
            IMPORT_CLOSURES
                .iter()
                .all(|(m, c)| strictly_sorted(c, |x| *x) && !c.contains(m))
        );
    }

    /// Every verified module has a closure; every console script's module has verified blobs.
    #[test]
    fn tables_are_consistent() {
        for (_, module, _) in VERIFIED_BLOBS {
            assert!(IMPORT_CLOSURES.iter().any(|(m, _)| m == module), "{module}");
        }
        for (entry, module) in CONSOLE_SCRIPTS {
            assert!(
                VERIFIED_BLOBS.iter().any(|(_, m, _)| m == module),
                "{entry}"
            );
        }
        for (module, closure) in IMPORT_CLOSURES {
            for dep in *closure {
                assert!(
                    VERIFIED_BLOBS.iter().any(|(_, m, _)| m == dep),
                    "{module} -> {dep}"
                );
            }
        }
    }

    /// Delegate hooks carry `Epoch(0)`; native ones a native epoch; ids are unique per tag.
    #[test]
    fn profile_classes_match_epochs() {
        for p in TAG_PROFILES {
            for (id, class, epoch) in p.hooks {
                let delegate = *class == HookClass::Delegate;
                assert_eq!(delegate, *epoch == Epoch(0), "{} {id}", p.tag);
            }
            let mut ids: Vec<&str> = p.hooks.iter().map(|h| h.0).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), p.hooks.len(), "{}", p.tag);
        }
    }

    #[test]
    fn suffix_strip() {
        assert_eq!(strip_suffix_ascii_ci("a.GIT", ".git"), "a");
        assert_eq!(strip_suffix_ascii_ci("git", ".git"), "git");
        assert_eq!(strip_suffix_ascii_ci("é.git", ".git"), "é");
        assert_eq!(strip_suffix_ascii_ci("xé", ".git"), "xé");
    }
}
