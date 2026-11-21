//! `gc`, `clean` (`commands/gc.py`, `clean.py`; §5.6) and `snp cache`.
//!
//! Owner: WP-33.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use snp_cli::{CacheAction, CacheArgs};
use snp_config::facts::{self, RepoFacts, Value};
use snp_config::{ConfigSource, check_config, check_manifest, check_manifest_bytes};
use snp_core::failure::Failure;
use snp_core::output::Level;
use snp_pyre::RegexCache;
use snp_store::{
    MarkerKey, RepoHandle, RepoId, RepoKey, Store, StoreGuard, TrackedConfig, db_repo_name,
    marker_repo_name,
};

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
#[derive(Debug, Default, PartialEq, Eq)]
struct Used {
    /// Repository keys still in use (pre-commit's `unused.discard((repo, rev))`).
    keys: HashSet<RepoKey>,
    /// `(marker_repo_name, rev)` of every used remote key whose manifest was loadable (present or Tier A): their
    /// `keys-v1` `Initialized` markers stay.
    remote_markers: HashSet<(String, String)>,
    /// `db_repo_name` of every used local key (`local`, `local:dep,…`).
    local_markers: HashSet<String>,
}

/// The manifest of a remote `(repo, rev)` as gc can load it, checked but not defaulted (`snp_config::facts`): from
/// the materialised clone, else from the embedded Tier A profile of pre-commit-hooks (ADR-075). `None` = not cloned,
/// or `InvalidManifestError` (the repository stays unused and is removed, `gc.py:33-38`). The check fails and warns
/// exactly when a full `load_manifest` does.
///
/// `present` is gc's one `repos-v1` snapshot, taken under the exclusive lock (pre-commit's single `SELECT repo, ref,
/// path FROM repos`, `gc.py:65-70`): nothing can materialise or retire a repository while gc holds that lock.
fn gc_manifest(
    cx: &CmdCtx<'_>,
    repo: &str,
    rev: &str,
    key: &RepoKey,
    present: &HashMap<RepoKey, &RepoHandle>,
    regexes: &RegexCache,
) -> Option<Value> {
    let o = frame::load_opts(cx, regexes, frame::ext_mode(cx));
    if let Some(h) = present.get(key) {
        let path = h.src.join(".pre-commit-hooks.yaml");
        return check_manifest(&path, path.as_os_str(), &o).ok();
    }
    if snp_hooks_data::is_pre_commit_hooks_url(repo)
        && let Some(profile) = snp_hooks_data::profile_for_rev(rev)
    {
        return check_manifest_bytes(
            profile.manifest_yaml.as_bytes(),
            OsStr::new(".pre-commit-hooks.yaml"),
            &o,
        )
        .ok();
    }
    None
}

/// `_mark_used_repos` (`gc.py:18-58`) for the repositories of one checked config, with every key derived by
/// `RepoId::from_config(repo, tracked.project_root)` (ADR-075). `manifest(repo, rev, base key)` is the checked
/// manifest of a remote repository ([`gc_manifest`]), loaded once per entry as pre-commit does.
///
/// A config hook that ends up without `additional_dependencies` uses the base key and marker, which the
/// repository's entry has already marked: only hooks with dependencies derive more.
fn mark_used(
    repos: &[RepoFacts<'_>],
    root: &Path,
    used: &mut Used,
    manifest: &mut dyn FnMut(&str, &str, &RepoKey) -> Option<Value>,
) {
    for repo in repos {
        match repo {
            RepoFacts::Local { deps } => {
                for d in deps {
                    used.keys.insert(Store::local_key(d));
                    used.local_markers.insert(db_repo_name("local", d));
                }
            }
            RepoFacts::Remote { repo, rev, hooks } => {
                let id = RepoId::from_config(repo, root);
                let key = Store::remote_key(&id, rev, &[]);
                let Some(checked) = manifest(repo, rev, &key) else {
                    continue;
                };
                let defined = facts::manifest_hooks(&checked);
                used.keys.insert(key);
                used.remote_markers
                    .insert((marker_repo_name(&id, repo, &[]), (*rev).to_owned()));
                for h in hooks {
                    let Some(manifest_deps) = defined.deps(h.id) else {
                        continue;
                    };
                    let deps = h.deps.as_deref().unwrap_or(manifest_deps);
                    if deps.is_empty() {
                        continue;
                    }
                    used.keys.insert(Store::remote_key(&id, rev, deps));
                    used.remote_markers
                        .insert((marker_repo_name(&id, repo, deps), (*rev).to_owned()));
                }
            }
            _ => {}
        }
    }
}

/// Checks one tracked config for gc: `None` = dead (pre-commit catches `InvalidConfigError`; snp also treats the
/// configs that would crash pre-commit's loader as dead, X13-CFG-14 / X13-CMD-09). The check fails and warns
/// exactly when a full `load_config` does; gc reads its few keys from the checked value (`snp_config::facts`).
fn load_tracked(cx: &CmdCtx<'_>, t: &TrackedConfig, regexes: &RegexCache) -> Option<Value> {
    let src = ConfigSource {
        path: t.path.clone(),
        display: t.path.as_os_str().to_os_string(),
        project_root: t.project_root.clone(),
    };
    check_config(&src, &frame::load_opts(cx, regexes, frame::ext_mode(cx))).ok()
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
    // One snapshot of the repositories (ordered by key, so the retirements and the first failure are deterministic).
    let repos = cx.store.all_repos();
    let present: HashMap<RepoKey, &RepoHandle> = repos.iter().map(|h| (h.key, h)).collect();
    let regexes = RegexCache::default();
    let mut used = Used::default();
    let mut dead = Vec::new();
    for t in cx.store.tracked_configs() {
        if let Some(f) = frame::interrupted(cx) {
            return Err(f);
        }
        match load_tracked(cx, &t, &regexes) {
            Some(checked) => mark_used(
                &facts::config_repos(&checked, frame::ext_mode(cx)),
                &t.project_root,
                &mut used,
                &mut |repo, rev, key| gc_manifest(cx, repo, rev, key, &present, &regexes),
            ),
            None => dead.push(t.path),
        }
    }
    cx.store.forget_configs(&dead);
    let mut removed: usize = 0;
    for h in &repos {
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
    // The `Initialized` markers of used remote keys are kept without being read (their file names are derived).
    let kept: Vec<MarkerKey> = used
        .remote_markers
        .iter()
        .map(|(name, rev)| MarkerKey::Initialized {
            db_repo_name: Arc::from(name.as_str()),
            rev: Arc::from(rev.as_str()),
        })
        .collect();
    let _stale = cx.store.retain_markers_keeping(&kept, &mut keep_marker);
    if report.legacy_v1_removed {
        cx.printer
            .diag(&messages::removed_v1(cx.store.location().path.as_os_str()));
    }
    if let Some(journal) = &report.unreadable_journal {
        cx.printer
            .diag(&messages::patches_kept(journal.as_os_str()));
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
/// pre-commit's store (D-STORE-02), and always prints `Cleaned {store}.` (ADR-030). A stash journal it cannot read
/// keeps every stash patch, with a stderr warning (D-DATA-04, D-DIAG-05).
pub fn clean(cx: &CmdCtx<'_>) -> Result<i32, Failure> {
    if cx.store.exists() {
        let _lock = exclusive(cx)?;
        if let Some(journal) = cx.store.clean()? {
            cx.printer
                .diag(&messages::patches_kept(journal.as_os_str()));
        }
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

    mod marks {
        use std::fmt::Write as _;
        use std::path::Path;

        use proptest::prelude::*;
        use snp_config::{
            Config, ConfigSource, ExtMode, LoadOpts, Manifest, RepoConfig, load_config,
            load_manifest_bytes,
        };
        use snp_core::brand::Brand;
        use snp_core::output::CollectWarnings;

        use super::*;

        /// The marking before the check-only loads (the oracle): the fully loaded config and manifests, every
        /// manifest hook re-deriving its key and marker even without dependencies.
        fn mark_used_full(
            cfg: &Config,
            root: &Path,
            used: &mut Used,
            manifest: &mut dyn FnMut(&str, &str, &RepoKey) -> Option<Manifest>,
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
                        let Some(manifest) = manifest(repo, rev, &key) else {
                            continue;
                        };
                        used.keys.insert(key);
                        used.remote_markers
                            .insert((marker_repo_name(&id, repo, &[]), rev.to_string()));
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
                            let deps =
                                raw_deps.unwrap_or_else(|| m.additional_dependencies.to_vec());
                            used.keys.insert(Store::remote_key(&id, rev, &deps));
                            used.remote_markers
                                .insert((marker_repo_name(&id, repo, &deps), rev.to_string()));
                        }
                    }
                    _ => {}
                }
            }
        }

        const IDS: [&str; 3] = ["a", "b", "c"];
        const DEPS: [&str; 4] = [
            "",
            "additional_dependencies: []",
            "additional_dependencies: [x]",
            "additional_dependencies: [x, 'y==1']",
        ];

        /// A config hook or manifest hook: id, dependencies line, whether the required keys are present.
        type Hook = (usize, usize, bool);

        /// A `repos` entry: 0 URL, 1 local path (an existing directory), 2 local, 3 meta, 4 builtin; the
        /// repository number (repeats share a manifest); its hooks.
        type Repo = (u8, usize, Vec<Hook>);

        /// A manifest per repository number: absent, invalid, or the hooks.
        type Man = Option<Option<Vec<Hook>>>;

        fn hook_lines(out: &mut String, (id, deps, full): &Hook, indent: &str) {
            let id = IDS.get(*id).copied().unwrap_or("a");
            writeln!(out, "{indent}-   id: {id}").unwrap();
            if *full {
                writeln!(
                    out,
                    "{indent}    name: n\n{indent}    entry: e\n{indent}    language: system"
                )
                .unwrap();
            }
            if let Some(d) = DEPS.get(*deps).filter(|d| !d.is_empty()) {
                writeln!(out, "{indent}    {d}").unwrap();
            }
        }

        fn render_config(repos: &[Repo]) -> String {
            let mut s = String::from(if repos.is_empty() {
                "repos: []\n"
            } else {
                "repos:\n"
            });
            for (kind, n, hooks) in repos {
                match kind {
                    0 => writeln!(s, "-   repo: https://example.com/r{n}\n    rev: v{n}"),
                    1 => writeln!(s, "-   repo: ./d{n}\n    rev: v{n}"),
                    2 => writeln!(s, "-   repo: local"),
                    3 => writeln!(s, "-   repo: meta"),
                    _ => writeln!(s, "-   repo: builtin"),
                }
                .unwrap();
                s.push_str(if hooks.is_empty() {
                    "    hooks: []\n"
                } else {
                    "    hooks:\n"
                });
                for h in hooks {
                    if *kind == 3 {
                        s.push_str("    -   id: identity\n");
                    } else {
                        // local hooks need the required keys; builtin hooks take ids only
                        let h = if *kind == 2 {
                            (h.0, h.1, true)
                        } else if *kind == 4 {
                            (h.0, 0, false)
                        } else {
                            (h.0, h.1, false)
                        };
                        hook_lines(&mut s, &h, "    ");
                    }
                }
            }
            s
        }

        fn render_manifest(m: Option<&Vec<Hook>>) -> String {
            match m {
                None => "-   id: a\n".to_owned(),
                Some(hooks) if hooks.is_empty() => "[]\n".to_owned(),
                Some(hooks) => {
                    let mut s = String::new();
                    for h in hooks {
                        hook_lines(&mut s, &(h.0, h.1, true), "");
                    }
                    s
                }
            }
        }

        fn hook() -> impl Strategy<Value = Hook> {
            (0..IDS.len(), 0..DEPS.len(), Just(true))
        }

        fn repo() -> impl Strategy<Value = Repo> {
            (
                prop::sample::select(vec![0u8, 0, 1, 1, 2, 3, 4]),
                0usize..4,
                prop::collection::vec(hook(), 0..4),
            )
        }

        fn man() -> impl Strategy<Value = Man> {
            prop::option::weighted(
                0.85,
                prop::option::weighted(0.85, prop::collection::vec(hook(), 0..5)),
            )
        }

        /// Marks of `config` with `manifests` (by repository number) both ways: (old, new); `None` = the config
        /// does not load (both ways).
        fn marks(config: &str, manifests: &[Man], ext: ExtMode) -> Option<(Used, Used)> {
            let dir = tempfile::tempdir().unwrap();
            for n in 0..4 {
                fs::create_dir_all(dir.path().join(format!("d{n}"))).unwrap();
            }
            let path = dir.path().join("cfg.yaml");
            fs::write(&path, config).unwrap();
            let src = ConfigSource {
                path,
                display: "cfg.yaml".into(),
                project_root: dir.path().to_path_buf(),
            };
            let regexes = RegexCache::default();
            let (warn, diag) = (CollectWarnings::new(), CollectWarnings::new());
            let o = LoadOpts {
                brand: Brand::Snp,
                ext,
                regexes: &regexes,
                warn: &warn,
                diag: &diag,
            };
            let text = |repo: &str| -> Option<String> {
                let n: usize = repo.rsplit(['r', 'd']).next()?.parse().ok()?;
                manifests
                    .get(n)?
                    .as_ref()
                    .map(|m| render_manifest(m.as_ref()))
            };
            let display = OsStr::new(".pre-commit-hooks.yaml");
            let full = load_config(&src, &o);
            let checked = check_config(&src, &o);
            assert_eq!(full.is_ok(), checked.is_ok(), "{config}");
            let (cfg, checked) = (full.ok()?, checked.ok()?);
            let mut old = Used::default();
            mark_used_full(&cfg, &src.project_root, &mut old, &mut |repo, _, _| {
                load_manifest_bytes(text(repo)?.as_bytes(), display, &o).ok()
            });
            let mut new = Used::default();
            mark_used(
                &facts::config_repos(&checked, ext),
                &src.project_root,
                &mut new,
                &mut |repo, _, _| check_manifest_bytes(text(repo)?.as_bytes(), display, &o).ok(),
            );
            Some((old, new))
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(128))]

            /// The marks gc derives from the check-only loads equal the old marking's over the full loads.
            #[test]
            fn check_only_marks_equal_full_load_marks(
                repos in prop::collection::vec(repo(), 0..5),
                manifests in prop::collection::vec(man(), 4),
                strict in prop::bool::weighted(0.2),
            ) {
                let ext = if strict { ExtMode::Strict } else { ExtMode::Enabled };
                let config = render_config(&repos);
                if let Some((old, new)) = marks(&config, &manifests, ext) {
                    prop_assert_eq!(old, new, "{}", config);
                }
            }
        }

        /// The generators reach configs that load with remote keys per dependency set, local-path ids, local
        /// markers and repositories left unmarked by an invalid or absent manifest.
        #[test]
        fn marks_generator_is_not_vacuous() {
            use proptest::strategy::ValueTree as _;
            let mut runner = proptest::test_runner::TestRunner::deterministic();
            let strategy = (
                prop::collection::vec(repo(), 0..5),
                prop::collection::vec(man(), 4),
            );
            let (mut loaded, mut dep_keys, mut abs, mut local) = (0, 0, 0, 0);
            for _ in 0..200 {
                let (repos, manifests) = strategy.new_tree(&mut runner).unwrap().current();
                let Some((old, new)) = marks(&render_config(&repos), &manifests, ExtMode::Enabled)
                else {
                    continue;
                };
                assert_eq!(old, new);
                loaded += 1;
                dep_keys += usize::from(new.remote_markers.iter().any(|(n, _)| n.contains(":x")));
                abs += usize::from(
                    new.remote_markers
                        .iter()
                        .any(|(n, _)| n.starts_with("abs:")),
                );
                local += usize::from(new.local_markers.iter().any(|n| n.starts_with("local:")));
            }
            assert!(
                loaded >= 100 && dep_keys >= 20 && abs >= 20 && local >= 20,
                "loaded {loaded}, dep keys {dep_keys}, abs {abs}, local {local}"
            );
        }
    }
}
