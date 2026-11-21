//! Check-only projections: the few raw keys gc's marking reads (`gc.py:18-58`, ADR-075), taken from the value
//! [`crate::check_config`] / [`crate::check_manifest`] return, without the schema defaults and the typed conversion
//! of a full load (ARCHITECTURE §5.6).
//!
//! A full load is `check` + `apply_defaults` + `convert`; the last two can neither fail nor warn, so a check-only
//! load succeeds, fails and warns exactly when the full load does. The projections give what `convert` would give
//! for the same keys: the `repo:` classification is the shared `repo_kind`, the defaults gc depends on (a local
//! or manifest hook without `additional_dependencies` has none) are applied here, and a manifest's by-id lookup
//! keeps the last duplicate like [`crate::Manifest::get`]. `tests/check_only.rs` checks the equality against the
//! full load for the oracle corpus and for generated configs and manifests.
//!
//! Owner: WP-21.

use std::sync::Arc;

use cfgv::Map;
/// The checked value of [`crate::check_config`], [`crate::check_manifest`] and [`crate::check_manifest_bytes`] (the
/// value `Config::raw` holds before the schema defaults).
pub use cfgv::Value;

use crate::ExtMode;
use crate::convert::{get, get_str};
use crate::items::{BUILTIN, LOCAL, META};

/// What a `repos` entry's `repo:` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepoKind {
    /// `repo: local`.
    Local,
    /// `repo: meta`.
    Meta,
    /// `repo: builtin` with the extensions enabled (§7.3).
    Builtin,
    /// Anything else (a URL or a path; `builtin` under `--strict-compat`).
    Remote,
}

/// The classification of `repo:` shared by `convert::repo_config` and [`config_repos`], so the two cannot drift.
pub(crate) fn repo_kind(url: &str, ext: ExtMode) -> RepoKind {
    match url {
        LOCAL => RepoKind::Local,
        META => RepoKind::Meta,
        BUILTIN if ext == ExtMode::Enabled => RepoKind::Builtin,
        _ => RepoKind::Remote,
    }
}

/// The hook dicts of a `repos` entry (non-mappings skipped, as `convert` does).
pub(crate) fn hook_maps(repo: &Map) -> impl Iterator<Item = &Map> {
    get(repo, "hooks")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_map)
}

/// The string items of a list value (what `convert::str_list` keeps), as owned keys.
fn strings(v: &Value) -> Vec<Arc<str>> {
    v.as_array()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(Arc::from)
        .collect()
}

/// A hook's `additional_dependencies` with the schema default `[]` (local and manifest hooks).
fn defaulted_deps(hook: &Map) -> Vec<Arc<str>> {
    get(hook, "additional_dependencies").map_or_else(Vec::new, strings)
}

/// One `repos` entry of a checked config, as gc reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RepoFacts<'a> {
    /// A remote repository: `RepoConfig::Remote`'s `repo`, `rev` and hooks.
    Remote {
        /// `repo` verbatim.
        repo: &'a str,
        /// `rev`.
        rev: &'a str,
        /// The config-level hooks, in config order.
        hooks: Vec<HookFacts<'a>>,
    },
    /// `repo: local`: each hook's `additional_dependencies` (default `[]`), in config order.
    Local {
        /// One entry per hook.
        deps: Vec<Vec<Arc<str>>>,
    },
    /// `repo: meta`.
    Meta,
    /// `repo: builtin` (extensions enabled).
    Builtin,
}

/// A config-level hook of a remote repository: its `id` and the `additional_dependencies` it sets itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookFacts<'a> {
    /// `id` (`""` when absent, as `ConfigHook::id`).
    pub id: &'a str,
    /// The config dict's own `additional_dependencies` (no default: `None` = the manifest's apply).
    pub deps: Option<Vec<Arc<str>>>,
}

/// The `repos` entries of a value [`crate::check_config`] accepted, in config order: what `Config::repos` holds for
/// the keys gc reads.
pub fn config_repos(checked: &Value, ext: ExtMode) -> Vec<RepoFacts<'_>> {
    let Some(root) = checked.as_map() else {
        return Vec::new();
    };
    get(root, "repos")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_map)
        .map(|repo| {
            let url = get_str(repo, "repo").unwrap_or_default();
            match repo_kind(url, ext) {
                RepoKind::Local => RepoFacts::Local {
                    deps: hook_maps(repo).map(defaulted_deps).collect(),
                },
                RepoKind::Meta => RepoFacts::Meta,
                RepoKind::Builtin => RepoFacts::Builtin,
                RepoKind::Remote => RepoFacts::Remote {
                    repo: url,
                    rev: get_str(repo, "rev").unwrap_or_default(),
                    hooks: hook_maps(repo)
                        .map(|h| HookFacts {
                            id: get_str(h, "id").unwrap_or_default(),
                            deps: get(h, "additional_dependencies")
                                .and_then(Value::as_array)
                                .map(|deps| {
                                    deps.iter()
                                        .filter_map(Value::as_str)
                                        .map(Arc::from)
                                        .collect()
                                }),
                        })
                        .collect(),
                },
            }
        })
        .collect()
}

/// The hooks of a checked manifest as gc reads them: `(id, additional_dependencies)` in file order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManifestFacts<'a> {
    /// `(id, additional_dependencies (default []))` per hook, in file order.
    pub hooks: Vec<(&'a str, Vec<Arc<str>>)>,
}

impl ManifestFacts<'_> {
    /// The `additional_dependencies` of hook `id`; the last duplicate wins, as in [`crate::Manifest::get`].
    pub fn deps(&self, id: &str) -> Option<&[Arc<str>]> {
        self.hooks
            .iter()
            .rev()
            .find(|(h, _)| *h == id)
            .map(|(_, deps)| deps.as_slice())
    }
}

/// The hooks of a value [`crate::check_manifest`] accepted: what `Manifest::hooks` holds for `id` and
/// `additional_dependencies`.
pub fn manifest_hooks(checked: &Value) -> ManifestFacts<'_> {
    ManifestFacts {
        hooks: checked
            .as_array()
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_map)
            .map(|h| (get_str(h, "id").unwrap_or_default(), defaulted_deps(h)))
            .collect(),
    }
}
