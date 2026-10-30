//! The local overlay `.pre-commit-config.local.y[a]ml` (ARCHITECTURE §8.4, ADR-070, COMPATIBILITY §7.4,
//! D-EXT-02/03): root items are applied to the `Config`, hook patches are data merged by `merge_hook`.
//!
//! Owner: WP-21.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use cfgv::{Map, Value};
use indexmap::IndexMap;
use snp_core::env::Env;
use snp_core::output::DiagSink;

use crate::convert::{self, get};
use crate::ext::{self, Condition, FailOnChanges, HookExt};
use crate::load::{self, Kind};
use crate::{Config, ConfigError, ExtMode, LoadOpts, RepoConfig, messages, schema};

/// The key under which [`apply_root`] records the overlay in `Config::raw` for `snp config dump` (a tuple key no
/// YAML document can produce as a plain mapping key).
pub(crate) fn marker_key() -> Value {
    Value::Tuple(vec![Value::from("snp"), Value::from("local-overrides")])
}

/// A loaded overlay: hooks by alias|id, `skip_groups`, `fail_on_changes`, appended `repos`.
#[derive(Debug, Clone)]
pub struct Overlay {
    /// The overlay file.
    path: PathBuf,
    /// `hooks:` entries in file order, keyed by alias or id.
    hooks: IndexMap<String, HookPatch>,
    /// `skip_groups`.
    skip_groups: Vec<String>,
    /// `fail_on_changes`.
    fail_on_changes: Option<FailOnChanges>,
    /// `repos`, appended after the main config's repos.
    repos: Vec<RepoConfig>,
    /// The validated `repos` values (for `Config::raw`).
    raw_repos: Vec<Value>,
}

/// One hook entry of the overlay: validated hook keys (any hook key except id/alias/repo), `entry` possibly
/// containing `{entry}`, parsed extension keys. `keys()` counts as user config for fast-path G4 (§7.2).
#[derive(Debug, Clone)]
pub struct HookPatch {
    /// The validated hook keys (extension keys included, so that [`HookPatch::keys`] lists everything set).
    pub raw: Map,
    /// Parsed extension keys.
    pub ext: HookExt,
}

impl HookPatch {
    /// The keys the patch sets, in file order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.raw.keys().filter_map(Value::as_str)
    }
}

/// The overlay file names next to the config, in lookup order.
const OVERLAY_NAMES: [&str; 2] = [
    ".pre-commit-config.local.yaml",
    ".pre-commit-config.local.yml",
];

/// `<config dir>/.pre-commit-config.local.y[a]ml` or `SNP_LOCAL_CONFIG`; ignored when tracked by git or when CI is
/// truthy unless named by `SNP_LOCAL_CONFIG`; `Err(note)` = ignored with a stderr note.
///
/// `SNP_LOCAL_CONFIG` empty or `0` disables the overlay; a relative value is resolved against the config's
/// directory. `SNP_COMPAT_STRICT` disables the overlay altogether (ADR-039).
pub fn locate(
    cfg: &Path,
    env: &Env,
    is_tracked: &dyn Fn(&Path) -> bool,
) -> Result<Option<PathBuf>, String> {
    if env.is_truthy_flag("SNP_COMPAT_STRICT") {
        return Ok(None);
    }
    let dir = cfg.parent().unwrap_or_else(|| Path::new(""));
    if let Some(named) = env.get("SNP_LOCAL_CONFIG") {
        if named.is_empty() || named == OsStr::new("0") {
            return Ok(None);
        }
        let p = Path::new(named);
        let path = if p.is_absolute() {
            p.to_path_buf()
        } else {
            dir.join(p)
        };
        if !path.is_file() {
            return Err(messages::overlay_missing(&path.to_string_lossy()));
        }
        return Ok(Some(path));
    }
    let Some(path) = OVERLAY_NAMES
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
    else {
        return Ok(None);
    };
    if env.is_truthy_flag("CI") {
        return Err(messages::overlay_ci(&path.to_string_lossy()));
    }
    if is_tracked(&path) {
        return Err(messages::overlay_tracked(&path.to_string_lossy()));
    }
    Ok(Some(path))
}

/// Loads and validates an overlay file (cfgv `load_from_filename` semantics with the overlay schema; errors are
/// `InvalidConfigError`s under `File <path>`).
pub fn load(path: &Path, o: &LoadOpts<'_>) -> Result<Overlay, ConfigError> {
    let display = path.to_string_lossy();
    let bytes = load::read_file(path, &display, false)?;
    let value = load::validate_bytes(
        &bytes,
        &display,
        schema::overlay_schema(o.brand),
        Kind::Overlay,
        o,
    )?;
    load::future_warnings(&value, Kind::Overlay, o);
    Ok(from_value(path, &value))
}

fn from_value(path: &Path, value: &Value) -> Overlay {
    let empty = Map::new();
    let root = value.as_map().unwrap_or(&empty);
    let skip_groups: Vec<String> = get(root, "skip_groups")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let mut hooks = IndexMap::new();
    if let Some(patches) = get(root, "hooks").and_then(Value::as_map) {
        for (k, v) in patches {
            let (Some(key), Some(raw)) = (k.as_str(), v.as_map()) else {
                continue;
            };
            let mut ext = ext::parse_hook_ext(raw);
            if ext
                .groups
                .iter()
                .any(|g| skip_groups.iter().any(|s| s == &**g))
            {
                ext.skip.push(Condition::All(Vec::new()));
            }
            hooks.insert(
                key.to_owned(),
                HookPatch {
                    raw: raw.clone(),
                    ext,
                },
            );
        }
    }
    let raw_repos: Vec<Value> = get(root, "repos")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .to_vec();
    let repos = raw_repos
        .iter()
        .filter_map(Value::as_map)
        .map(|r| convert::repo_config(r, ExtMode::Enabled))
        .collect();
    Overlay {
        path: path.to_path_buf(),
        hooks,
        skip_groups,
        fail_on_changes: get(root, "fail_on_changes").and_then(ext::parse_fail_on_changes),
        repos,
        raw_repos,
    }
}

/// Marks every hook of `hooks` whose groups intersect `skip` as always skipped.
fn skip_groups_in(ext: &mut HookExt, skip: &[String]) {
    if ext.groups.iter().any(|g| skip.iter().any(|s| s == &**g)) {
        ext.skip.push(Condition::All(Vec::new()));
    }
}

/// Root items only: `skip_groups`, `fail_on_changes`, appended repos; prints the active-overlay stderr line.
///
/// `skip_groups` is applied as an always-true `skip` condition on every hook (of the config and of the appended
/// repos) whose `groups` intersect it, so the hook prints pre-commit's `Skipped` line like `SKIP=`. The overlay is
/// recorded in `Config::raw` for [`crate::effective_dump`].
pub fn apply_root(cfg: &mut Config, ov: &Overlay, diag: &dyn DiagSink) {
    diag.diag(&messages::overlay_active(&ov.path.to_string_lossy()));
    if let Some(f) = ov.fail_on_changes {
        cfg.root_ext.fail_on_changes = f;
    }
    cfg.repos.extend(ov.repos.iter().cloned());
    if !ov.skip_groups.is_empty() {
        for repo in &mut cfg.repos {
            match repo {
                RepoConfig::Remote { hooks, .. } | RepoConfig::Builtin { hooks } => {
                    for h in hooks {
                        skip_groups_in(&mut h.ext, &ov.skip_groups);
                    }
                }
                RepoConfig::Local { hooks } | RepoConfig::Meta { hooks } => {
                    for h in hooks {
                        skip_groups_in(&mut h.ext, &ov.skip_groups);
                    }
                }
            }
        }
    }
    let mut fields: Vec<Value> = Vec::new();
    for (key, patch) in &ov.hooks {
        for k in patch.keys() {
            fields.push(Value::from(format!("hooks.{key}.{k}")));
        }
    }
    if !ov.skip_groups.is_empty() {
        fields.push(Value::from("skip_groups"));
    }
    if ov.fail_on_changes.is_some() {
        fields.push(Value::from("fail_on_changes"));
    }
    if !ov.repos.is_empty() {
        fields.push(Value::from(format!("repos (+{})", ov.repos.len())));
    }
    if let Some(root) = cfg.raw.as_map_mut() {
        if let Some(Value::List(repos)) = root.get_mut(&Value::from("repos")) {
            repos.extend(ov.raw_repos.iter().cloned());
        }
        let mut marker = Map::new();
        marker.insert(Value::from("path"), Value::from(ov.path.to_string_lossy()));
        marker.insert(Value::from("fields"), Value::List(fields));
        root.insert(marker_key(), Value::Map(marker));
    }
}

impl Overlay {
    /// The overlay file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Keyed by alias first, else id (an id matching several hooks applies to all of them).
    pub fn hook_patch(&self, id: &str, alias: &str) -> Option<&HookPatch> {
        if !alias.is_empty()
            && let Some(p) = self.hooks.get(alias)
        {
            return Some(p);
        }
        self.hooks.get(id)
    }

    /// Overlay keys that matched no resolved hook → `snp: local config overrides unknown hook '<id>'` (stderr).
    /// `matched` lists the ids and aliases of the resolved hooks.
    pub fn unmatched<'a>(&'a self, matched: &[&str]) -> Vec<&'a str> {
        self.hooks
            .keys()
            .map(String::as_str)
            .filter(|k| !matched.contains(k))
            .collect()
    }
}

/// The stderr note for one [`Overlay::unmatched`] key (D-EXT-03).
pub fn unknown_hook_note(id: &str) -> String {
    messages::overlay_unknown_hook(id)
}
