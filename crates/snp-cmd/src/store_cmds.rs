//! `gc`, `clean` (`commands/gc.py`, `clean.py`; §5.6) and `snp cache`.
//!
//! Owner: WP-33.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use snp_cli::{CacheAction, CacheArgs};
use snp_config::{
    ConfigSource, Manifest, RepoConfig, load_config, load_manifest, load_manifest_bytes,
};
use snp_core::failure::Failure;
use snp_core::output::Level;
use snp_pyre::RegexCache;
use snp_store::{MarkerKey, RepoId, RepoKey, Store, StoreGuard, TrackedConfig, db_repo_name};

use crate::{CmdCtx, frame, messages};

/// The exclusive store lock, printing `[INFO] Locking … directory` once when it has to wait (`store.py:105-106`).
fn exclusive(cx: &CmdCtx<'_>) -> Result<StoreGuard, Failure> {
    let mut printed = false;
    let mut on_wait = || {
        if !printed {
            printed = true;
            cx.printer
                .log(Level::Info, snp_store::locking_message(cx.brand));
        }
    };
    cx.store.exclusive(&mut on_wait, cx.cancel)
}

/// What gc's marking keeps (ADR-075).
#[derive(Debug, Default)]
struct Used {
    /// Repository keys still in use (pre-commit's `unused.discard((repo, rev))`).
    keys: HashSet<RepoKey>,
    /// `(db_repo_name, rev)` of every used remote key whose manifest was loadable (present or Tier A): their
    /// `keys-v1` `Initialized` markers stay.
    remote_markers: HashSet<(String, String)>,
    /// `db_repo_name` of every used local key (`local`, `local:dep,…`).
    local_markers: HashSet<String>,
}

/// The `additional_dependencies` a config hook sets (config-level dict, no defaults), when it sets them.
fn config_hook_deps(raw_deps: Option<Vec<Arc<str>>>, manifest_deps: &[Arc<str>]) -> Vec<Arc<str>> {
    raw_deps.unwrap_or_else(|| manifest_deps.to_vec())
}

/// The manifest of a remote `(repo, rev)` as gc can load it: from the materialised clone, else from the embedded
/// Tier A profile of pre-commit-hooks (ADR-075). `None` = not cloned, or `InvalidManifestError` (the repository
/// stays unused and is removed, `gc.py:33-38`).
fn gc_manifest(
    cx: &CmdCtx<'_>,
    repo: &str,
    rev: &str,
    key: &RepoKey,
    regexes: &RegexCache,
) -> Option<(Manifest, bool)> {
    let o = frame::load_opts(cx, regexes, frame::ext_mode(cx));
    if let Some(h) = cx.store.lookup(key) {
        let path = h.src.join(".pre-commit-hooks.yaml");
        return load_manifest(&path, path.as_os_str(), &o)
            .ok()
            .map(|m| (m, true));
    }
    if snp_hooks_data::is_pre_commit_hooks_url(repo)
        && let Some(profile) = snp_hooks_data::profile_for_rev(rev)
    {
        return load_manifest_bytes(
            profile.manifest_yaml.as_bytes(),
            OsStr::new(".pre-commit-hooks.yaml"),
            &o,
        )
        .ok()
        .map(|m| (m, false));
    }
    None
}

/// `_mark_used_repos` (`gc.py:18-58`) for one loaded config, with every key derived by
/// `RepoId::from_config(repo, tracked.project_root)` (ADR-075).
fn mark_used(
    cx: &CmdCtx<'_>,
    cfg: &snp_config::Config,
    root: &Path,
    used: &mut Used,
    regexes: &RegexCache,
) {
    for repo in &cfg.repos {
        match repo {
            RepoConfig::Local { hooks } => {
                for h in hooks {
                    used.keys
                        .insert(Store::local_key(&h.additional_dependencies));
                    used.local_markers
                        .insert(db_repo_name("local", &h.additional_dependencies));
                }
            }
            RepoConfig::Remote { repo, rev, hooks } => {
                let id = RepoId::from_config(repo, root);
                let key = Store::remote_key(&id, rev, &[]);
                let Some((manifest, _present)) = gc_manifest(cx, repo, rev, &key, regexes) else {
                    continue;
                };
                used.keys.insert(key);
                used.remote_markers
                    .insert((db_repo_name(repo, &[]), rev.to_string()));
                for h in hooks {
                    let Some(m) = manifest.get(&h.id) else {
                        continue;
                    };
                    let raw_deps = h
                        .raw
                        .iter()
                        .find(|(k, _)| k.as_str() == Some("additional_dependencies"))
                        .and_then(|(_, v)| v.as_array())
                        .map(|deps| {
                            deps.iter()
                                .filter_map(|d| d.as_str().map(Arc::from))
                                .collect::<Vec<Arc<str>>>()
                        });
                    let deps = config_hook_deps(raw_deps, &m.additional_dependencies);
                    used.keys.insert(Store::remote_key(&id, rev, &deps));
                    used.remote_markers
                        .insert((db_repo_name(repo, &deps), rev.to_string()));
                }
            }
            _ => {}
        }
    }
}

/// Loads one tracked config for gc: `None` = dead (pre-commit catches `InvalidConfigError`; snp also treats the
/// configs that would crash pre-commit's loader as dead, X13-CFG-14 / X13-CMD-09).
fn load_tracked(
    cx: &CmdCtx<'_>,
    t: &TrackedConfig,
    regexes: &RegexCache,
) -> Option<snp_config::Config> {
    let src = ConfigSource {
        path: t.path.clone(),
        display: t.path.as_os_str().to_os_string(),
        project_root: t.project_root.clone(),
    };
    load_config(&src, &frame::load_opts(cx, regexes, frame::ext_mode(cx))).ok()
}

/// `pre-commit gc`: `_mark_used_repos` over the tracked configs, then sweep; `{n} repo(s) removed.`.
///
/// `gc.py:61-96` under the exclusive store lock: every tracked config is loaded (its warnings print, parity); a
/// config that fails to load is forgotten and protects nothing; the used keys follow pre-commit's rules (meta
/// skipped, local keys by deps, remote keys by `(repo, rev)` and per-hook deps, only when the manifest loads);
/// every other repository is retired (ENOENT-tolerant, X13-CMD-07; other failures are the `OSError`, exit 3,
/// X13-CMD-08). Then snp's own maintenance: `sweep` (snp 1.x leftovers announced on stderr, D-DIAG-05) and the
/// `keys-v1` markers of keys that are neither present nor Tier-A-loadable (ADR-075). A store that does not exist
/// is not created: nothing is tracked, `0 repo(s) removed.`.
pub fn gc(cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    if !cx.store.exists() {
        cx.printer.write_line(messages::repos_removed(0).as_bytes());
        return Ok(0);
    }
    cx.store.ensure_created(cx.brand, cx.printer)?;
    let _lock = exclusive(cx)?;
    let regexes = RegexCache::default();
    let mut used = Used::default();
    let mut dead = Vec::new();
    for t in cx.store.tracked_configs() {
        if let Some(f) = frame::interrupted(cx) {
            return Err(f);
        }
        match load_tracked(cx, &t, &regexes) {
            Some(cfg) => mark_used(cx, &cfg, &t.project_root, &mut used, &regexes),
            None => dead.push(t.path),
        }
    }
    cx.store.forget_configs(&dead);
    let mut removed: usize = 0;
    for h in cx.store.all_repos() {
        if used.keys.contains(&h.key) {
            continue;
        }
        cx.store.retire_repo(&h.key).map_err(|e| {
            Failure::from_io(&e, Some(h.src.parent().unwrap_or(&h.src).as_os_str()), None)
        })?;
        removed = removed.saturating_add(1);
    }
    let report = cx.store.sweep(SystemTime::now());
    let keep_marker = |k: &MarkerKey| match k {
        MarkerKey::Initialized { db_repo_name, rev } => {
            used.remote_markers
                .contains(&(db_repo_name.to_string(), rev.to_string()))
                || used.local_markers.contains(&**db_repo_name)
        }
        // Virtual-install markers are derived from install keys gc cannot recompute; they are only consulted for
        // a repository whose `Initialized` marker (kept or not, above) decides whether anything is announced.
        _ => true,
    };
    let mut keep_marker = keep_marker;
    let _stale = cx.store.retain_markers(&mut keep_marker);
    if report.legacy_v1_removed {
        cx.printer
            .diag(&messages::removed_v1(cx.store.location().path.as_os_str()));
    }
    cx.printer
        .write_line(messages::repos_removed(removed).as_bytes());
    Ok(0)
}

/// `pre-commit clean`: `Cleaned {store}.`.
///
/// `clean.py:10-16`: pre-commit removes its store (re-created by `Store()` before dispatch, so the line always
/// prints) and the pre-1.0 `~/.pre-commit`. snp retires its own layout under the exclusive lock (recovery data,
/// `.lock` and foreign files kept, D-DATA-04), never creates the store, never touches `~/.pre-commit` or
/// pre-commit's store (D-STORE-02), and always prints `Cleaned {store}.` (ADR-030).
pub fn clean(cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    if cx.store.exists() {
        let _lock = exclusive(cx)?;
        cx.store.clean()?;
    }
    cx.printer
        .write_line(&messages::cleaned(cx.store.location().path.as_os_str()));
    Ok(0)
}

/// Total size in bytes of the regular files below `dir` (symlinks are not followed; unreadable entries count 0).
fn tree_size(dir: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else {
                continue;
            };
            if md.is_dir() {
                stack.push(e.path());
            } else if md.is_file() {
                total = total.saturating_add(md.len());
            }
        }
    }
    total
}

/// `12345` → `12.1 KiB` (binary units, one decimal; bytes below 1 KiB as `N B`).
pub(crate) fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut unit = 0_usize;
    let mut scaled = n;
    while scaled >= 1024 * 1024 && unit.saturating_add(1) < UNITS.len() {
        scaled /= 1024;
        unit = unit.saturating_add(1);
    }
    // `scaled` is in [1 KiB, 1 MiB) of the current unit: one decimal from the remainder.
    let whole = scaled / 1024;
    let tenth = (scaled % 1024).saturating_mul(10) / 1024;
    format!(
        "{whole}.{tenth} {}",
        UNITS.get(unit).copied().unwrap_or("PiB")
    )
}

/// `snp cache {dir,size,gc,clean}`.
///
/// `dir` prints the store path (whether or not it exists yet); `size` the total size of its files
/// (`{human} ({bytes} bytes)`, `0 B (0 bytes)` for a missing store); `gc` and `clean` are [`gc`] and [`clean`].
pub fn cache(a: &CacheArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    let path = &cx.store.location().path;
    match a.action {
        CacheAction::Dir => {
            cx.printer.write_line(path.as_os_str().as_encoded_bytes());
            Ok(0)
        }
        CacheAction::Size => {
            let n = if cx.store.exists() {
                tree_size(path)
            } else {
                0
            };
            cx.printer
                .write_line(format!("{} ({n} bytes)", human_size(n)).as_bytes());
            Ok(0)
        }
        CacheAction::Gc => gc(cx),
        CacheAction::Clean => clean(cx),
        _ => Err(frame::unsupported("cache")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(5 * 1024 * 1024 + 512 * 1024), "5.5 MiB");
        assert_eq!(human_size(u64::MAX), "16383.9 PiB");
    }

    #[test]
    fn deps_default_to_the_manifest() {
        let m: Vec<Arc<str>> = vec![Arc::from("six")];
        assert_eq!(config_hook_deps(None, &m), m);
        assert_eq!(
            config_hook_deps(Some(Vec::new()), &m),
            Vec::<Arc<str>>::new()
        );
    }
}
